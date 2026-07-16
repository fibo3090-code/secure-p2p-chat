//! Best-effort UPnP (IGD) port mapping for NAT traversal.
//!
//! When hosting, the app can ask the local router to forward the listening
//! port and report the router's external IP, so the generated invite carries
//! an address that works from outside the LAN. This is strictly best-effort:
//! no gateway, a gateway with UPnP disabled, or a carrier-grade NAT all make
//! it fail, and the caller is expected to fall back to relay-assisted
//! connectivity. Nothing here touches the wire protocol.

use anyhow::{Context, Result};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

/// Requested lease for the mapping. Routers reclaim the port after this even
/// if the app dies without unmapping; long enough for a hosting session.
const LEASE_SECS: u32 = 24 * 60 * 60;

/// Hard cap on the whole discovery + mapping exchange so a hosting toast is
/// never held hostage by a silent network.
const OVERALL_TIMEOUT: Duration = Duration::from_secs(15);

/// A successfully established router port mapping.
#[derive(Debug, Clone)]
pub struct MappedAddress {
    pub external_ip: IpAddr,
    pub external_port: u16,
}

impl MappedAddress {
    pub fn to_host_port(&self) -> String {
        crate::util::format_host_port(&self.external_ip.to_string(), self.external_port)
    }
}

/// Discover the gateway, map `local_port` (TCP, same external port), and
/// return the external address peers can connect to.
pub async fn map_port(local_port: u16) -> Result<MappedAddress> {
    tokio::time::timeout(OVERALL_TIMEOUT, map_port_inner(local_port))
        .await
        .map_err(|_| anyhow::anyhow!("UPnP gateway did not answer within 15s"))?
}

async fn map_port_inner(local_port: u16) -> Result<MappedAddress> {
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

    tracing::info!(%external_ip, local_port, "UPnP port mapping established");
    Ok(MappedAddress {
        external_ip,
        external_port: local_port,
    })
}

/// Remove a mapping previously created by [`map_port`]. Best-effort: the
/// lease guarantees eventual cleanup even if this never runs.
pub async fn unmap_port(local_port: u16) {
    let removed = async {
        let gateway =
            igd_next::aio::tokio::search_gateway(igd_next::SearchOptions::default()).await?;
        gateway
            .remove_port(igd_next::PortMappingProtocol::TCP, local_port)
            .await
            .map_err(anyhow::Error::from)
    };
    match tokio::time::timeout(OVERALL_TIMEOUT, removed).await {
        Ok(Ok(())) => tracing::info!(local_port, "UPnP port mapping removed"),
        Ok(Err(e)) => tracing::debug!(error = %e, "UPnP unmap failed (lease will expire)"),
        Err(_) => tracing::debug!("UPnP unmap timed out (lease will expire)"),
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
        };
        assert_eq!(v4.to_host_port(), "203.0.113.7:12345");

        let v6 = MappedAddress {
            external_ip: "2001:db8::7".parse().unwrap(),
            external_port: 12345,
        };
        assert_eq!(v6.to_host_port(), "[2001:db8::7]:12345");
    }
}
