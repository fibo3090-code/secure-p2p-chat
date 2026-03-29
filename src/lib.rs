//! Encrypted P2P Messenger core library.
//!
//! This crate powers the desktop application by providing:
//! - End-to-end encrypted messaging with AES-256-GCM
//! - Forward secrecy via X25519 ephemeral key exchange and HKDF-SHA256
//! - A simple length-prefixed TCP protocol with a secure v3 handshake
//! - Business logic, identity management, file transfer, and GUI integration points
//!
//! Modules:
//! - `app`: High-level orchestration (`ChatManager`) and state handling.
//! - `core`: Cryptography and wire protocol structures.
//! - `network`: TCP sessions and handshake implementation.
//! - `transfer`: Chunked file transfer utilities.
//! - `identity`: Persistent identity (RSA keys, fingerprints).
//! - `types`: Shared domain types used across layers.
//! - `util`: Helpers and utilities.
pub mod app;
pub mod core;
pub mod gui;
pub mod identity;
pub mod network;
pub mod support;
pub mod transfer;
pub mod tui;
pub mod types;
pub mod util;

// Re-export commonly used types
pub use types::*;
pub use util::*;

// Constants
pub const PORT_DEFAULT: u16 = 12345;
pub const MAX_PACKET_SIZE: usize = 8 * 1024 * 1024; // 8 MiB
pub const FILE_CHUNK_SIZE: usize = 64 * 1024; // 64 KiB
pub const AES_KEY_SIZE: usize = 32; // 256 bits
pub const AES_NONCE_SIZE: usize = 12; // 96 bits (GCM standard)
pub const AES_GCM_TAG_SIZE: usize = 16; // 128 bits (GCM authentication tag)
pub const RSA_KEY_BITS: usize = 2048;
pub const HANDSHAKE_TIMEOUT_SECS: u64 = 15;
/// Maximum file size for transfers: 10 GiB
pub const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024 * 1024; // 10 GiB
