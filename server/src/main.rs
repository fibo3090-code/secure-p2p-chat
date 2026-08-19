//! Party/Community server for the Encrypted Messenger.
//!
//! The server binds a TCP listener and serves each client over a reused
//! Protocol v3 encrypted tunnel ([`messenger_core::network::host_handshake`]),
//! driving the shared [`state::PartyState`] via the [`dispatch`] layer. Members
//! join with a username + the optional server password, post to channels and DMs,
//! share files, and the server stores history durably (SQLite + blob store) so
//! offline members catch up on reconnect.

mod connection;
mod dispatch;
mod hub;
mod identity;
mod state;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use messenger_core::core::{fingerprint_pubkey, pem_encode_public};
// Shared with the relay server's accept loop — both face the open internet and
// need the same per-address ceiling.
use messenger_core::network::ratelimit::RateLimiter;
use rsa::RsaPublicKey;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use hub::Hub;
use state::PartyState;

/// Self-hosted Community (Party) server. Friends join with your address, an
/// optional password, and a username — no port forwarding gymnastics on their
/// side, no account system. All transport is end-to-end encrypted to the server
/// (Protocol v3); clients pin this server's fingerprint on first join.
#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Args {
    /// Display name of this community, shown to everyone who joins.
    #[arg(long, env = "PARTY_NAME", default_value = "Encrypted Messenger Party")]
    name: String,

    /// TCP port to listen on.
    #[arg(long, env = "PARTY_PORT", default_value_t = messenger_core::PORT_DEFAULT)]
    port: u16,

    /// Require this password to join. Omit for an open server.
    #[arg(long, env = "PARTY_PASSWORD")]
    password: Option<String>,

    /// Directory for durable state: member/channel/message database (party.db),
    /// the file blob store, and the server identity key.
    #[arg(long, env = "PARTY_DATA_DIR", default_value = "party-data")]
    data_dir: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();
    let password_protected = args.password.is_some();

    // Durable state: members, channels, and history survive restarts (loaded from
    // and auto-saved to the data dir).
    let state = Arc::new(Mutex::new(PartyState::load(
        &args.name,
        args.password,
        &args.data_dir,
    )?));

    // Persistent server identity: clients pin this fingerprint via TOFU on first
    // connect, so it must stay stable across restarts.
    let privkey = Arc::new(identity::load_or_create_server_identity(&args.data_dir)?);
    let fingerprint =
        fingerprint_pubkey(pem_encode_public(&RsaPublicKey::from(&*privkey))?.as_bytes());
    let hub = Arc::new(Hub::new());

    let port = args.port;
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!(
        server_name = %state.lock().await.name(),
        %fingerprint,
        port,
        data_dir = %args.data_dir.display(),
        password_protected,
        "Community server listening — share your address and this fingerprint with people you invite"
    );

    // Bounds how fast one address may reconnect. Each accepted socket spawns a
    // task that immediately runs an RSA handshake, and `dispatch::MAX_JOIN_ATTEMPTS`
    // makes a password guesser reconnect every few attempts — so without this,
    // reconnecting IS the guessing loop.
    let rate_limiter = Arc::new(Mutex::new(RateLimiter::new()));

    loop {
        // A failed `accept` is almost always transient — the peer vanished
        // between the SYN and our accept, or we briefly ran out of file
        // descriptors. Propagating it with `?` took the whole community server
        // down for the life of the process over one hiccup.
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
