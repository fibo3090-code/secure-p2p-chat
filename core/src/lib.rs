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
/// Upper bound on how many chunks one large text message may be split into,
/// enforced symmetrically on send and receive. Caps a chunked message at
/// `MAX_TEXT_CHUNKS * TEXT_CHUNK_BYTES` (~24 MiB) and — critically — stops a
/// peer from sending a single `TextChunk` whose `total_chunks` is huge, which
/// would make the reassembly buffer pre-allocate gigabytes (remote OOM/DoS).
pub const MAX_TEXT_CHUNKS: u32 = 512;
/// Cap on distinct in-flight large-message reassembly buffers per chat, so a
/// peer cannot exhaust memory by opening many partial messages that each sit
/// around until the reassembly timeout.
pub const MAX_CONCURRENT_PARTIAL_TEXT_PER_CHAT: usize = 16;
pub const AES_KEY_SIZE: usize = 32; // 256 bits
pub const AES_NONCE_SIZE: usize = 12; // 96 bits (GCM standard)
pub const AES_GCM_TAG_SIZE: usize = 16; // 128 bits (GCM authentication tag)
pub const REKEY_NONCE_SIZE: usize = 16; // 128-bit salt for HKDF key rotation
pub const RSA_KEY_BITS: usize = 2048;
/// Minimum length for the password that protects the identity keystore and the
/// encrypted history. Argon2 over this password is the *entire* at-rest story,
/// so the floor has to match the advice the UI gives ("use 12+ characters")
/// rather than undercut it. Enforced in the identity layer, not just the UI, so
/// no front-end can set a weaker one. Unlocking an existing identity is
/// deliberately unaffected — a floor change must never lock anyone out.
pub const MIN_PASSWORD_LEN: usize = 12;
pub const HANDSHAKE_TIMEOUT_SECS: u64 = 15;
/// Per-candidate TCP connect timeout when an invite carries several
/// addresses to try in order (multi-address invites). Short enough that
/// falling from a dead external address back to the LAN one feels snappy,
/// long enough for a slow WAN round-trip.
pub const CONNECT_ATTEMPT_TIMEOUT_SECS: u64 = 10;
/// Maximum file size for transfers: 10 GiB
pub const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024 * 1024; // 10 GiB
/// Signed invites older than this are rejected at import (30 days).
pub const INVITE_MAX_AGE_SECS: u64 = 30 * 24 * 60 * 60;
/// Tolerated clock skew for invite timestamps that appear to be in the future.
pub const INVITE_TIMESTAMP_SKEW_SECS: u64 = 60 * 60;
