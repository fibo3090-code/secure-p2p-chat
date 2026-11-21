//! Best-effort NAT traversal for hosting: UPnP (IGD) with a NAT-PMP fallback.
//!
//! When hosting, the app can ask the local router to forward the listening
//! port and report the router's external IP, so the generated invite carries
//! an address that works from outside the LAN. This is strictly best-effort:
//! no gateway, a gateway with the protocol disabled, or a carrier-grade NAT
//! all make it fail, and the caller falls back to relay-assisted
//! connectivity. Nothing here touches the wire protocol.

use anyhow::{Context, Result};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

/// Requested lease for the mapping. Routers reclaim the port after this even
/// if the app dies without unmapping; a background task renews it well before
/// expiry while hosting continues.
pub const LEASE_SECS: u32 = 60 * 60;

/// Re-map this long before the lease expires, so a renewal failure still
/// leaves a working mapping for a while.
pub const RENEW_AFTER: Duration = Duration::from_secs((LEASE_SECS as u64) * 2 / 3);

/// Hard cap on any single discovery + mapping exchange so a hosting toast is
/// never held hostage by a silent network.
const OVERALL_TIMEOUT: Duration = Duration::from_secs(15);

/// Which protocol produced a mapping (for logging / unmapping).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Upnp,
    NatPmp,
}

/// A successfully established router port mapping.
#[derive(Debug, Clone)]
pub struct MappedAddress {
    pub external_ip: IpAddr,
    pub external_port: u16,
    pub protocol: Protocol,
}

impl MappedAddress {
    pub fn to_host_port(&self) -> String {
        crate::util::format_host_port(&self.external_ip.to_string(), self.external_port)
    }
}

/// True if `ip` is not globally routable, so a router reporting it as the
/// "external" address means we're still behind another NAT (double-NAT or
/// carrier-grade NAT) and the mapping is useless to outside peers.
pub fn is_unroutable_external(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || is_cgnat(v4)
                || v4.is_documentation()
        }
        // Any global-unicast v6 is fine; treat the obviously-local ones as bad.
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified(),
    }
}

/// RFC 6598 shared address space (100.64.0.0/10) used by carrier-grade NAT.
fn is_cgnat(ip: &Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 100 && (64..=127).contains(&o[1])
}

/// Discover a gateway and map `local_port` (TCP, same external port),
/// returning the external address peers can connect to. Tries UPnP first,
/// then NAT-PMP.
pub async fn map_port(local_port: u16) -> Result<MappedAddress> {
    first_mapping(
        map_upnp(local_port),
        map_natpmp(local_port),
        OVERALL_TIMEOUT,
    )
    .await
}

/// UPnP, then NAT-PMP only if UPnP failed, each within `budget`. The NAT-PMP
/// future is not polled at all when UPnP succeeds — a second mapping on a
/// second protocol would be a forward nobody tracks or removes.
async fn first_mapping(
    upnp: impl std::future::Future<Output = Result<MappedAddress>>,
    natpmp: impl std::future::Future<Output = Result<MappedAddress>>,
    budget: Duration,
) -> Result<MappedAddress> {
    let secs = budget.as_secs();
    let upnp_err = match tokio::time::timeout(budget, upnp).await {
        Ok(Ok(mapping)) => return Ok(mapping),
        Ok(Err(e)) => e,
        Err(_) => anyhow::anyhow!("UPnP gateway did not answer within {secs}s"),
    };
    tracing::debug!(error = %upnp_err, "UPnP failed, trying NAT-PMP");

    match tokio::time::timeout(budget, natpmp).await {
        Ok(Ok(mapping)) => Ok(mapping),
        Ok(Err(natpmp_err)) => Err(anyhow::anyhow!(
            "UPnP failed ({upnp_err}); NAT-PMP failed ({natpmp_err})"
        )),
        Err(_) => Err(anyhow::anyhow!(
            "UPnP failed ({upnp_err}); NAT-PMP gateway did not answer within {secs}s"
        )),
    }
}

