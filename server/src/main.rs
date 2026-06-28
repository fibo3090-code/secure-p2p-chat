//! Party server for the Encrypted Messenger.
//!
//! Phase 1: the server binds a TCP listener and serves each client over a reused
//! Protocol v3 encrypted tunnel ([`messenger_core::network::host_handshake`]),
//! driving the shared [`state::PartyState`] via the [`dispatch`] layer. Members
//! join with a username + the optional server password, post to channels, and the
//! server stores history so offline members catch up on reconnect.
//!
//! Not yet wired (next step): SQLite/blob persistence of state and history. See
//! `docs/05_platform_spec.md`.

mod connection;
mod dispatch;
mod hub;
mod identity;
mod state;

use std::path::PathBuf;
use std::sync::Arc;

use messenger_core::core::{fingerprint_pubkey, pem_encode_public};
use rsa::RsaPublicKey;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use hub::Hub;
use state::PartyState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let data_dir = std::env::var_os("PARTY_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("party-data"));
    let server_password = std::env::var("PARTY_PASSWORD").ok();

    // Durable state: members, channels, and history survive restarts (loaded from
    // and auto-saved to the data dir).
    let state = Arc::new(Mutex::new(PartyState::load(
        "Encrypted Messenger Party",
        server_password,
        &data_dir,
    )?));

    // Persistent server identity: clients pin this fingerprint via TOFU on first
    // connect, so it must stay stable across restarts.
    let privkey = Arc::new(identity::load_or_create_server_identity(&data_dir)?);
    let fingerprint =
        fingerprint_pubkey(pem_encode_public(&RsaPublicKey::from(&*privkey))?.as_bytes());
    let hub = Arc::new(Hub::new());

    let port = messenger_core::PORT_DEFAULT;
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!(
        server_name = %state.lock().await.name(),
        %fingerprint,
        port,
        data_dir = %data_dir.display(),
        password_protected = std::env::var("PARTY_PASSWORD").is_ok(),
        "Party server listening"
    );

    loop {
        let (mut stream, addr) = listener.accept().await?;
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
