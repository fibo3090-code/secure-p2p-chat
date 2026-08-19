//! Party/Community server, as a library.
//!
//! The binary in `main.rs` is a thin wrapper: it parses arguments, loads the
//! durable state and the server identity, binds a listener, and hands all of it
//! to [`run_accept_loop`].
//!
//! It is split this way so the server can be stood up **in process** by a test.
//! Communities are the most involved subsystem in the workspace — a real join
//! crosses the v3 handshake, the join gate, the hub, the dispatch layer, SQLite
//! and the blob store — and none of that was reachable end to end while the only
//! entry point was `fn main`. `client/tests/party_e2e.rs` drives this against the
//! real `PartyManager` over a loopback socket.

pub mod connection;
pub mod dispatch;
pub mod hub;
pub mod identity;
pub mod state;

use std::sync::Arc;
use std::time::Duration;

use messenger_core::network::ratelimit::RateLimiter;
use rsa::RsaPrivateKey;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use hub::Hub;
use state::PartyState;

/// Accept connections forever, serving each one over its own encrypted tunnel.
///
/// Never returns in normal operation. A failed `accept` is logged and skipped
/// rather than propagated: it is almost always transient (the peer vanished
/// between the SYN and the accept, or the process briefly ran out of file
/// descriptors), and returning would take the whole community down for the life
/// of the process over one hiccup.
pub async fn run_accept_loop(
    listener: TcpListener,
    state: Arc<Mutex<PartyState>>,
    privkey: Arc<RsaPrivateKey>,
    hub: Arc<Hub>,
) -> anyhow::Result<()> {
    // Bounds how fast one address may reconnect. Each accepted socket spawns a
    // task that immediately runs an RSA handshake, and `dispatch::MAX_JOIN_ATTEMPTS`
    // makes a password guesser reconnect every few attempts — so without this,
    // reconnecting IS the guessing loop.
    let rate_limiter = Arc::new(Mutex::new(RateLimiter::new()));

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
        tracing::info!(%addr, "client connected");
        let state = state.clone();
        let privkey = privkey.clone();
        let hub = hub.clone();
        tokio::spawn(async move {
            if let Err(e) = connection::serve_connection(&mut stream, &privkey, state, hub).await {
                tracing::warn!(%addr, error = %e, "connection ended with error");
            } else {
                tracing::info!(%addr, "client disconnected");
            }
        });
    }
}
