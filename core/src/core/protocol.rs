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

    /// Chunk of a large text message, reassembled by the app layer.
    TextChunk {
        message_id: uuid::Uuid,
        chunk_index: u32,
        total_chunks: u32,
        text_part: String,
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
            Self::TextChunk {
                message_id,
                chunk_index,
                total_chunks,
                timestamp,
                seq,
                ..
            } => f
                .debug_struct("TextChunk")
                .field("message_id", message_id)
                .field("chunk_index", chunk_index)
                .field("total_chunks", total_chunks)
                .field("seq", seq)
                .field("timestamp", timestamp)
                .field("text_part", &"***REDACTED***")
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
    /// Overwrite the transport sequence number on variants that carry one.
    ///
    /// The session message loop owns a single monotonic outgoing counter and
    /// stamps it onto every frame it sends (application messages *and* `Rekey`),
    /// so the receiver's replay check sees one strictly-increasing stream.
    /// Handshake-only variants have no sequence number and are left unchanged.
    pub fn set_seq(&mut self, new_seq: u64) {
        match self {
            Self::Text { seq, .. }
            | Self::TextChunk { seq, .. }
            | Self::FileMeta { seq, .. }
            | Self::FileChunk { seq, .. }
            | Self::FileEnd { seq }
            | Self::Ping { seq }
            | Self::TypingStart { seq }
            | Self::TypingStop { seq }
            | Self::Rekey { seq, .. } => *seq = new_seq,
            Self::Version { .. }
            | Self::EphemeralKey { .. }
            | Self::SupportedSignatureSchemes { .. } => {}
        }
    }

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

            Self::TextChunk {
                message_id,
                chunk_index,
                total_chunks,
                text_part,
                timestamp,
                seq,
            } => {
                v.push(11u8);
                v.extend_from_slice(message_id.as_bytes());
                v.extend_from_slice(&seq.to_be_bytes());
                v.extend_from_slice(&timestamp.to_be_bytes());
                v.extend_from_slice(&chunk_index.to_be_bytes());
                v.extend_from_slice(&total_chunks.to_be_bytes());
                let bytes = text_part.as_bytes();
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
            let public_key_bytes = &b[14..];
            if public_key_bytes.len() != 32 {
                return None;
            }
            let public_key = public_key_bytes.to_vec();
            return Some(Self::EphemeralKey { public_key });
        }

        if b.starts_with(b"TEXT:") {
            if b.len() > crate::MAX_TEXT_MESSAGE_BYTES {
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
                if len > crate::MAX_TEXT_MESSAGE_BYTES {
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
            11 => {
                if cursor + 16 + 8 + 8 + 4 + 4 + 4 > b.len() {
                    return None;
                }
                let message_id = uuid::Uuid::from_slice(&b[cursor..cursor + 16]).ok()?;
                cursor += 16;
                let seq = u64::from_be_bytes(b[cursor..cursor + 8].try_into().ok()?);
                cursor += 8;
                let timestamp = u64::from_be_bytes(b[cursor..cursor + 8].try_into().ok()?);
                cursor += 8;
                let chunk_index = u32::from_be_bytes(b[cursor..cursor + 4].try_into().ok()?);
                cursor += 4;
                let total_chunks = u32::from_be_bytes(b[cursor..cursor + 4].try_into().ok()?);
                cursor += 4;
                // Reject a huge `total_chunks` here: the reassembler allocates one
                // slot per chunk up front, so an unbounded value is a remote-OOM
                // vector. A capped, non-zero, in-range index is required.
                if total_chunks == 0
                    || total_chunks > crate::MAX_TEXT_CHUNKS
                    || chunk_index >= total_chunks
                {
                    return None;
                }
                let len = u32::from_be_bytes(b[cursor..cursor + 4].try_into().ok()?) as usize;
                cursor += 4;
                if len > crate::TEXT_CHUNK_BYTES {
                    return None;
                }
                if cursor + len > b.len() {
                    return None;
                }
                let text_part = String::from_utf8_lossy(&b[cursor..cursor + len]).to_string();
                Some(Self::TextChunk {
                    message_id,
                    chunk_index,
                    total_chunks,
                    text_part,
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

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    /// Encode then decode must yield an identical message for every variant.
    fn assert_roundtrip(msg: ProtocolMessage) {
        let bytes = msg.to_plain_bytes();
        let decoded = ProtocolMessage::from_plain_bytes(&bytes)
            .unwrap_or_else(|| panic!("decode failed for {:?}", msg));
        assert_eq!(decoded, msg, "round-trip mismatch for {:?}", msg);
    }

    #[test]
    fn roundtrip_all_variants_representative() {
        assert_roundtrip(ProtocolMessage::Version { version: 3 });
        assert_roundtrip(ProtocolMessage::EphemeralKey {
            public_key: vec![7u8; 32],
        });
        assert_roundtrip(ProtocolMessage::SupportedSignatureSchemes {
            schemes: vec![
                SignatureScheme::RsaPss.to_u8(),
                SignatureScheme::Ed25519.to_u8(),
            ],
        });
        assert_roundtrip(ProtocolMessage::Text {
            text: "hello world".to_string(),
            timestamp: 1_700_000_000_000,
            seq: 42,
        });
        assert_roundtrip(ProtocolMessage::TextChunk {
            message_id: Uuid::new_v4(),
            chunk_index: 2,
            total_chunks: 5,
            text_part: "part".to_string(),
            timestamp: 123,
            seq: 9,
        });
        assert_roundtrip(ProtocolMessage::FileMeta {
            filename: "report.pdf".to_string(),
            size: 1024,
            seq: 1,
        });
        assert_roundtrip(ProtocolMessage::FileChunk {
            chunk: vec![1, 2, 3, 4, 5],
            seq: 2,
        });
        assert_roundtrip(ProtocolMessage::FileEnd { seq: 3 });
        assert_roundtrip(ProtocolMessage::Ping { seq: 4 });
        assert_roundtrip(ProtocolMessage::TypingStart { seq: 5 });
        assert_roundtrip(ProtocolMessage::TypingStop { seq: 6 });
        assert_roundtrip(ProtocolMessage::Rekey {
            nonce: vec![0xAB; 16],
            seq: 7,
        });
    }

    #[test]
    fn roundtrip_edge_values() {
        // Empty and unicode text.
        assert_roundtrip(ProtocolMessage::Text {
            text: String::new(),
            timestamp: 0,
            seq: 0,
        });
        assert_roundtrip(ProtocolMessage::Text {
            text: "héllo 🌍 سلام こんにちは".to_string(),
            timestamp: u64::MAX,
            seq: u64::MAX,
        });
        // Empty file chunk and a max-size chunk.
        assert_roundtrip(ProtocolMessage::FileChunk {
            chunk: Vec::new(),
            seq: 1,
        });
        assert_roundtrip(ProtocolMessage::FileChunk {
            chunk: vec![0xCD; crate::FILE_CHUNK_SIZE],
            seq: 2,
        });
        // Largest permitted text chunk part.
        assert_roundtrip(ProtocolMessage::TextChunk {
            message_id: Uuid::nil(),
            chunk_index: 0,
            total_chunks: 1,
            text_part: "x".repeat(crate::TEXT_CHUNK_BYTES),
            timestamp: 1,
            seq: 1,
        });
        // File at exactly the maximum allowed size.
        assert_roundtrip(ProtocolMessage::FileMeta {
            filename: "big.bin".to_string(),
            size: crate::MAX_FILE_SIZE,
            seq: 1,
        });
    }

    #[test]
    fn filemeta_sanitizes_path_traversal_on_decode() {
        let msg = ProtocolMessage::FileMeta {
            filename: "../../etc/passwd".to_string(),
            size: 10,
            seq: 1,
        };
        let decoded = ProtocolMessage::from_plain_bytes(&msg.to_plain_bytes()).unwrap();
        match decoded {
            ProtocolMessage::FileMeta { filename, .. } => {
                assert!(!filename.contains(".."));
                assert!(!filename.contains('/'));
            }
            other => panic!("expected FileMeta, got {:?}", other),
        }
    }

    #[test]
    fn malformed_inputs_return_none() {
        // Empty buffer.
        assert!(ProtocolMessage::from_plain_bytes(&[]).is_none());
        // Truncated Text (tag present, missing length-prefixed payload).
        assert!(ProtocolMessage::from_plain_bytes(&[2u8, 0, 0]).is_none());
        // Unknown tag with no legacy prefix.
        assert!(ProtocolMessage::from_plain_bytes(&[200u8, 1, 2, 3]).is_none());
        // Truncated FileEnd (needs 8 bytes of seq).
        assert!(ProtocolMessage::from_plain_bytes(&[5u8, 0, 0]).is_none());
    }

    #[test]
    fn oversized_payloads_are_rejected() {
        // FileMeta with size above MAX_FILE_SIZE must not decode.
        let bad = ProtocolMessage::FileMeta {
            filename: "x".to_string(),
            size: crate::MAX_FILE_SIZE + 1,
            seq: 1,
        };
        assert!(ProtocolMessage::from_plain_bytes(&bad.to_plain_bytes()).is_none());

        // FileChunk larger than FILE_CHUNK_SIZE must not decode.
        let bad_chunk = ProtocolMessage::FileChunk {
            chunk: vec![0u8; crate::FILE_CHUNK_SIZE + 1],
            seq: 1,
        };
        assert!(ProtocolMessage::from_plain_bytes(&bad_chunk.to_plain_bytes()).is_none());

        // EphemeralKey longer than the 256-byte cap must not decode.
        let bad_eph = ProtocolMessage::EphemeralKey {
            public_key: vec![0u8; 300],
        };
        assert!(ProtocolMessage::from_plain_bytes(&bad_eph.to_plain_bytes()).is_none());
    }

    #[test]
    fn textchunk_invariants_enforced_on_decode() {
        // total_chunks == 0 is invalid.
        let zero_total = ProtocolMessage::TextChunk {
            message_id: Uuid::nil(),
            chunk_index: 0,
            total_chunks: 0,
            text_part: "x".to_string(),
            timestamp: 1,
            seq: 1,
        };
        assert!(ProtocolMessage::from_plain_bytes(&zero_total.to_plain_bytes()).is_none());

        // chunk_index >= total_chunks is invalid.
        let oob = ProtocolMessage::TextChunk {
            message_id: Uuid::nil(),
            chunk_index: 3,
            total_chunks: 3,
            text_part: "x".to_string(),
            timestamp: 1,
            seq: 1,
        };
        assert!(ProtocolMessage::from_plain_bytes(&oob.to_plain_bytes()).is_none());
    }

    #[test]
    fn textchunk_total_chunks_cap_enforced_on_decode() {
        // A frame whose total_chunks exceeds the cap must be rejected before it
        // can drive a giant reassembly-buffer allocation (remote-OOM guard).
        let over = ProtocolMessage::TextChunk {
            message_id: Uuid::nil(),
            chunk_index: 0,
            total_chunks: crate::MAX_TEXT_CHUNKS + 1,
            text_part: "x".to_string(),
            timestamp: 1,
            seq: 1,
        };
        assert!(ProtocolMessage::from_plain_bytes(&over.to_plain_bytes()).is_none());

        // The exact cap is still allowed.
        let at_cap = ProtocolMessage::TextChunk {
            message_id: Uuid::nil(),
            chunk_index: 0,
            total_chunks: crate::MAX_TEXT_CHUNKS,
            text_part: "x".to_string(),
            timestamp: 1,
            seq: 1,
        };
        assert!(ProtocolMessage::from_plain_bytes(&at_cap.to_plain_bytes()).is_some());

        // A pathological u32::MAX (the original OOM vector) is rejected.
        let huge = ProtocolMessage::TextChunk {
            message_id: Uuid::nil(),
            chunk_index: 0,
            total_chunks: u32::MAX,
            text_part: "x".to_string(),
            timestamp: 1,
            seq: 1,
        };
        assert!(ProtocolMessage::from_plain_bytes(&huge.to_plain_bytes()).is_none());
    }

    #[test]
    fn legacy_ascii_formats_still_parse() {
        assert_eq!(
            ProtocolMessage::from_plain_bytes(b"VERSION:3"),
            Some(ProtocolMessage::Version { version: 3 })
        );
        match ProtocolMessage::from_plain_bytes(b"TEXT:5:hi there") {
            Some(ProtocolMessage::Text { text, seq, .. }) => {
                assert_eq!(text, "hi there");
                assert_eq!(seq, 5);
            }
            other => panic!("expected legacy Text, got {:?}", other),
        }
        assert_eq!(
            ProtocolMessage::from_plain_bytes(b"PING:7"),
            Some(ProtocolMessage::Ping { seq: 7 })
        );
        assert_eq!(
            ProtocolMessage::from_plain_bytes(b"FILE_END:9"),
            Some(ProtocolMessage::FileEnd { seq: 9 })
        );
    }

    #[test]
    fn identity_proof_serde_roundtrip_and_default_scheme() {
        let proof = IdentityProof {
            public_key_pem: "-----BEGIN PUBLIC KEY-----\nabc\n-----END PUBLIC KEY-----".to_string(),
            signature: vec![9, 8, 7, 6],
            version: PROTOCOL_VERSION as u32,
            chat_id: Uuid::new_v4(),
            signature_scheme: SignatureScheme::RsaPss,
        };
        let bytes = bincode::serialize(&proof).unwrap();
        let decoded: IdentityProof = bincode::deserialize(&bytes).unwrap();
        assert_eq!(decoded.public_key_pem, proof.public_key_pem);
        assert_eq!(decoded.signature, proof.signature);
        assert_eq!(decoded.version, proof.version);
        assert_eq!(decoded.chat_id, proof.chat_id);
        assert_eq!(decoded.signature_scheme, proof.signature_scheme);

        // JSON without a signature_scheme field falls back to the RSA-PSS default.
        let json = format!(
            r#"{{"public_key_pem":"k","signature":[1,2],"version":3,"chat_id":"{}"}}"#,
            Uuid::nil()
        );
        let decoded: IdentityProof = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.signature_scheme, SignatureScheme::RsaPss);
    }

    #[test]
    fn debug_redacts_message_contents() {
        let msg = ProtocolMessage::Text {
            text: "super-secret".to_string(),
            timestamp: 0,
            seq: 1,
        };
        let rendered = format!("{:?}", msg);
        assert!(!rendered.contains("super-secret"));
        assert!(rendered.contains("REDACTED"));
    }
}
