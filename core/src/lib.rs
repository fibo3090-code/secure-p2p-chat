//! Shared core for the Encrypted Messenger.
//!
//! This crate is reused by both the client app and the Party server. It provides:
//! - End-to-end encrypted messaging primitives with AES-256-GCM
//! - Forward secrecy via X25519 ephemeral key exchange and HKDF-SHA256
//! - A simple length-prefixed TCP protocol with a secure v3 handshake
//! - Identity management, file-transfer wire types, and shared domain types
//!
//! Modules:
//! - `core`: Cryptography and wire protocol structures.
//! - `network`: TCP sessions, relay rendezvous, and local discovery.
//! - `party`: Party server application protocol (Phase 1), riding on the v3 tunnel.
//! - `transfer`: Chunked file transfer utilities.
//! - `identity`: Persistent identity (RSA keys, fingerprints).
//! - `types`: Shared domain types used across layers.
//! - `util`: Helpers and utilities.
pub mod core;
pub mod identity;
pub mod network;
pub mod party;
pub mod transfer;
pub mod types;
pub mod util;

// Re-export commonly used types
pub use types::*;
pub use util::*;

// Constants
pub const PORT_DEFAULT: u16 = 12345;
pub const MAX_PACKET_SIZE: usize = 8 * 1024 * 1024; // 8 MiB
pub const FILE_CHUNK_SIZE: usize = 64 * 1024; // 64 KiB
pub const MAX_TEXT_MESSAGE_BYTES: usize = 64 * 1024; // 64 KiB
pub const TEXT_CHUNK_BYTES: usize = 48 * 1024; // 48 KiB to leave headroom for metadata
pub const AES_KEY_SIZE: usize = 32; // 256 bits
pub const AES_NONCE_SIZE: usize = 12; // 96 bits (GCM standard)
pub const AES_GCM_TAG_SIZE: usize = 16; // 128 bits (GCM authentication tag)
pub const REKEY_NONCE_SIZE: usize = 16; // 128-bit salt for HKDF key rotation
pub const RSA_KEY_BITS: usize = 2048;
pub const HANDSHAKE_TIMEOUT_SECS: u64 = 15;
/// Maximum file size for transfers: 10 GiB
pub const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024 * 1024; // 10 GiB
/// Signed invites older than this are rejected at import (30 days).
pub const INVITE_MAX_AGE_SECS: u64 = 30 * 24 * 60 * 60;
/// Tolerated clock skew for invite timestamps that appear to be in the future.
pub const INVITE_TIMESTAMP_SKEW_SECS: u64 = 60 * 60;
