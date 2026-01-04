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
    /// The fingerprint of the peer's identity key.
    pub fingerprint: Option<String>,
}

/// Manages mDNS service registration and peer discovery.
pub struct Discovery {
    daemon: ServiceDaemon,
    receiver: Receiver<ServiceEvent>,
    registered_service_fullname: Option<String>,
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
    /// * `fingerprint` - The user's identity fingerprint.
    pub fn register(&mut self, name: &str, port: u16, fingerprint: &str) -> anyhow::Result<()> {
        // Construct properties (TXT record)
        let mut properties = HashMap::new();
        properties.insert("fingerprint".to_string(), fingerprint.to_string());

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
                    let _fullname = info.get_fullname().to_string();
                    let addresses = info.get_addresses();
                    let port = info.get_port();
                    let fingerprint = info.get_properties().get("fingerprint").map(|p| p.val_str().to_string());

                    if let Some(addr) = addresses.iter().next() {
                        let peer = DiscoveredPeer {
                            name: info.get_hostname().trim_end_matches('.').to_string(),
                            address: addr.to_string(),
                            port,
                            fingerprint,
                        };
                        
                        tracing::info!(
                            name = %peer.name,
                            address = %peer.address,
                            port = %peer.port,
                            "Discovered peer via mDNS"
                        );
                        
                        if let Ok(mut peers) = discovered_peers.lock() {
                            // Avoid duplicates
                            if !peers.iter().any(|p| p.address == peer.address && p.port == peer.port) {
                                peers.push(peer);
                            }
                        }
                    }
                }
                ServiceEvent::ServiceRemoved(_, fullname) => {
                    tracing::info!(fullname = %fullname, "Peer removed from mDNS");
                    if let Ok(mut peers) = discovered_peers.lock() {
                        peers.retain(|p| !fullname.contains(&p.name));
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
