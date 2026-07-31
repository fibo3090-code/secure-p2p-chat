//! Encrypted P2P Messenger — application layer.
//!
//! This crate holds the UI-agnostic application orchestration shared by every
//! front-end, plus the ratatui terminal interface. It builds on
//! [`messenger_core`] for all cryptography, wire protocol, identity, and
//! transport.
//!
//! The desktop app (`p2pem-desktop`, a Tauri 2 + React shell) links this crate
//! as a library and drives the same [`app::ChatManager`]. The egui GUI that used
//! to live here was retired once the desktop app reached parity — shipping two
//! desktop apps meant users had to guess which one to install, and the older one
//! looked like the default. See `docs/platform_spec.md` §10.
//!
//! Modules:
//! - `app`: orchestration ([`app::ChatManager`]), persistence, and state handling.
//! - `tui`: ratatui terminal interface.
//! - `logbuf`: bounded in-process `tracing` buffer for the log overlay and
//!   diagnostics bundles.
//! - `colorgrid`: fingerprint colour-grid data (UI-toolkit agnostic).
//! - `support`: diagnostics export and panic/crash support.

// Re-export the shared core so existing `crate::core`, `crate::types`,
// `crate::network`, `crate::identity`, `crate::transfer`, `crate::util`, and the
// shared constants continue to resolve unchanged across the client modules and the
// integration test suite.
pub use messenger_core::*;

pub mod app;
pub mod colorgrid;
pub mod logbuf;
pub mod support;
pub mod tui;
