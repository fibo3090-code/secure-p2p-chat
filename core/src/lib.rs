//! Shared core for the Encrypted Messenger, used by the client and the Party
//! server: crypto and wire protocol (`core`), sessions/relay/discovery
//! (`network`), the Party protocol (`party`), file transfer (`transfer`),
//! identity, shared domain types and utilities.
pub mod core;
pub mod identity;
pub mod network;
pub mod party;
pub mod transfer;
pub mod types;
pub mod util;

pub use types::*;
pub use util::*;

pub const PORT_DEFAULT: u16 = 12345;
pub const MAX_PACKET_SIZE: usize = 8 * 1024 * 1024; // 8 MiB
pub const FILE_CHUNK_SIZE: usize = 64 * 1024; // 64 KiB
pub const MAX_TEXT_MESSAGE_BYTES: usize = 64 * 1024; // 64 KiB
pub const TEXT_CHUNK_BYTES: usize = 48 * 1024; // 48 KiB to leave headroom for metadata
/// Cap on chunks per text message (~24 MiB total), enforced on send and
/// receive; also stops a huge `total_chunks` pre-allocating gigabytes.
pub const MAX_TEXT_CHUNKS: u32 = 512;
/// Cap on in-flight partial text reassemblies per chat.
pub const MAX_CONCURRENT_PARTIAL_TEXT_PER_CHAT: usize = 16;
pub const AES_KEY_SIZE: usize = 32; // 256 bits
pub const AES_NONCE_SIZE: usize = 12; // 96 bits (GCM standard)
pub const AES_GCM_TAG_SIZE: usize = 16; // 128 bits (GCM authentication tag)
pub const REKEY_NONCE_SIZE: usize = 16; // 128-bit salt for HKDF key rotation
pub const RSA_KEY_BITS: usize = 2048;
/// Minimum keystore/history password length, enforced in the identity layer so
/// no front-end can set a weaker one. Unlocking is deliberately unaffected, so
/// raising the floor never locks anyone out.
pub const MIN_PASSWORD_LEN: usize = 12;
pub const HANDSHAKE_TIMEOUT_SECS: u64 = 15;
/// Per-candidate TCP connect timeout when an invite lists several addresses.
pub const CONNECT_ATTEMPT_TIMEOUT_SECS: u64 = 10;
pub const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024 * 1024; // 10 GiB
/// Signed invites older than this are rejected at import (30 days).
pub const INVITE_MAX_AGE_SECS: u64 = 30 * 24 * 60 * 60;
/// Tolerated clock skew for invite timestamps in the future.
pub const INVITE_TIMESTAMP_SKEW_SECS: u64 = 60 * 60;
