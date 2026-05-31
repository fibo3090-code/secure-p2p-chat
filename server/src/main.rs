//! Party server for the Encrypted Messenger.
//!
//! Placeholder binary established in Phase 0 (workspace refactor) to prove that the
//! shared [`messenger_core`] crate is reusable from a server context. The actual
//! Party server — accounts, channels, server-routed messaging, offline buffering,
//! and the encrypted transport handshake reusing `messenger_core` — is built in
//! Phase 1. See `docs/05_platform_spec.md`.

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // Touch the shared core so the dependency is exercised and the contract that
    // "the server reuses core" is enforced by the build from day one.
    tracing::info!(
        core_default_port = messenger_core::PORT_DEFAULT,
        "Party server is not yet implemented (Phase 1). See docs/05_platform_spec.md."
    );

    eprintln!(
        "messenger-server is a Phase 0 placeholder and does not serve yet. \
         The Party server is implemented in Phase 1 (see docs/05_platform_spec.md)."
    );

    Ok(())
}
