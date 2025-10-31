pub mod core;
pub mod network;
pub mod transfer;
pub mod app;
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
pub const RSA_KEY_BITS: usize = 2048;
pub const HANDSHAKE_TIMEOUT_SECS: u64 = 15;
