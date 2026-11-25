//! mDNS/Bonjour LAN discovery: advertise this instance while hosting and list
//! peers on the same network. It supplies addresses, never trust (TOFU still runs).

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
    /// Identity fingerprint, if an older peer advertises one. Unused, and this
    /// build does not advertise it (see [`Discovery::register`]).
    pub fingerprint: Option<String>,
    /// Full mDNS service name (`<instance>._p2p-messenger._tcp.local.`): the only
    /// thing a `ServiceRemoved` carries, so removal matches it exactly. (`name` is
    /// the hostname, and a substring test would evict "laptop" with "laptop-alice".)
    pub fullname: String,
}

/// Manages mDNS service registration and peer discovery.
pub struct Discovery {
    daemon: ServiceDaemon,
    receiver: Receiver<ServiceEvent>,
    registered_service_fullname: Option<String>,
}

/// The TXT record advertised with the service. Deliberately empty: it is
/// broadcast in the clear to the whole LAN, so a field belongs here only if
/// something reads it and strangers may see it. The fingerprint met neither test.
fn service_txt_properties() -> HashMap<String, String> {
    HashMap::new()
}

/// Most peers the list will hold. mDNS is unauthenticated LAN input resolved
/// under a mutex the UI reads, so a flood of announcements must not grow it (or
/// the linear scans over it) without bound. Far above any real network.
const MAX_DISCOVERED_PEERS: usize = 256;

/// Most addresses recorded for one service; a real dual-homed peer has two or three.
const MAX_ADDRESSES_PER_PEER: usize = 8;

/// Apply one mDNS browse event to the peer list: a resolved service becomes
/// one entry per advertised address (at most `MAX_ADDRESSES_PER_PEER`),
/// carrying the `fingerprint` it advertises and its host name without the
/// trailing dot; a removed one goes, matched by its exact full name.
fn apply_event(event: ServiceEvent, peers: &mut Vec<DiscoveredPeer>) {
    match event {
        ServiceEvent::ServiceResolved(info) => {
            let fullname = info.get_fullname().to_string();
            let fingerprint = info
                .get_properties()
                .get("fingerprint")
                .map(|p| p.val_str().to_string());
            let name = info.get_hostname().trim_end_matches('.').to_string();
            let port = info.get_port();
            let mut addresses: Vec<_> = info
                .get_addresses()
                .iter()
                .map(ToString::to_string)
                .collect();
            // A set underneath: order it, so which addresses survive the cap
            // does not depend on hashing.
            addresses.sort();
            let resolved: Vec<DiscoveredPeer> = addresses
                .into_iter()
                .take(MAX_ADDRESSES_PER_PEER)
                .map(|address| DiscoveredPeer {
                    name: name.clone(),
                    address,
                    port,
                    fingerprint: fingerprint.clone(),
                    fullname: fullname.clone(),
                })
                .collect();
            if resolved.is_empty() {
                return;
            }
            for peer in &resolved {
                tracing::info!(
                    name = %peer.name,
                    address = %peer.address,
                    port = %peer.port,
                    "Discovered peer via mDNS"
                );
            }
            merge_resolved(peers, &fullname, resolved);
        }
        ServiceEvent::ServiceRemoved(_, fullname) => {
            tracing::info!(fullname = %fullname, "Peer removed from mDNS");
            // Exact match, so "laptop-alice" leaving keeps "laptop".
            peers.retain(|p| p.fullname != fullname);
        }
        _ => {}
    }
}