async fn map_upnp(local_port: u16) -> Result<MappedAddress> {
    let local_ip: IpAddr = crate::util::primary_local_ipv4()
        .context("could not determine local IP address")?
        .parse()
        .context("local IP did not parse")?;
    map_upnp_via(igd_next::SearchOptions::default(), local_ip, local_port).await
}

/// [`map_upnp`] against a gateway found with `options` — the real multicast
/// search in production, a loopback responder in tests.
async fn map_upnp_via(
    options: igd_next::SearchOptions,
    local_ip: IpAddr,
    local_port: u16,
) -> Result<MappedAddress> {
    let gateway = igd_next::aio::tokio::search_gateway(options)
        .await
        .context("no UPnP gateway found on this network")?;
    let local_addr = SocketAddr::new(local_ip, local_port);

    // Ask before mapping. A router whose own external address is private is
    // behind another NAT, and a forward on it reaches nobody outside — but it
    // still forwards the port to this machine for the whole lease. Checking
    // afterwards refused the mapping correctly and left it in place.
    let external_ip = gateway
        .get_external_ip()
        .await
        .context("router did not report an external IP")?;
    check_routable(external_ip)?;

    gateway
        .add_port(
            igd_next::PortMappingProtocol::TCP,
            local_port,
            local_addr,
            LEASE_SECS,
            "P2PEM host",
        )
        .await
        .context("router refused the port mapping")?;

    tracing::info!(%external_ip, local_port, "UPnP port mapping established");
    Ok(MappedAddress {
        external_ip,
        external_port: local_port,
        protocol: Protocol::Upnp,
    })
}

/// How long to wait for one NAT-PMP reply. The client library waits on
/// `recv` with no deadline of its own, so without this a network with no
/// NAT-PMP gateway spent the whole [`OVERALL_TIMEOUT`] finding that out.
const NATPMP_REPLY_TIMEOUT: Duration = Duration::from_secs(3);

type NatPmp = natpmp::NatpmpAsync<tokio::net::UdpSocket>;

async fn map_natpmp(local_port: u16) -> Result<MappedAddress> {
    let n = natpmp::new_tokio_natpmp()
        .await
        .context("could not open a NAT-PMP socket")?;
    map_natpmp_via(n, local_port, NATPMP_REPLY_TIMEOUT).await
}

/// [`map_natpmp`] over an already-connected client — the default gateway in
/// production, a loopback responder in tests.
async fn map_natpmp_via(
    mut n: NatPmp,
    local_port: u16,
    reply_timeout: Duration,
) -> Result<MappedAddress> {
    // Ask for the external IP.
    n.send_public_address_request()
        .await
        .context("NAT-PMP external-address request failed")?;
    let external_ip = match read_natpmp_response(&n, reply_timeout).await? {
        natpmp::Response::Gateway(g) => IpAddr::V4(*g.public_address()),
        _ => anyhow::bail!("unexpected NAT-PMP response to address request"),
    };
    // Before mapping, for the reason given in `map_upnp_via`.
    check_routable(external_ip)?;

    // Map the port (TCP), same external port, our lease.
    n.send_port_mapping_request(natpmp::Protocol::TCP, local_port, local_port, LEASE_SECS)
        .await
        .context("NAT-PMP port-mapping request failed")?;
    let external_port = match read_natpmp_response(&n, reply_timeout).await? {
        natpmp::Response::TCP(t) => t.public_port(),
        _ => anyhow::bail!("unexpected NAT-PMP response to mapping request"),
    };

    tracing::info!(%external_ip, external_port, "NAT-PMP port mapping established");
    Ok(MappedAddress {
        external_ip,
        external_port,
        protocol: Protocol::NatPmp,
    })
}

async fn read_natpmp_response(n: &NatPmp, reply_timeout: Duration) -> Result<natpmp::Response> {
    match tokio::time::timeout(reply_timeout, n.read_response_or_retry()).await {
        Ok(Ok(resp)) => Ok(resp),
        Ok(Err(e)) => Err(anyhow::anyhow!("NAT-PMP error: {e:?}")),
        Err(_) => anyhow::bail!("no NAT-PMP response within {reply_timeout:?}"),
    }
}

