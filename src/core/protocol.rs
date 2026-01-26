use super::SignatureScheme;
use serde::{Deserialize, Serialize};

/// Protocol version for forward compatibility
///
/// Version 3: ECDH-first handshake (Encrypted Identity Exchange)
pub const PROTOCOL_VERSION: u8 = 3;

/// Protocol messages exchanged between peers
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub enum ProtocolMessage {
    /// Protocol version announcement (first message)
    Version { version: u8 },

    /// Ephemeral X25519 public key for forward secrecy
    EphemeralKey { public_key: Vec<u8> },

    /// Supported signature schemes for negotiation (sent before identity proof)
    /// Encoded as: u8 count | [u8 scheme]*
    /// Allows peers to agree on a common signature scheme (RSA-PSS or Ed25519)
    SupportedSignatureSchemes { schemes: Vec<u8> },

    /// Text message (with sequence number for replay protection)
    Text {
        text: String,
        timestamp: u64,
        seq: u64,
    },

    /// File metadata (sent before chunks)
    FileMeta {
        filename: String,
        size: u64,
        seq: u64,
    },

    /// File data chunk
    FileChunk { chunk: Vec<u8>, seq: u64 },

    /// File transfer complete
    FileEnd { seq: u64 },

    /// Keep-alive ping
    Ping { seq: u64 },

    /// Typing indicator - user started typing
    TypingStart { seq: u64 },

    /// Typing indicator - user stopped typing
    TypingStop { seq: u64 },

    /// Session key rotation (rekeying) - contains random nonce for next key derivation
    /// After receiving this message, peers independently derive the next session key using:
    /// next_key = rekey_session_key(current_key, nonce)
    /// All subsequent messages use the new key.
    Rekey { nonce: Vec<u8>, seq: u64 },
}

impl std::fmt::Debug for ProtocolMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Version { version } => {
                f.debug_struct("Version").field("version", version).finish()
            }
            Self::EphemeralKey { public_key } => f
                .debug_struct("EphemeralKey")
                .field("public_key_len", &public_key.len())
                .finish(),
            Self::SupportedSignatureSchemes { schemes } => f
                .debug_struct("SupportedSignatureSchemes")
                .field("schemes", schemes)
                .finish(),
            Self::Text { seq, timestamp, .. } => f
                .debug_struct("Text")
                .field("seq", seq)
                .field("timestamp", timestamp)
                .field("text", &"***REDACTED***")
                .finish(),
            Self::FileMeta {
                filename,
                size,
                seq,
            } => f
                .debug_struct("FileMeta")
                .field("seq", seq)
                .field("filename", filename)
                .field("size", size)
                .finish(),
            Self::FileChunk { seq, chunk } => f
                .debug_struct("FileChunk")
                .field("seq", seq)
                .field("chunk_len", &chunk.len())
                .finish(),
            Self::FileEnd { seq } => f.debug_struct("FileEnd").field("seq", seq).finish(),
            Self::Ping { seq } => f.debug_struct("Ping").field("seq", seq).finish(),
            Self::TypingStart { seq } => f.debug_struct("TypingStart").field("seq", seq).finish(),
            Self::TypingStop { seq } => f.debug_struct("TypingStop").field("seq", seq).finish(),
            Self::Rekey { nonce, seq } => f
                .debug_struct("Rekey")
                .field("seq", seq)
                .field("nonce_len", &nonce.len())
                .finish(),
        }
    }
}

/// Identity Proof for Encrypted Handshake (Protocol v3)
/// Sent inside the encrypted tunnel to prove identity.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct IdentityProof {
    /// Public Key PEM (Identity) - currently only RSA
    /// Future: could support PEM-encoded Ed25519 keys
    pub public_key_pem: String,
    /// Signature of the Ephemeral Public Key used in this session
    /// Signed by the Identity Private Key.
    /// Format: Sign(SHA256("IDENTITY_PROOF" || ephemeral_pubkey))
    pub signature: Vec<u8>,
    /// Protocol Version
    pub version: u32,
    /// Chat ID (optional here, or separate message)
    pub chat_id: uuid::Uuid,
    /// Signature scheme used (RSA-PSS or Ed25519)
    /// Default: RSA-PSS for v3.0 compatibility
    #[serde(default = "default_signature_scheme")]
    pub signature_scheme: SignatureScheme,
}

