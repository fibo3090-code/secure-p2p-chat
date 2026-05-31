//! Encrypted P2P Messenger client application.
//!
//! This crate is the unified desktop app (egui GUI + ratatui TUI). It builds on
//! [`messenger_core`] for all cryptography, wire protocol, identity, and transport,
//! and adds the application orchestration (`ChatManager`), persistence, diagnostics,
//! and user interface layers.
//!
//! Modules:
//! - `app`: High-level orchestration (`ChatManager`), persistence, and state handling.
//! - `gui`: egui/eframe desktop interface.
//! - `tui`: ratatui terminal interface.
//! - `support`: diagnostics export and panic/crash support.
//! - `colorgrid`: fingerprint color-grid rendering helper (egui-coupled).

// Re-export the shared core so existing `crate::core`, `crate::types`,
// `crate::network`, `crate::identity`, `crate::transfer`, `crate::util`, and the
// shared constants continue to resolve unchanged across the client modules and the
// integration test suite.
pub use messenger_core::*;

pub mod app;
pub mod colorgrid;
pub mod gui;
pub mod support;
pub mod tui;
