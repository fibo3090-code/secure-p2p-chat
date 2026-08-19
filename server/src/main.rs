//! Party/Community server for the Encrypted Messenger.
//!
//! The server binds a TCP listener and serves each client over a reused
//! Protocol v3 encrypted tunnel ([`messenger_core::network::host_handshake`]),
//! driving the shared [`state::PartyState`] via the [`dispatch`] layer. Members
//! join with a username + the optional server password, post to channels and DMs,
//! share files, and the server stores history durably (SQLite + blob store) so
//! offline members catch up on reconnect.

//! Everything below the argument parsing lives in the library half of this
//! crate (`lib.rs`), so the server can also be stood up in process by a test.

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use messenger_core::core::{fingerprint_pubkey, pem_encode_public};
use messenger_server::hub::Hub;
use messenger_server::state::PartyState;
use messenger_server::{identity, run_accept_loop};
use rsa::RsaPublicKey;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

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

    run_accept_loop(listener, state, privkey, hub).await
}
