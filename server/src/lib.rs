//! Party/Community server, as a library.
//!
//! `main.rs` only parses arguments, loads state and identity, binds, and hands
//! everything to [`run_accept_loop`] — so a test can stand the real server up
//! in process (`client/tests/party_e2e.rs`).

pub mod connection;
pub mod dispatch;
pub mod hub;
pub mod identity;
pub mod state;

#[cfg(test)]
mod adversarial_tests;

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use messenger_core::network::ratelimit::{limiter_key, RateLimiter};
use rsa::PrivateKey as RsaPrivateKey;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, Semaphore};

pub use connection::ConnectionLimits;
use hub::Hub;
use state::PartyState;

/// Default ceiling on connections served at once: below the common 1024
/// `RLIMIT_NOFILE`, leaving room for the database, blobs and listener (the
/// per-address *rate* limit alone does not bound concurrency).
pub const DEFAULT_MAX_CONNECTIONS: usize = 512;

/// Default ceiling on connections from one address (IPv4 address or IPv6 /64,
/// see [`limiter_key`]) at once: enough for a household behind one NAT.
pub const DEFAULT_MAX_CONNECTIONS_PER_ADDRESS: usize = 32;

/// Everything the accept loop bounds.
#[derive(Debug, Clone)]
pub struct ServerLimits {
    /// Timeouts applied to each connection.
    pub connection: ConnectionLimits,
    /// Connections served at once; more are refused at accept.
    pub max_connections: usize,
    /// Connections from one address at once; more are refused at accept.
    pub max_connections_per_address: usize,
}

impl Default for ServerLimits {
    fn default() -> Self {
        Self {
            connection: ConnectionLimits::default(),
            max_connections: DEFAULT_MAX_CONNECTIONS,
            max_connections_per_address: DEFAULT_MAX_CONNECTIONS_PER_ADDRESS,
        }
    }
}

/// Live connections per address, for the per-address concurrency cap.
#[derive(Default)]
struct AddressSlots {
    counts: std::sync::Mutex<HashMap<IpAddr, usize>>,
}

/// One address's claim on a slot, released when the connection's task ends.
struct AddressSlot {
    slots: Arc<AddressSlots>,
    key: IpAddr,
}

impl AddressSlots {
    fn try_claim(self: &Arc<Self>, ip: IpAddr, max: usize) -> Option<AddressSlot> {
        let key = limiter_key(ip);
        let mut counts = self.counts.lock().unwrap();
        let count = counts.entry(key).or_insert(0);
        if *count >= max {
            return None;
        }
        *count += 1;
        Some(AddressSlot {
            slots: self.clone(),
            key,
        })
    }
}

impl Drop for AddressSlot {
    fn drop(&mut self) {
        let mut counts = self.slots.counts.lock().unwrap();
        if let Some(count) = counts.get_mut(&self.key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                // Drop idle entries, or the map grows with every address seen.
                counts.remove(&self.key);
            }
        }
    }
}

/// Turn on TCP keepalive, so a half-open connection (a laptop that slept, a
/// NAT that forgot) is eventually noticed rather than staying "online" with its
/// task and reservations. Joined members may idle for hours, so an
/// application-level idle timeout is not an option. Failure is logged.
fn enable_keepalive(stream: &TcpStream) {
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(Duration::from_secs(60))
        .with_interval(Duration::from_secs(20));
    if let Err(e) = socket2::SockRef::from(stream).set_tcp_keepalive(&keepalive) {
        tracing::debug!(error = %e, "could not enable TCP keepalive on a community connection");
    }
}

/// Accept connections forever with the default [`ServerLimits`].
pub async fn run_accept_loop(
    listener: TcpListener,
    state: Arc<Mutex<PartyState>>,
    privkey: Arc<RsaPrivateKey>,
    hub: Arc<Hub>,
) -> anyhow::Result<()> {
    run_accept_loop_with(listener, state, privkey, hub, ServerLimits::default()).await
}