/// Default signature scheme for backward compatibility
fn default_signature_scheme() -> SignatureScheme {
    SignatureScheme::RsaPss
}

impl ProtocolMessage {
    /// Convert message to plain bytes (binary tagged format)
    pub fn to_plain_bytes(&self) -> Vec<u8> {
        // Binary tagged format:
        // [type: u8][payload...]
        // Multi-field payloads use big-endian integers and length-prefixed blobs.
        let mut v = Vec::new();
        match self {
            Self::Version { version } => {
                v.push(0u8);
                v.push(*version);
            }

            Self::EphemeralKey { public_key } => {
                v.push(1u8);
                let len = (public_key.len() as u32).to_be_bytes();
                v.extend_from_slice(&len);
                v.extend_from_slice(public_key);
            }

            Self::SupportedSignatureSchemes { schemes } => {
                v.push(9u8);
                v.push(schemes.len() as u8);
                v.extend_from_slice(schemes);
            }

            Self::Text {
                text,
                timestamp,
                seq,
            } => {
                v.push(2u8);
                v.extend_from_slice(&seq.to_be_bytes());
                v.extend_from_slice(&timestamp.to_be_bytes());
                let bytes = text.as_bytes();
                let len = (bytes.len() as u32).to_be_bytes();
                v.extend_from_slice(&len);
                v.extend_from_slice(bytes);
            }

            Self::FileMeta {
                filename,
                size,
                seq,
            } => {
                v.push(3u8);
                v.extend_from_slice(&seq.to_be_bytes());
                v.extend_from_slice(&size.to_be_bytes());
                let fn_bytes = filename.as_bytes();
                let len = (fn_bytes.len() as u32).to_be_bytes();
                v.extend_from_slice(&len);
                v.extend_from_slice(fn_bytes);
            }

            Self::FileChunk { chunk, seq } => {
                v.push(4u8);
                v.extend_from_slice(&seq.to_be_bytes());
                let len = (chunk.len() as u32).to_be_bytes();
                v.extend_from_slice(&len);
                v.extend_from_slice(chunk);
            }

            Self::FileEnd { seq } => {
                v.push(5u8);
                v.extend_from_slice(&seq.to_be_bytes());
            }

            Self::Ping { seq } => {
                v.push(6u8);
                v.extend_from_slice(&seq.to_be_bytes());
            }

            Self::TypingStart { seq } => {
                v.push(7u8);
                v.extend_from_slice(&seq.to_be_bytes());
            }

            Self::TypingStop { seq } => {
                v.push(8u8);
                v.extend_from_slice(&seq.to_be_bytes());
            }

            Self::Rekey { nonce, seq } => {
                v.push(10u8);
                v.extend_from_slice(&seq.to_be_bytes());
                let len = (nonce.len() as u32).to_be_bytes();
                v.extend_from_slice(&len);
                v.extend_from_slice(nonce);
            }
        }
        v
    }

