//! Party server for the Encrypted Messenger.
//!
//! Phase 1: the server binds a TCP listener and serves each client over a reused
//! Protocol v3 encrypted tunnel ([`messenger_core::network::host_handshake`]),
//! driving the shared [`state::PartyState`] via the [`dispatch`] layer. Members
//! join with a username + the optional server password, post to channels, and the
//! server stores history so offline members catch up on reconnect.
//!
//! Not yet wired (next steps): cross-connection broadcast fan-out, a persistent
//! server identity, and SQLite/blob persistence. See `docs/06_phase1_party_server.md`.

mod connection;
mod dispatch;
mod hub;
mod state;

use std::sync::Arc;

use messenger_core::core::{fingerprint_pubkey, generate_rsa_keypair, pem_encode_public};
use messenger_core::RSA_KEY_BITS;
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

    let server_password = std::env::var("PARTY_PASSWORD").ok();
    let state = Arc::new(Mutex::new(PartyState::new(
        "Encrypted Messenger Party",
        server_password,
    )));

    // Ephemeral server identity for now (persisted identity + stable TOFU is a
    // follow-up). Clients verify this fingerprint on first connect.
    let privkey = Arc::new(generate_rsa_keypair(RSA_KEY_BITS)?);
    let fingerprint =
        fingerprint_pubkey(pem_encode_public(&RsaPublicKey::from(&*privkey))?.as_bytes());
    let hub = Arc::new(Hub::new());

    let port = messenger_core::PORT_DEFAULT;
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!(
        server_name = %state.lock().await.name(),
        %fingerprint,
        port,
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
