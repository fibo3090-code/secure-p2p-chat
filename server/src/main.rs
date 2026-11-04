//! Party/Community server for the Encrypted Messenger.
//!
//! The server binds a TCP listener and serves each client over a reused
//! Protocol v3 encrypted tunnel ([`messenger_core::network::host_handshake`]),
//! driving the shared [`state::PartyState`] via the [`dispatch`] layer. Members
//! join with a username + the server password, post to channels and DMs,
//! share files, and the server stores history durably (SQLite + blob store) so
//! offline members catch up on reconnect.

//! Everything below the argument parsing lives in the library half of this
//! crate (`lib.rs`), so the server can also be stood up in process by a test.

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use messenger_core::core::{fingerprint_pubkey, pem_encode_public};
use messenger_core::util::canonical_fingerprint;
use messenger_server::hub::Hub;
use messenger_server::state::PartyState;
use messenger_server::{identity, run_accept_loop};
use rsa::RsaPublicKey;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

/// Self-hosted Community (Party) server. Friends join with your address, the
/// password, and a username — no port forwarding gymnastics on their side, no
/// account system. All transport is end-to-end encrypted to the server
/// (Protocol v3); clients pin this server's fingerprint on first join.
///
/// A password is required unless you pass --open. Until the community has an
/// owner, the server prints a one-time owner claim token at startup: join with
/// it in the password field to become the owner (or pass --owner with your
/// own fingerprint instead).
#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Args {
    /// Display name of this community, shown to everyone who joins.
    #[arg(long, env = "PARTY_NAME", default_value = "Encrypted Messenger Party")]
    name: String,

    /// TCP port to listen on.
    #[arg(long, env = "PARTY_PORT", default_value_t = messenger_core::PORT_DEFAULT)]
    port: u16,

    /// Require this password to join. Required unless --open is given.
    #[arg(long, env = "PARTY_PASSWORD")]
    password: Option<String>,

    /// Run without a password, so anyone who can reach the port may join.
    /// Must be given explicitly: an open server on a public address is open to
    /// the whole internet.
    #[arg(long, env = "PARTY_OPEN", conflicts_with = "password")]
    open: bool,

    /// Fingerprint of the identity that owns this community (shown as yours in
    /// the app: desktop Settings > Your identity, TUI :identity). That member
    /// becomes the owner when they join, and any other owner becomes an admin —
    /// which is also how to recover a community whose ownership went to the
    /// wrong identity. Without it, the server prints a one-time owner claim
    /// token until someone claims ownership.
    #[arg(long, env = "PARTY_OWNER", value_name = "FINGERPRINT")]
    owner: Option<String>,

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
    if args.password.is_none() && !args.open {
        anyhow::bail!(
            "refusing to start without a password: pass --password <PASSWORD> (or set \
             PARTY_PASSWORD), or pass --open to let anyone who can reach this port join"
        );
    }
    let owner = match args.owner.as_deref().map(canonical_fingerprint) {
        Some(fp) if fp.len() != 64 || !fp.bytes().all(|b| b.is_ascii_hexdigit()) => {
            anyhow::bail!("--owner must be a 64-character hex fingerprint, got {fp:?}");
        }
        other => other,
    };
    let password_protected = args.password.is_some();

    // Durable state: members, channels, and history survive restarts (loaded from
    // and auto-saved to the data dir).
    let mut loaded = PartyState::load(&args.name, args.password, &args.data_dir)?;
    loaded.set_owner_fingerprint(owner);
    let claim_token = loaded.owner_claim_token().map(str::to_owned);
    let state = Arc::new(Mutex::new(loaded));

    // Persistent server identity: clients pin this fingerprint via TOFU on first
    // connect, so it must stay stable across restarts.
    let privkey = Arc::new(identity::load_or_create_server_identity(&args.data_dir)?);
    let fingerprint =
        fingerprint_pubkey(pem_encode_public(&RsaPublicKey::from(&*privkey))?.as_bytes())?;
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
    if !password_protected {
        tracing::warn!("this server is OPEN (--open): anyone who can reach this port can join it");
    }
    if let Some(token) = claim_token {
        tracing::warn!(
            owner_claim_token = %token,
            "this community has no owner yet. Join it with this one-time owner claim \
             token in the PASSWORD field to become its owner; it stops working once used, \
             and a restart issues a new one. Until then nobody can administer it. \
             (Or restart with --owner <your fingerprint>.)"
        );
    }

    run_accept_loop(listener, state, privkey, hub).await
}
