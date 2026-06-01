//! Party server for the Encrypted Messenger.
//!
//! Phase 1 (slice 1) establishes the in-memory [`state::PartyState`] model and the
//! shared [`messenger_core::party`] protocol. The network runtime — per-connection
//! v3 handshake reuse plus a Party message loop, and SQLite/blob persistence — is
//! the next slice. See `docs/06_phase1_party_server.md`.

mod state;

use state::PartyState;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // Initialise the runtime model so the protocol/state foundation is exercised.
    // (Accepting connections over the v3 tunnel arrives in the next slice.)
    let server_password = std::env::var("PARTY_PASSWORD").ok();
    let state = PartyState::new("Encrypted Messenger Party", server_password);

    tracing::info!(
        server_name = state.name(),
        tier = ?state.tier(),
        default_channel = %state.default_channel(),
        listen_port = messenger_core::PORT_DEFAULT,
        "Party server state initialised"
    );

    eprintln!(
        "messenger-server (Phase 1, slice 1): protocol + state foundation is in \
         place and tested. The network runtime is the next slice \
         (see docs/06_phase1_party_server.md)."
    );

    Ok(())
}