/// Fold a freshly resolved service into the peer list. A re-resolve *replaces*
/// the service's previous entries (a peer that moved, or is dual-homed, must be
/// listed where it is reachable). The retain runs *before* the cap check, so the
/// slots a service gives up are free for it again and a LAN flood cannot make a
/// re-announcing real peer vanish.
fn merge_resolved(peers: &mut Vec<DiscoveredPeer>, fullname: &str, resolved: Vec<DiscoveredPeer>) {
    peers.retain(|p| p.fullname != fullname);

    for peer in resolved {
        if peers.len() >= MAX_DISCOVERED_PEERS {
            tracing::warn!(
                cap = MAX_DISCOVERED_PEERS,
                "mDNS peer list is full; ignoring further advertisements"
            );
            break;
        }
        // A second service at an endpoint already listed is not a second peer.
        if peers
            .iter()
            .any(|p| p.address == peer.address && p.port == peer.port)
        {
            continue;
        }
        peers.push(peer);
    }
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

    /// Advertise `name` (the display name) on `port`. The identity fingerprint
    /// is deliberately *not* advertised: nothing read it, and broadcasting it told
    /// the whole LAN which long-term identity sat at which address. Peers that
    /// still advertise one are parsed without complaint (see `poll`).
    pub fn register(&mut self, name: &str, port: u16) -> anyhow::Result<()> {
        let properties = service_txt_properties();

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

    /// Apply the discovery events that arrived since the last call to
    /// `discovered_peers`. Non-blocking.
    pub fn poll(&self, discovered_peers: &Arc<Mutex<Vec<DiscoveredPeer>>>) {
        while let Ok(event) = self.receiver.try_recv() {
            if let Ok(mut peers) = discovered_peers.lock() {
                apply_event(event, &mut peers);
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

    /// A resolved service event, as the daemon would deliver one.
    fn resolved(instance: &str, host: &str, addrs: &str, fp: Option<&str>) -> ServiceEvent {
        let props: Vec<(&str, &str)> = fp.map(|f| vec![("fingerprint", f)]).unwrap_or_default();
        let info =
            ServiceInfo::new(SERVICE_TYPE, instance, host, addrs, 12345, &props[..]).unwrap();
        ServiceEvent::ServiceResolved(Box::new(info.as_resolved_service()))
    }

    /// The daemon's events, applied the way `poll` applies them: one entry per
    /// advertised address with the advertised fingerprint and the host name
    /// without its trailing dot, a cap on addresses per peer, nothing for a
    /// service with no address, and removal by the exact full name only.
    #[test]
    fn browse_events_become_peers_and_leave_by_exact_name() {
        let mut peers = Vec::new();
        let fp = "ab".repeat(32);
        apply_event(
            resolved(
                "laptop",
                "laptop.local.",
                "192.168.1.5,192.168.1.6",
                Some(&fp),
            ),
            &mut peers,
        );
        apply_event(
            resolved("laptop-alice", "alice.local.", "192.168.1.9", None),
            &mut peers,
        );
        assert_eq!(peers.len(), 3);
        let laptop: Vec<_> = peers.iter().filter(|p| p.name == "laptop.local").collect();
        assert_eq!(
            laptop
                .iter()
                .map(|p| p.address.as_str())
                .collect::<Vec<_>>(),
            ["192.168.1.5", "192.168.1.6"]
        );
        assert!(laptop
            .iter()
            .all(|p| p.fingerprint.as_deref() == Some(fp.as_str()) && p.port == 12345));
        assert!(peers
            .iter()
            .any(|p| p.name == "alice.local" && p.fingerprint.is_none()));

        // A dual-homed host advertising more than the cap keeps only the cap.
        let many: Vec<String> = (1..=MAX_ADDRESSES_PER_PEER + 3)
            .map(|i| format!("10.0.0.{i}"))
            .collect();
        apply_event(
            resolved("busy", "busy.local.", &many.join(","), None),
            &mut peers,
        );
        assert_eq!(
            peers.iter().filter(|p| p.name == "busy.local").count(),
            MAX_ADDRESSES_PER_PEER
        );
        // No address, nothing to dial: not listed.
        apply_event(resolved("ghost", "ghost.local.", "", None), &mut peers);
        assert!(!peers.iter().any(|p| p.name == "ghost.local"));

        // "laptop" leaving must not take "laptop-alice" with it.
        let laptop_fullname = format!("laptop.{SERVICE_TYPE}");
        apply_event(
            ServiceEvent::ServiceRemoved(SERVICE_TYPE.into(), laptop_fullname),
            &mut peers,
        );
        assert!(!peers.iter().any(|p| p.name == "laptop.local"));
        assert!(
            peers.iter().any(|p| p.name == "alice.local"),
            "a prefix match removed another peer"
        );
    }

    /// The removal rule, isolated from the daemon: a departing peer takes itself
    /// out of the list and nothing else (a substring match removed too many).
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

    /// The TXT record must not carry identity: trust is TOFU's job, and a
    /// broadcast fingerprint links a long-term identity to a machine.
    #[test]
    fn the_advertised_txt_record_carries_no_identity() {
        let props = service_txt_properties();
        assert!(
            props.is_empty(),
            "nothing should be advertised in the clear, found {props:?}"
        );
        assert!(!props.contains_key("fingerprint"));
    }

    fn peer(fullname: &str, address: &str) -> DiscoveredPeer {
        DiscoveredPeer {
            name: "laptop".to_string(),
            address: address.to_string(),
            port: 12345,
            fingerprint: None,
            fullname: fullname.to_string(),
        }
    }

    /// A peer that changes address (DHCP renewal, Wi-Fi to Ethernet) is listed at
    /// the new one, not left at the stale one.
    #[test]
    fn a_peer_that_moved_is_listed_at_its_new_address() {
        let mut peers = vec![peer("laptop._p2p-messenger._tcp.local.", "192.168.1.10")];
        merge_resolved(
            &mut peers,
            "laptop._p2p-messenger._tcp.local.",
            vec![peer("laptop._p2p-messenger._tcp.local.", "192.168.1.42")],
        );

        assert_eq!(peers.len(), 1, "the stale entry must not survive");
        assert_eq!(peers[0].address, "192.168.1.42");
    }

    /// A dual-homed peer is listed on every interface it advertises, not only
    /// on whichever one resolved first.
    #[test]
    fn a_dual_homed_peer_is_listed_on_every_address() {
        let mut peers = Vec::new();
        merge_resolved(
            &mut peers,
            "laptop._p2p-messenger._tcp.local.",
            vec![
                peer("laptop._p2p-messenger._tcp.local.", "192.168.1.42"),
                peer("laptop._p2p-messenger._tcp.local.", "fe80::1"),
            ],
        );

        let addresses: Vec<&str> = peers.iter().map(|p| p.address.as_str()).collect();
        assert_eq!(addresses, ["192.168.1.42", "fe80::1"]);
    }

    /// The list is bounded: mDNS is unauthenticated LAN input.
    #[test]
    fn the_peer_list_is_capped() {
        let mut peers = Vec::new();
        for i in 0..(MAX_DISCOVERED_PEERS + 50) {
            let fullname = format!("flood-{i}._p2p-messenger._tcp.local.");
            merge_resolved(
                &mut peers,
                &fullname,
                vec![peer(&fullname, &format!("10.0.{}.{}", i / 256, i % 256))],
            );
        }
        assert_eq!(peers.len(), MAX_DISCOVERED_PEERS);
    }

    /// A flood must not evict a real peer at its next re-announcement (why the
    /// retain runs before the cap check).
    #[test]
    fn a_full_list_does_not_swallow_a_re_announcing_peer() {
        let mut peers = Vec::new();
        let mine = "mine._p2p-messenger._tcp.local.";
        merge_resolved(&mut peers, mine, vec![peer(mine, "192.168.1.42")]);

        for i in 0..(MAX_DISCOVERED_PEERS + 50) {
            let fullname = format!("flood-{i}._p2p-messenger._tcp.local.");
            merge_resolved(
                &mut peers,
                &fullname,
                vec![peer(&fullname, &format!("10.0.{}.{}", i / 256, i % 256))],
            );
        }
        assert_eq!(peers.len(), MAX_DISCOVERED_PEERS, "the cap still holds");

        // The real peer re-announces, as mDNS makes it do periodically.
        merge_resolved(&mut peers, mine, vec![peer(mine, "192.168.1.42")]);
        assert!(
            peers.iter().any(|p| p.fullname == mine),
            "a re-announcing peer vanished from a full list"
        );
    }

    /// Two services advertising one endpoint are one peer.
    #[test]
    fn a_duplicate_endpoint_is_not_a_second_peer() {
        let mut peers = Vec::new();
        merge_resolved(
            &mut peers,
            "a._p2p-messenger._tcp.local.",
            vec![peer("a._p2p-messenger._tcp.local.", "192.168.1.42")],
        );
        merge_resolved(
            &mut peers,
            "b._p2p-messenger._tcp.local.",
            vec![peer("b._p2p-messenger._tcp.local.", "192.168.1.42")],
        );
        assert_eq!(peers.len(), 1);
    }

    /// Register, poll, unregister when an mDNS daemon can start (sandboxed CI
    /// without multicast may have none; that is not a failure).
    #[test]
    fn discovery_lifecycle_does_not_block_the_caller() {
        let Ok(mut discovery) = Discovery::new() else {
            return;
        };
        let registered = discovery.register("test-peer", 12345).is_ok();
        let peers = Arc::new(Mutex::new(Vec::new()));
        let started = std::time::Instant::now();
        discovery.poll(&peers);
        assert!(
            started.elapsed() < std::time::Duration::from_millis(500),
            "poll blocked for {:?}",
            started.elapsed()
        );
        if registered {
            assert!(
                discovery.unregister().is_ok(),
                "a registration could not be undone"
            );
        }
    }
}