    /// Parse message from plain bytes; supports new binary tagged format and falls back to legacy ASCII prefixes.
    pub fn from_plain_bytes(b: &[u8]) -> Option<Self> {
        // First attempt binary tagged format
        if let Some(bin) = Self::from_binary_tagged(b) {
            return Some(bin);
        }

        // Fallback: legacy ASCII-prefixed format for compatibility with tests and older peers
        // Keep legacy parsing limited and defensive to avoid DoS or injection.
        if b.starts_with(b"VERSION:") {
            let version_str = String::from_utf8_lossy(&b[8..]);
            if let Ok(version) = version_str.trim().parse::<u8>() {
                return Some(Self::Version { version });
            }
            return None;
        }

        if b.starts_with(b"EPHEMERAL_KEY:") {
            let public_key = b[14..].to_vec();
            return Some(Self::EphemeralKey { public_key });
        }

        if b.starts_with(b"TEXT:") {
            if b.len() > 64 * 1024 {
                return None;
            }
            let s = String::from_utf8_lossy(&b[5..]);
            let parts: Vec<&str> = s.splitn(2, ':').collect();
            if parts.len() == 2 {
                let seq = parts[0].parse::<u64>().ok()?;
                let text = parts[1].to_string();
                return Some(Self::Text {
                    text,
                    timestamp: crate::util::current_timestamp_millis(),
                    seq,
                });
            }
            return None;
        }

        if b.starts_with(b"FILE_META|") {
            let s = String::from_utf8_lossy(b);
            let parts: Vec<&str> = s.splitn(4, '|').collect();
            if parts.len() == 4 {
                let seq = parts[1].parse::<u64>().ok()?;
                let raw_filename = parts[2];
                let filename = crate::util::sanitize_filename(raw_filename);
                if let Ok(size) = parts[3].parse::<u64>() {
                    if size > crate::MAX_FILE_SIZE {
                        return None;
                    }
                    return Some(Self::FileMeta {
                        filename,
                        size,
                        seq,
                    });
                }
            }
            return None;
        }

        if b.starts_with(b"FILE_CHUNK:") {
            let mut parts = b[11..].splitn(2, |&c| c == b':');
            let seq_bytes = parts.next()?;
            let chunk = parts.next()?;
            let seq_str = std::str::from_utf8(seq_bytes).ok()?;
            let seq = seq_str.parse::<u64>().ok()?;
            if chunk.len() > crate::FILE_CHUNK_SIZE {
                return None;
            }
            return Some(Self::FileChunk {
                chunk: chunk.to_vec(),
                seq,
            });
        }

        if b.starts_with(b"FILE_END:") {
            let s = String::from_utf8_lossy(&b[9..]);
            let seq = s.trim().parse::<u64>().ok()?;
            return Some(Self::FileEnd { seq });
        }

        if b.starts_with(b"PING:") {
            let s = String::from_utf8_lossy(&b[5..]);
            let seq = s.trim().parse::<u64>().ok()?;
            return Some(Self::Ping { seq });
        }

        if b.starts_with(b"TYPING_START:") {
            let s = String::from_utf8_lossy(&b[13..]);
            let seq = s.trim().parse::<u64>().ok()?;
            return Some(Self::TypingStart { seq });
        }

        if b.starts_with(b"TYPING_STOP:") {
            let s = String::from_utf8_lossy(&b[12..]);
            let seq = s.trim().parse::<u64>().ok()?;
            return Some(Self::TypingStop { seq });
        }

        None
    }

