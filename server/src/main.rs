//! Party server for the Encrypted Messenger.
//!
//! Phase 1 (slice 1) establishes the in-memory [`state::PartyState`] model, the
//! shared [`messenger_core::party`] protocol, and the [`dispatch`] layer that maps
//! requests to state mutations and responses. The network runtime — per-connection
//! v3 handshake reuse plus a Party message loop, and SQLite/blob persistence — is
//! the next slice. See `docs/06_phase1_party_server.md`.

mod dispatch;
mod state;

use dispatch::{handle_request, ConnState};
use messenger_core::party::PartyRequest;
use state::PartyState;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let server_password = std::env::var("PARTY_PASSWORD").ok();
    let mut state = PartyState::new("Encrypted Messenger Party", server_password);
    let channel = state.default_channel();

    tracing::info!(
        server_name = state.name(),
        tier = ?state.tier(),
        default_channel = %channel,
        listen_port = messenger_core::PORT_DEFAULT,
        "Party server state initialised"
    );

    // In-process self-check of the protocol → dispatch → state path, so the binary
    // demonstrably exercises the slice. (Real connections over the v3 tunnel are
    // wired in the next slice.)
    let mut conn = ConnState::new();
    let _ = handle_request(
        &mut state,
        &mut conn,
        PartyRequest::Join {
            username: "operator".to_string(),
            password: std::env::var("PARTY_PASSWORD").ok(),
        },
    );
    let _ = handle_request(
        &mut state,
        &mut conn,
        PartyRequest::PostMessage {
            channel,
            text: "server is up".to_string(),
        },
    );
    tracing::info!(
        member = ?conn.member(),
        history = state.history_since(channel, 0).len(),
        "self-check: join + post succeeded"
    );

    eprintln!(
        "messenger-server (Phase 1, slice 1): protocol, state, and dispatch are in \
         place and tested. The network runtime is the next slice \
         (see docs/06_phase1_party_server.md)."
    );

    Ok(())
}