fn check_routable(external_ip: IpAddr) -> Result<()> {
    if is_unroutable_external(&external_ip) {
        anyhow::bail!(
            "router's external IP {external_ip} is itself private (carrier-grade/double NAT) - use a relay"
        );
    }
    Ok(())
}

/// Remove a mapping previously created by [`map_port`]. Best-effort: the
/// lease guarantees eventual cleanup even if this never runs.
pub async fn unmap_port(local_port: u16, protocol: Protocol) {
    let result = tokio::time::timeout(OVERALL_TIMEOUT, async {
        match protocol {
            Protocol::Upnp => unmap_upnp_via(igd_next::SearchOptions::default(), local_port).await,
            Protocol::NatPmp => {
                unmap_natpmp_via(&natpmp::new_tokio_natpmp().await?, local_port).await
            }
        }
    })
    .await;
    match result {
        Ok(Ok(())) => tracing::info!(local_port, ?protocol, "port mapping removed"),
        Ok(Err(e)) => tracing::debug!(error = %e, "unmap failed (lease will expire)"),
        Err(_) => tracing::debug!("unmap timed out (lease will expire)"),
    }
}

async fn unmap_upnp_via(options: igd_next::SearchOptions, local_port: u16) -> Result<()> {
    let gateway = igd_next::aio::tokio::search_gateway(options).await?;
    gateway
        .remove_port(igd_next::PortMappingProtocol::TCP, local_port)
        .await
        .map_err(anyhow::Error::from)
}

