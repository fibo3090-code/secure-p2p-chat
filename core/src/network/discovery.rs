//! mDNS/Bonjour local peer discovery.
//!
//! This module provides functionality to:
//! - Register this instance on the local network when hosting.
//! - Discover other peers on the same network.

use mdns_sd::{Receiver, ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Service type for mDNS discovery. Follows Zeroconf naming conventions.
const SERVICE_TYPE: &str = "_p2p-messenger._tcp.local.";

/// Information about a discovered peer on the local network.
#[derive(Debug, Clone)]
pub struct DiscoveredPeer {
    /// Human-readable name of the peer.
    pub name: String,
    /// The IP address of the peer.
    pub address: String,
    /// The port the peer is listening on.
    pub port: u16,
    /// The fingerprint of the peer's identity key, when the peer advertises one.
    ///
    /// Nothing consumes this and this build does not advertise it — see
    /// [`Discovery::register`]. Kept so a peer on an older version is still
    /// parsed cleanly.
    pub fingerprint: Option<String>,
    /// The full mDNS service name (`<instance>._p2p-messenger._tcp.local.`).
    ///
    /// This is the identity mDNS itself uses, and the only thing a
    /// `ServiceRemoved` event carries. Removal used to match
    /// `fullname.contains(peer.name)`, which was wrong twice over: `name` holds
    /// the *hostname* while the fullname is built from the *instance* name, so
    /// it frequently matched nothing and the peer was never removed; and being a
    /// substring test, when it did match it could take "laptop" out along with
    /// "laptop-alice".
    pub fullname: String,
}

/// Manages mDNS service registration and peer discovery.
pub struct Discovery {
    daemon: ServiceDaemon,
    receiver: Receiver<ServiceEvent>,
    registered_service_fullname: Option<String>,
}

/// The TXT record advertised alongside the service.
///
/// Deliberately empty. The service type, instance name, address and port are all
/// discovery needs, and anything added here is broadcast in the clear to every
/// device on the network — so a field belongs here only if something actually
/// reads it and it is safe for strangers to see. The identity fingerprint met
/// neither test.
fn service_txt_properties() -> HashMap<String, String> {
    HashMap::new()
}

impl Discovery {
    /// Create a new Discovery instance.
    pub fn new() -> anyhow::Result<Self> {
        let daemon = ServiceDaemon::new()?;
        let receiver = daemon.browse(SERVICE_TYPE)?;
        Ok(Self {
            daemon,
            receiver,
            registered_service_fullname: None,
        })
    }

    /// Register this instance on the network.
    ///
    /// # Arguments
    /// * `name` - The user's display name.
    /// * `port` - The port the app is listening on.
    ///
    /// ## What is deliberately *not* advertised
    ///
    /// The identity fingerprint used to go in the TXT record, and nothing ever
    /// read it: the UI offers a nearby peer as an address to dial and nothing
    /// more, because discovery supplies reachability and never trust — TOFU
    /// still runs on connect. Broadcasting it therefore bought nothing and told
    /// every device on the network which long-term identity was sitting at which
    /// address, which is exactly the linkage someone on a café or office LAN
    /// would want.
    ///
    /// Peers that still advertise one are parsed without complaint (see `poll`),
    /// so a mixed-version network keeps working.
    pub fn register(&mut self, name: &str, port: u16) -> anyhow::Result<()> {
        let properties = service_txt_properties();

        // Construct the service info
        let host_ipv4 = crate::util::primary_local_ipv4().unwrap_or_else(|| "0.0.0.0".to_string());

        let service_info = ServiceInfo::new(
            SERVICE_TYPE,
            name,
            &format!("{}.local.", hostname::get()?.to_string_lossy()),
            host_ipv4.as_str(),
            port,
            properties,
        )?;

        let fullname = service_info.get_fullname().to_string();

        self.daemon.register(service_info)?;
        self.registered_service_fullname = Some(fullname.clone());

        tracing::info!(
            name = %name,
            port = %port,
            "Registered mDNS service"
        );
        Ok(())
    }

    /// Unregister the service when hosting stops.
    pub fn unregister(&mut self) -> anyhow::Result<()> {
        if let Some(fullname) = self.registered_service_fullname.take() {
            self.daemon.unregister(&fullname)?;
            tracing::info!("Unregistered mDNS service");
        }
        Ok(())
    }

    /// Poll for newly discovered or removed peers.
    /// Returns a list of currently known peers.
    ///
    /// This is non-blocking. It processes events that have arrived since the last call.
    pub fn poll(&self, discovered_peers: &Arc<Mutex<Vec<DiscoveredPeer>>>) {
        // Process all pending events
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                ServiceEvent::ServiceResolved(info) => {
                    let fullname = info.get_fullname().to_string();
                    let addresses = info.get_addresses();
                    let port = info.get_port();
                    let fingerprint = info
                        .get_properties()
                        .get("fingerprint")
                        .map(|p| p.val_str().to_string());

                    if let Some(addr) = addresses.iter().next() {
                        let peer = DiscoveredPeer {
                            name: info.get_hostname().trim_end_matches('.').to_string(),
                            address: addr.to_string(),
                            port,
                            fingerprint,
                            fullname: fullname.clone(),
                        };

                        tracing::info!(
                            name = %peer.name,
                            address = %peer.address,
                            port = %peer.port,
                            "Discovered peer via mDNS"
                        );

                        if let Ok(mut peers) = discovered_peers.lock() {
                            // Avoid duplicates
                            // Keyed on the service name as well as the endpoint:
                            // one host can advertise twice, and re-resolving an
                            // existing service must not duplicate it.
                            if !peers.iter().any(|p| {
                                p.fullname == peer.fullname
                                    || (p.address == peer.address && p.port == peer.port)
                            }) {
                                peers.push(peer);
                            }
                        }
                    }
                }
                ServiceEvent::ServiceRemoved(_, fullname) => {
                    tracing::info!(fullname = %fullname, "Peer removed from mDNS");
                    if let Ok(mut peers) = discovered_peers.lock() {
                        // Exact match on the service name mDNS gave us, so a
                        // departing "laptop-alice" cannot also evict "laptop".
                        peers.retain(|p| p.fullname != fullname);
                    }
                }
                _ => {}
            }
        }
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        let _ = self.unregister();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovered_peer_construction_and_clone() {
        let peer = DiscoveredPeer {
            name: "Laptop".to_string(),
            address: "192.168.1.20".to_string(),
            port: 12345,
            fingerprint: Some("AB".repeat(32)),
            fullname: "Laptop._p2p-messenger._tcp.local.".to_string(),
        };
        let cloned = peer.clone();
        assert_eq!(cloned.name, "Laptop");
        assert_eq!(cloned.address, "192.168.1.20");
        assert_eq!(cloned.port, 12345);
        assert_eq!(
            cloned.fingerprint.as_deref(),
            Some("AB".repeat(32).as_str())
        );
        // Debug must render without panicking.
        assert!(format!("{:?}", peer).contains("Laptop"));
    }

    /// The removal rule, isolated from the daemon: a departing peer must take
    /// itself out of the list and nothing else.
    ///
    /// This is the shape of the bug it replaces. Matching
    /// `fullname.contains(peer.name)` compared the event's *instance* name
    /// against the peer's *hostname*, so it usually removed nobody — and when
    /// the two happened to share a prefix, it removed too many.
    #[test]
    fn removing_one_peer_leaves_similarly_named_peers_alone() {
        let mk = |instance: &str, host: &str, last: u8| DiscoveredPeer {
            name: host.to_string(),
            address: format!("192.168.1.{last}"),
            port: 12345,
            fingerprint: None,
            fullname: format!("{instance}._p2p-messenger._tcp.local."),
        };

        let mut peers = vec![
            mk("laptop", "laptop", 10),
            mk("laptop-alice", "laptop-alice", 11),
            mk("desktop", "desktop", 12),
        ];

        let departing = "laptop-alice._p2p-messenger._tcp.local.".to_string();
        peers.retain(|p| p.fullname != departing);

        let left: Vec<&str> = peers.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            left,
            vec!["laptop", "desktop"],
            "only the peer that actually left may be removed"
        );
    }

    /// The TXT record must not carry identity. Discovery answers "something is
    /// reachable at this address"; deciding whether to trust it is TOFU's job,
    /// and broadcasting the fingerprint linked a long-term identity to a machine
    /// for every device on the network.
    #[test]
    fn the_advertised_txt_record_carries_no_identity() {
        let props = service_txt_properties();
        assert!(
            props.is_empty(),
            "nothing should be advertised in the clear, found {props:?}"
        );
        assert!(!props.contains_key("fingerprint"));
    }

    /// Exercises the register → poll → unregister lifecycle when an mDNS daemon is
    /// available. Sandboxed CI without multicast may be unable to start the
    /// daemon; in that case the constructor path is still exercised and the test
    /// does not fail on an environment limitation. `poll` is non-blocking.
    #[test]
    fn discovery_lifecycle_is_non_panicking_when_available() {
        if let Ok(mut discovery) = Discovery::new() {
            let _ = discovery.register("test-peer", 12345);
            let peers = Arc::new(Mutex::new(Vec::new()));
            discovery.poll(&peers); // must return immediately
            let _ = discovery.unregister();
            // Dropping also unregisters; must not panic.
        }
    }
}
