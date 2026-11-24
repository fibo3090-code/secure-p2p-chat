//! Application layer: UI-agnostic orchestration ([`app::ChatManager`]) shared
//! with the desktop app (`p2pem-desktop`), plus the ratatui terminal UI. Built
//! on [`messenger_core`] for crypto, protocol, identity and transport.

// Re-exported so `crate::types`, `crate::util`, the constants etc. resolve here
// and in the integration tests.
pub use messenger_core::*;

pub mod app;
pub mod colorgrid;
pub mod logbuf;
pub mod support;
pub mod tui;