    // Helper: parse the new binary tagged format; returns None if not matching
    fn from_binary_tagged(b: &[u8]) -> Option<Self> {
        if b.is_empty() {
            return None;
        }
        let t = b[0];
        let mut cursor = 1usize;
        match t {
            0 => {
                if cursor + 1 > b.len() {
                    return None;
                }
                let version = b[cursor];
                Some(Self::Version { version })
            }
            1 => {
                if cursor + 4 > b.len() {
                    return None;
                }
                let len = u32::from_be_bytes(b[cursor..cursor + 4].try_into().ok()?) as usize;
                // Cap to 256 to prevent huge allocations (X25519 is 32 bytes)
                if len > 256 {
                    return None;
                }
                cursor += 4;
                if cursor + len > b.len() {
                    return None;
                }
                let public_key = b[cursor..cursor + len].to_vec();
                Some(Self::EphemeralKey { public_key })
            }
            9 => {
                if cursor + 1 > b.len() {
                    return None;
                }
                let count = b[cursor] as usize;
                cursor += 1;
                if count > 8 {
                    // Cap to prevent abuse (reasonable max of 8 signature schemes)
                    return None;
                }
                if cursor + count > b.len() {
                    return None;
                }
                let schemes = b[cursor..cursor + count].to_vec();
                Some(Self::SupportedSignatureSchemes { schemes })
            }
            2 => {
                if cursor + 8 + 8 + 4 > b.len() {
                    return None;
                }
                let seq = u64::from_be_bytes(b[cursor..cursor + 8].try_into().ok()?);
                cursor += 8;
                let timestamp = u64::from_be_bytes(b[cursor..cursor + 8].try_into().ok()?);
                cursor += 8;
                let len = u32::from_be_bytes(b[cursor..cursor + 4].try_into().ok()?) as usize;
                cursor += 4;
                if len > 64 * 1024 {
                    return None;
                }
                if cursor + len > b.len() {
                    return None;
                }
                let text = String::from_utf8_lossy(&b[cursor..cursor + len]).to_string();
                Some(Self::Text {
                    text,
                    timestamp,
                    seq,
                })
            }
            3 => {
                if cursor + 8 + 8 + 4 > b.len() {
                    return None;
                }
                let seq = u64::from_be_bytes(b[cursor..cursor + 8].try_into().ok()?);
                cursor += 8;
                let size = u64::from_be_bytes(b[cursor..cursor + 8].try_into().ok()?);
                if size > crate::MAX_FILE_SIZE {
                    return None;
                }
                cursor += 8;
                let fn_len = u32::from_be_bytes(b[cursor..cursor + 4].try_into().ok()?) as usize;
                cursor += 4;
                if cursor + fn_len > b.len() {
                    return None;
                }
                let raw_filename = String::from_utf8_lossy(&b[cursor..cursor + fn_len]).to_string();
                let filename = crate::util::sanitize_filename(&raw_filename);
                Some(Self::FileMeta {
                    filename,
                    size,
                    seq,
                })
            }
            4 => {
                if cursor + 8 + 4 > b.len() {
                    return None;
                }
                let seq = u64::from_be_bytes(b[cursor..cursor + 8].try_into().ok()?);
                cursor += 8;
                let len = u32::from_be_bytes(b[cursor..cursor + 4].try_into().ok()?) as usize;
                cursor += 4;
                if len > crate::FILE_CHUNK_SIZE {
                    return None;
                }
                if cursor + len > b.len() {
                    return None;
                }
                let chunk = b[cursor..cursor + len].to_vec();
                Some(Self::FileChunk { chunk, seq })
            }
            5 => {
                if cursor + 8 > b.len() {
                    return None;
                }
                let seq = u64::from_be_bytes(b[cursor..cursor + 8].try_into().ok()?);
                Some(Self::FileEnd { seq })
            }
            6 => {
                if cursor + 8 > b.len() {
                    return None;
                }
                let seq = u64::from_be_bytes(b[cursor..cursor + 8].try_into().ok()?);
                Some(Self::Ping { seq })
            }
            7 => {
                if cursor + 8 > b.len() {
                    return None;
                }
                let seq = u64::from_be_bytes(b[cursor..cursor + 8].try_into().ok()?);
                Some(Self::TypingStart { seq })
            }
            8 => {
                if cursor + 8 > b.len() {
                    return None;
                }
                let seq = u64::from_be_bytes(b[cursor..cursor + 8].try_into().ok()?);
                Some(Self::TypingStop { seq })
            }
            10 => {
                if cursor + 8 + 4 > b.len() {
                    return None;
                }
                let seq = u64::from_be_bytes(b[cursor..cursor + 8].try_into().ok()?);
                cursor += 8;
                let nonce_len = u32::from_be_bytes(b[cursor..cursor + 4].try_into().ok()?) as usize;
                cursor += 4;
                // Nonce must be at least 16 bytes for sufficient entropy and at most 256 bytes
                if !(16..=256).contains(&nonce_len) {
                    return None;
                }
                if cursor + nonce_len > b.len() {
                    return None;
                }
                let nonce = b[cursor..cursor + nonce_len].to_vec();
                Some(Self::Rekey { nonce, seq })
            }
            _ => None,
        }
    }
}