async fn unmap_natpmp_via(n: &NatPmp, local_port: u16) -> Result<()> {
    // A lease of 0 deletes the mapping (RFC 6886 §3.4).
    n.send_port_mapping_request(natpmp::Protocol::TCP, local_port, 0, 0)
        .await
        .map_err(|e| anyhow::anyhow!("NAT-PMP delete failed: {e:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapped_address_formats_ipv4_and_ipv6() {
        let v4 = MappedAddress {
            external_ip: "203.0.113.7".parse().unwrap(),
            external_port: 12345,
            protocol: Protocol::Upnp,
        };
        assert_eq!(v4.to_host_port(), "203.0.113.7:12345");

        let v6 = MappedAddress {
            external_ip: "2001:db8::7".parse().unwrap(),
            external_port: 12345,
            protocol: Protocol::NatPmp,
        };
        assert_eq!(v6.to_host_port(), "[2001:db8::7]:12345");
    }

    /// A mapping whose "external" address is itself private is behind a
    /// second NAT and useless to outside peers: it is refused with an error
    /// that says to use a relay, and a public one is accepted.
    #[test]
    fn a_mapping_behind_a_second_nat_is_refused() {
        let err = check_routable("100.64.3.4".parse().unwrap())
            .unwrap_err()
            .to_string();
        assert!(err.contains("use a relay"), "{err}");
        assert!(check_routable("192.168.0.1".parse().unwrap()).is_err());
        assert!(
            check_routable("203.0.113.9".parse().unwrap()).is_err(),
            "documentation range"
        );
        assert!(check_routable("8.8.4.4".parse().unwrap()).is_ok());
    }

    // ---- Loopback gateways -------------------------------------------------
    //
    // The protocol code above talks to a router. These stand one up on
    // loopback — an IGD (SSDP responder + HTTP/SOAP) and a NAT-PMP responder —
    // and record every request, so the tests can say what was *asked of the
    // router*, not only what came back. That is the property that matters on
    // a double NAT: refusing the mapping is not enough if it was already made.

    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, UdpSocket};

    type Log = Arc<Mutex<Vec<(String, String)>>>;

    const SERVICE: &str = "urn:schemas-upnp-org:service:WANIPConnection:1";

    fn scpd() -> String {
        let action = |name: &str, args: &[&str]| {
            let args: String = args
                .iter()
                .map(|a| format!("<argument><name>{a}</name><direction>in</direction></argument>"))
                .collect();
            format!("<action><name>{name}</name><argumentList>{args}</argumentList></action>")
        };
        format!(
            r#"<?xml version="1.0"?><scpd xmlns="urn:schemas-upnp-org:service-1-0"><actionList>{}{}{}</actionList></scpd>"#,
            action(
                "AddPortMapping",
                &["NewRemoteHost", "NewExternalPort", "NewProtocol", "NewInternalPort",
                  "NewInternalClient", "NewEnabled", "NewPortMappingDescription", "NewLeaseDuration"],
            ),
            action("DeletePortMapping", &["NewRemoteHost", "NewExternalPort", "NewProtocol"]),
            "<action><name>GetExternalIPAddress</name><argumentList><argument><name>NewExternalIPAddress</name><direction>out</direction></argument></argumentList></action>",
        )
    }

    fn device() -> String {
        format!(
            r#"<?xml version="1.0"?><root xmlns="urn:schemas-upnp-org:device-1-0"><device>
<deviceType>urn:schemas-upnp-org:device:InternetGatewayDevice:1</deviceType>
<deviceList><device><deviceType>urn:schemas-upnp-org:device:WANDevice:1</deviceType>
<deviceList><device><deviceType>urn:schemas-upnp-org:device:WANConnectionDevice:1</deviceType>
<serviceList><service><serviceType>{SERVICE}</serviceType><serviceId>urn:upnp-org:serviceId:WANIPConn1</serviceId>
<controlURL>/ctl/IPConn</controlURL><eventSubURL>/evt/IPConn</eventSubURL><SCPDURL>/WANIPCn.xml</SCPDURL>
</service></serviceList></device></deviceList></device></deviceList></device></root>"#
        )
    }

    fn soap(body: &str) -> String {
        format!(
            r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body>{body}</s:Body></s:Envelope>"#
        )
    }

    /// A UPnP gateway on loopback. `refuse` answers AddPortMapping with UPnP
    /// error 718 (ConflictInMappingEntry), as a router does when the port is
    /// already forwarded elsewhere.
    /// Search options aimed at `responder`. (`SearchOptions` is
    /// non-exhaustive and not `Clone`, so each search builds its own.)
    fn search_at(
        responder: SocketAddr,
        total: Duration,
        single: Duration,
    ) -> igd_next::SearchOptions {
        let mut o = igd_next::SearchOptions::default();
        o.bind_addr = "127.0.0.1:0".parse().unwrap();
        o.broadcast_address = responder;
        o.timeout = Some(total);
        o.single_search_timeout = Some(single);
        // igd-next refuses a loopback LOCATION unless it is listed: a real
        // gateway is never on loopback, so only a test needs this.
        o.allowed_gateway_ips = vec![IpAddr::V4(Ipv4Addr::LOCALHOST)];
        o
    }

    async fn fake_igd(external_ip: &'static str, refuse: bool) -> (SocketAddr, Log) {
        let log: Log = Arc::default();
        let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http_port = http.local_addr().unwrap().port();
        let requests = log.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = http.accept().await {
                let requests = requests.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let (head, body) = loop {
                        let n = sock.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        let text = String::from_utf8_lossy(&buf).to_string();
                        if let Some(end) = text.find("\r\n\r\n") {
                            let head = text[..end].to_string();
                            let len = head
                                .lines()
                                .find_map(|l| {
                                    let (k, v) = l.split_once(':')?;
                                    k.eq_ignore_ascii_case("content-length")
                                        .then(|| v.trim().parse::<usize>().ok())?
                                })
                                .unwrap_or(0);
                            if buf.len() >= end + 4 + len {
                                break (head, text[end + 4..end + 4 + len].to_string());
                            }
                        }
                    };
                    let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
                    let action = head
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case("soapaction").then(|| {
                                v.trim()
                                    .trim_matches('"')
                                    .rsplit('#')
                                    .next()
                                    .unwrap()
                                    .to_string()
                            })
                        })
                        .unwrap_or_default();
                    let reply = match (path.as_str(), action.as_str()) {
                        ("/rootDesc.xml", _) => device(),
                        ("/WANIPCn.xml", _) => scpd(),
                        (_, "AddPortMapping") if refuse => soap(
                            "<s:Fault><faultcode>s:Client</faultcode><faultstring>UPnPError</faultstring><detail><UPnPError xmlns=\"urn:schemas-upnp-org:control-1-0\"><errorCode>718</errorCode><errorDescription>ConflictInMappingEntry</errorDescription></UPnPError></detail></s:Fault>",
                        ),
                        (_, "GetExternalIPAddress") => soap(&format!(
                            "<u:GetExternalIPAddressResponse xmlns:u=\"{SERVICE}\"><NewExternalIPAddress>{external_ip}</NewExternalIPAddress></u:GetExternalIPAddressResponse>"
                        )),
                        (_, a) => soap(&format!("<u:{a}Response xmlns:u=\"{SERVICE}\"></u:{a}Response>")),
                    };
                    if !action.is_empty() {
                        requests.lock().unwrap().push((action, body));
                    }
                    let status = if reply.contains("<s:Fault>") {
                        "500 Internal Server Error"
                    } else {
                        "200 OK"
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                        reply.len()
                    );
                    let _ = sock.write_all(response.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });

        let ssdp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let search_addr = ssdp.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            while let Ok((_, from)) = ssdp.recv_from(&mut buf).await {
                let answer = format!(
                    "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=120\r\nST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\nUSN: uuid:fake::urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\nLOCATION: http://127.0.0.1:{http_port}/rootDesc.xml\r\n\r\n"
                );
                let _ = ssdp.send_to(answer.as_bytes(), from).await;
            }
        });

        (search_addr, log)
    }

    fn actions(log: &Log) -> Vec<String> {
        log.lock().unwrap().iter().map(|(a, _)| a.clone()).collect()
    }

    fn body_of(log: &Log, action: &str) -> String {
        log.lock()
            .unwrap()
            .iter()
            .find(|(a, _)| a == action)
            .map(|(_, b)| b.clone())
            .unwrap_or_else(|| panic!("no {action} request in {:?}", actions(log)))
    }

    #[tokio::test]
    async fn upnp_forwards_the_port_to_this_machine_and_takes_it_back() {
        let (gw, log) = fake_igd("8.8.4.4", false).await;
        let options = || search_at(gw, Duration::from_secs(5), Duration::from_secs(2));
        let local_ip: IpAddr = "10.0.0.7".parse().unwrap();

        let mapped = map_upnp_via(options(), local_ip, 40123)
            .await
            .expect("mapped");
        assert_eq!(mapped.external_ip, "8.8.4.4".parse::<IpAddr>().unwrap());
        assert_eq!(mapped.external_port, 40123);
        assert_eq!(mapped.protocol, Protocol::Upnp);
        assert_eq!(mapped.to_host_port(), "8.8.4.4:40123");

        // What the router was asked to do: forward this port, TCP, to this
        // machine, for our lease — not to some other host, not forever.
        let add = body_of(&log, "AddPortMapping");
        for field in [
            "<NewExternalPort>40123</NewExternalPort>",
            "<NewInternalPort>40123</NewInternalPort>",
            "<NewInternalClient>10.0.0.7</NewInternalClient>",
            "<NewProtocol>TCP</NewProtocol>",
            &format!("<NewLeaseDuration>{LEASE_SECS}</NewLeaseDuration>"),
        ] {
            assert!(add.contains(field), "AddPortMapping lacks {field}: {add}");
        }

        unmap_upnp_via(options(), 40123).await.expect("unmapped");
        let del = body_of(&log, "DeletePortMapping");
        assert!(
            del.contains("<NewExternalPort>40123</NewExternalPort>"),
            "{del}"
        );
        assert!(del.contains("<NewProtocol>TCP</NewProtocol>"), "{del}");
    }

    /// A router whose own "external" address is private sits behind another
    /// NAT, so a forward on it reaches nobody outside. The mapping used to be
    /// made first and the address checked after: the call failed correctly,
    /// and the router kept a forward to this machine for the full lease.
    #[tokio::test]
    async fn upnp_behind_a_second_nat_asks_the_router_for_nothing() {
        let (gw, log) = fake_igd("100.64.1.2", false).await;
        let options = search_at(gw, Duration::from_secs(5), Duration::from_secs(2));
        let err = map_upnp_via(options, "10.0.0.7".parse().unwrap(), 40124)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("use a relay"), "{err}");
        assert!(
            !actions(&log).iter().any(|a| a == "AddPortMapping"),
            "a port was forwarded on a router that cannot be reached from outside: {:?}",
            actions(&log)
        );
    }

    #[tokio::test]
    async fn upnp_a_router_that_refuses_the_mapping_says_so() {
        let (gw, _log) = fake_igd("8.8.4.4", true).await;
        let options = search_at(gw, Duration::from_secs(5), Duration::from_secs(2));
        let err = map_upnp_via(options, "10.0.0.7".parse().unwrap(), 40125)
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("router refused the port mapping"),
            "{err:#}"
        );
    }

    #[tokio::test]
    async fn upnp_no_gateway_on_the_network_is_an_error_not_a_hang() {
        // A responder that never answers.
        let silent = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let options = search_at(
            silent.local_addr().unwrap(),
            Duration::from_millis(300),
            Duration::from_millis(100),
        );
        let err = map_upnp_via(options, "10.0.0.7".parse().unwrap(), 40126)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("no UPnP gateway"), "{err:#}");
    }

    /// A NAT-PMP gateway on loopback. Answers the address request with
    /// `external`, and a mapping request with `public_port` (a gateway may
    /// grant a different port from the one asked for). `None` never answers.
    async fn fake_natpmp(
        external: Option<[u8; 4]>,
        public_port: u16,
    ) -> (NatPmp, Arc<Mutex<Vec<Vec<u8>>>>) {
        let gw = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = gw.local_addr().unwrap();
        let seen: Arc<Mutex<Vec<Vec<u8>>>> = Arc::default();
        let log = seen.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 64];
            while let Ok((n, from)) = gw.recv_from(&mut buf).await {
                let req = buf[..n].to_vec();
                log.lock().unwrap().push(req.clone());
                let Some(ip) = external else { continue };
                let epoch = 1234u32.to_be_bytes();
                let reply: Vec<u8> = match req[1] {
                    0 => [&[0, 128, 0, 0][..], &epoch, &ip].concat(),
                    op => [
                        &[0, 128 + op, 0, 0][..],
                        &epoch,
                        &req[4..6],
                        &public_port.to_be_bytes(),
                        &req[8..12],
                    ]
                    .concat(),
                };
                let _ = gw.send_to(&reply, from).await;
            }
        });
        let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sock.connect(addr).await.unwrap();
        (
            natpmp::new_natpmp_async_with(sock, Ipv4Addr::LOCALHOST),
            seen,
        )
    }

    #[tokio::test]
    async fn natpmp_reports_the_port_the_gateway_actually_opened() {
        let (n, seen) = fake_natpmp(Some([8, 8, 4, 4]), 50999).await;
        let mapped = map_natpmp_via(n, 40200, Duration::from_secs(2))
            .await
            .expect("mapped");
        // The invite must carry the port that was opened, not the one asked for.
        assert_eq!(mapped.to_host_port(), "8.8.4.4:50999");
        assert_eq!(mapped.protocol, Protocol::NatPmp);

        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen[0], vec![0, 0], "address request first");
        let map = &seen[1];
        assert_eq!(map[1], 2, "TCP");
        assert_eq!(u16::from_be_bytes([map[4], map[5]]), 40200, "private port");
        assert_eq!(
            u32::from_be_bytes([map[8], map[9], map[10], map[11]]),
            LEASE_SECS,
            "our lease, not forever"
        );
    }

    #[tokio::test]
    async fn natpmp_behind_a_second_nat_asks_the_gateway_for_nothing() {
        let (n, seen) = fake_natpmp(Some([192, 168, 1, 1]), 40201).await;
        let err = map_natpmp_via(n, 40201, Duration::from_secs(2))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("use a relay"), "{err}");
        let seen = seen.lock().unwrap().clone();
        assert!(
            seen.iter().all(|r| r[1] == 0),
            "a mapping was requested on a gateway that cannot be reached from outside: {seen:?}"
        );
    }

    #[tokio::test]
    async fn natpmp_a_silent_gateway_fails_within_the_reply_timeout() {
        let (n, _) = fake_natpmp(None, 0).await;
        let started = std::time::Instant::now();
        let err = map_natpmp_via(n, 40202, Duration::from_millis(200))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no NAT-PMP response"), "{err}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn natpmp_unmapping_asks_for_a_zero_lease() {
        let (n, seen) = fake_natpmp(Some([8, 8, 4, 4]), 40203).await;
        unmap_natpmp_via(&n, 40203).await.expect("sent");
        // The request is fire-and-forget; wait for the gateway to have it.
        for _ in 0..100 {
            if !seen.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let req = seen.lock().unwrap()[0].clone();
        assert_eq!(req[1], 2);
        assert_eq!(u16::from_be_bytes([req[4], req[5]]), 40203, "private port");
        assert_eq!(u16::from_be_bytes([req[6], req[7]]), 0, "public port 0");
        assert_eq!(
            u32::from_be_bytes([req[8], req[9], req[10], req[11]]),
            0,
            "lease 0 deletes"
        );
    }

    fn mapping(protocol: Protocol) -> Result<MappedAddress> {
        Ok(MappedAddress {
            external_ip: "8.8.4.4".parse().unwrap(),
            external_port: 1,
            protocol,
        })
    }

    #[tokio::test]
    async fn upnp_is_tried_first_and_natpmp_only_when_it_fails() {
        let budget = Duration::from_secs(5);
        let got = first_mapping(
            async { mapping(Protocol::Upnp) },
            async { panic!("NAT-PMP must not run once UPnP has mapped the port") },
            budget,
        )
        .await
        .unwrap();
        assert_eq!(got.protocol, Protocol::Upnp);

        let got = first_mapping(
            async { Err(anyhow::anyhow!("no gateway")) },
            async { mapping(Protocol::NatPmp) },
            budget,
        )
        .await
        .unwrap();
        assert_eq!(got.protocol, Protocol::NatPmp);

        // Both failing names both, or "it didn't work" is all a user gets.
        let err = first_mapping(
            async { Err(anyhow::anyhow!("upnp said no")) },
            async { Err(anyhow::anyhow!("natpmp said no")) },
            budget,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("upnp said no") && err.contains("natpmp said no"),
            "{err}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_gateway_that_never_answers_costs_one_budget_not_forever() {
        let budget = Duration::from_secs(15);
        let got = first_mapping(
            std::future::pending(),
            async { mapping(Protocol::NatPmp) },
            budget,
        )
        .await
        .unwrap();
        assert_eq!(
            got.protocol,
            Protocol::NatPmp,
            "a silent UPnP gateway must not block the fallback"
        );

        let err = first_mapping(std::future::pending(), std::future::pending(), budget)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("did not answer within 15s"), "{err}");
    }

    #[test]
    fn detects_unroutable_external_addresses() {
        // Public: routable.
        assert!(!is_unroutable_external(&"8.8.8.8".parse().unwrap()));
        assert!(!is_unroutable_external(&"1.1.1.1".parse().unwrap()));
        // RFC1918 private (double-NAT).
        assert!(is_unroutable_external(&"192.168.1.1".parse().unwrap()));
        assert!(is_unroutable_external(&"10.0.0.1".parse().unwrap()));
        assert!(is_unroutable_external(&"172.16.5.4".parse().unwrap()));
        // CGNAT shared space (RFC 6598).
        assert!(is_unroutable_external(&"100.64.0.1".parse().unwrap()));
        assert!(is_unroutable_external(&"100.127.255.1".parse().unwrap()));
        // Just outside CGNAT range: routable.
        assert!(!is_unroutable_external(&"100.63.0.1".parse().unwrap()));
        assert!(!is_unroutable_external(&"100.128.0.1".parse().unwrap()));
    }
}
