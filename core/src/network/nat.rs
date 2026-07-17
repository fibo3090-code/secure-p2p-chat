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
    let upnp_err = match tokio::time::timeout(OVERALL_TIMEOUT, map_upnp(local_port)).await {
        Ok(Ok(mapping)) => return Ok(mapping),
        Ok(Err(e)) => e,
        Err(_) => anyhow::anyhow!("UPnP gateway did not answer within 15s"),
    };
    tracing::debug!(error = %upnp_err, "UPnP failed, trying NAT-PMP");

    match tokio::time::timeout(OVERALL_TIMEOUT, map_natpmp(local_port)).await {
        Ok(Ok(mapping)) => Ok(mapping),
        Ok(Err(natpmp_err)) => Err(anyhow::anyhow!(
            "UPnP failed ({upnp_err}); NAT-PMP failed ({natpmp_err})"
        )),
        Err(_) => Err(anyhow::anyhow!(
            "UPnP failed ({upnp_err}); NAT-PMP gateway did not answer within 15s"
        )),
    }
}

async fn map_upnp(local_port: u16) -> Result<MappedAddress> {
    let gateway = igd_next::aio::tokio::search_gateway(igd_next::SearchOptions::default())
        .await
        .context("no UPnP gateway found on this network")?;

    let local_ip: IpAddr = crate::util::primary_local_ipv4()
        .context("could not determine local IP address")?
        .parse()
        .context("local IP did not parse")?;
    let local_addr = SocketAddr::new(local_ip, local_port);

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

    let external_ip = gateway
        .get_external_ip()
        .await
        .context("router did not report an external IP")?;

    check_routable(external_ip)?;
    tracing::info!(%external_ip, local_port, "UPnP port mapping established");
    Ok(MappedAddress {
        external_ip,
        external_port: local_port,
        protocol: Protocol::Upnp,
    })
}

async fn map_natpmp(local_port: u16) -> Result<MappedAddress> {
    let mut n = natpmp::new_tokio_natpmp()
        .await
        .context("could not open a NAT-PMP socket")?;

    // Ask for the external IP.
    n.send_public_address_request()
        .await
        .context("NAT-PMP external-address request failed")?;
    let external_ip = match read_natpmp_response(&n).await? {
        natpmp::Response::Gateway(g) => IpAddr::V4(*g.public_address()),
        _ => anyhow::bail!("unexpected NAT-PMP response to address request"),
    };

    // Map the port (TCP), same external port, our lease.
    n.send_port_mapping_request(natpmp::Protocol::TCP, local_port, local_port, LEASE_SECS)
        .await
        .context("NAT-PMP port-mapping request failed")?;
    let external_port = match read_natpmp_response(&n).await? {
        natpmp::Response::TCP(t) => t.public_port(),
        _ => anyhow::bail!("unexpected NAT-PMP response to mapping request"),
    };

    check_routable(external_ip)?;
    tracing::info!(%external_ip, external_port, "NAT-PMP port mapping established");
    Ok(MappedAddress {
        external_ip,
        external_port,
        protocol: Protocol::NatPmp,
    })
}

async fn read_natpmp_response(
    n: &natpmp::NatpmpAsync<tokio::net::UdpSocket>,
) -> Result<natpmp::Response> {
    // The client returns TryAgain until the datagram arrives.
    for _ in 0..20 {
        match n.read_response_or_retry().await {
            Ok(resp) => return Ok(resp),
            Err(natpmp::Error::NATPMP_TRYAGAIN) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => return Err(anyhow::anyhow!("NAT-PMP error: {e:?}")),
        }
    }
    anyhow::bail!("no NAT-PMP response")
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
            Protocol::Upnp => {
                let gateway =
                    igd_next::aio::tokio::search_gateway(igd_next::SearchOptions::default())
                        .await?;
                gateway
                    .remove_port(igd_next::PortMappingProtocol::TCP, local_port)
                    .await
                    .map_err(anyhow::Error::from)
            }
            Protocol::NatPmp => {
                let n = natpmp::new_tokio_natpmp().await?;
                // A lease of 0 deletes the mapping (RFC 6886 §3.4).
                n.send_port_mapping_request(natpmp::Protocol::TCP, local_port, 0, 0)
                    .await
                    .map_err(|e| anyhow::anyhow!("NAT-PMP delete failed: {e:?}"))
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