/// Accept connections forever, serving each one over its own encrypted tunnel.
/// A failed `accept` (usually transient, e.g. EMFILE) is logged and skipped
/// rather than taking the community down.
pub async fn run_accept_loop_with(
    listener: TcpListener,
    state: Arc<Mutex<PartyState>>,
    privkey: Arc<RsaPrivateKey>,
    hub: Arc<Hub>,
    limits: ServerLimits,
) -> anyhow::Result<()> {
    // Bounds how fast one address may reconnect — otherwise reconnecting after
    // `dispatch::MAX_JOIN_ATTEMPTS` would itself be the password-guessing loop.
    let rate_limiter = Arc::new(Mutex::new(RateLimiter::new()));
    // Bounds how many connections exist at once, which the rate limit does not.
    let slots = Arc::new(Semaphore::new(limits.max_connections));
    let per_address = Arc::new(AddressSlots::default());
    let connection_limits = Arc::new(limits.connection.clone());

    loop {
        let (mut stream, addr) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(e) => {
                tracing::warn!(error = %e, "accept failed; continuing to listen");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        if rate_limiter.lock().await.check(addr.ip()) {
            tracing::warn!(%addr, "refusing connection: rate limit exceeded");
            // Drop it without handshaking; the client sees a closed connection.
            continue;
        }
        // Refused before any work is done for it: no task, no handshake.
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            tracing::warn!(%addr, max = limits.max_connections, "refusing connection: server is at its connection limit");
            continue;
        };
        let Some(address_slot) =
            per_address.try_claim(addr.ip(), limits.max_connections_per_address)
        else {
            tracing::warn!(%addr, max = limits.max_connections_per_address, "refusing connection: too many connections from this address");
            continue;
        };
        enable_keepalive(&stream);
        tracing::info!(%addr, "client connected");
        let state = state.clone();
        let privkey = privkey.clone();
        let hub = hub.clone();
        let connection_limits = connection_limits.clone();
        tokio::spawn(async move {
            // Held for the life of the connection; released when the task ends.
            let _permit = permit;
            let _address_slot = address_slot;
            match connection::serve_connection_with(
                &mut stream,
                &privkey,
                state,
                hub,
                &connection_limits,
            )
            .await
            {
                Err(e) => tracing::warn!(%addr, error = %e, "connection ended with error"),
                Ok(()) => tracing::info!(%addr, "client disconnected"),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_slots_are_capped_and_released() {
        let slots = Arc::new(AddressSlots::default());
        let ip: IpAddr = "203.0.113.7".parse().unwrap();
        let a = slots.try_claim(ip, 2).expect("first");
        let _b = slots.try_claim(ip, 2).expect("second");
        assert!(slots.try_claim(ip, 2).is_none(), "the third is refused");
        // Another address has its own allowance.
        assert!(slots.try_claim("203.0.113.8".parse().unwrap(), 2).is_some());
        drop(a);
        assert!(
            slots.try_claim(ip, 2).is_some(),
            "a released slot is reusable"
        );
    }

    #[test]
    fn an_ipv6_slash_64_shares_one_allowance() {
        let slots = Arc::new(AddressSlots::default());
        let _a = slots.try_claim("2001:db8::1".parse().unwrap(), 1).unwrap();
        assert!(slots.try_claim("2001:db8::2".parse().unwrap(), 1).is_none());
    }

    #[test]
    fn released_addresses_are_forgotten() {
        let slots = Arc::new(AddressSlots::default());
        drop(slots.try_claim("198.51.100.1".parse().unwrap(), 4).unwrap());
        assert!(slots.counts.lock().unwrap().is_empty());
    }

    /// Past the global cap a connection is closed at accept, before a
    /// handshake byte is sent, while the ones holding slots are served.
    #[tokio::test]
    async fn the_accept_loop_refuses_connections_past_its_cap() {
        use tokio::io::AsyncReadExt;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let state = Arc::new(Mutex::new(PartyState::new("Srv", None)));
        let privkey = Arc::new(
            messenger_core::core::generate_rsa_keypair(messenger_core::RSA_KEY_BITS).unwrap(),
        );
        let limits = ServerLimits {
            max_connections: 2,
            ..ServerLimits::default()
        };
        tokio::spawn(run_accept_loop_with(
            listener,
            state,
            privkey,
            Arc::new(Hub::new()),
            limits,
        ));

        // Each served connection receives the host's version packet first.
        let mut held = Vec::new();
        for _ in 0..2 {
            let mut s = TcpStream::connect(address).await.unwrap();
            let mut header = [0u8; 4];
            tokio::time::timeout(Duration::from_secs(10), s.read_exact(&mut header))
                .await
                .expect("a served connection hears the handshake")
                .unwrap();
            held.push(s);
        }
        let mut refused = TcpStream::connect(address).await.unwrap();
        let mut buf = [0u8; 4];
        let n = tokio::time::timeout(Duration::from_secs(10), refused.read(&mut buf))
            .await
            .expect("a refused connection is closed promptly")
            .unwrap_or(0);
        assert_eq!(n, 0, "closed without a handshake");

        // Releasing a slot lets the next one in.
        drop(held.pop());
        let mut next = None;
        for _ in 0..50 {
            let mut s = TcpStream::connect(address).await.unwrap();
            let mut header = [0u8; 4];
            match tokio::time::timeout(Duration::from_secs(2), s.read(&mut header)).await {
                Ok(Ok(n)) if n > 0 => {
                    next = Some(s);
                    break;
                }
                _ => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
        assert!(next.is_some(), "a freed slot is reused");
    }
}
