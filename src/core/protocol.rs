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

    /// Text message (with sequence number for replay protection)
    Text { text: String, timestamp: u64, seq: u64 },

    /// File metadata (sent before chunks)
    FileMeta { filename: String, size: u64, seq: u64 },

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
}

impl std::fmt::Debug for ProtocolMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Version { version } => f.debug_struct("Version").field("version", version).finish(),
            Self::EphemeralKey { public_key } => f.debug_struct("EphemeralKey").field("public_key_len", &public_key.len()).finish(),
            Self::Text { seq, timestamp, .. } => f.debug_struct("Text")
                .field("seq", seq)
                .field("timestamp", timestamp)
                .field("text", &"***REDACTED***")
                .finish(),
            Self::FileMeta { filename, size, seq } => f.debug_struct("FileMeta")
                .field("seq", seq)
                .field("filename", filename)
                .field("size", size)
                .finish(),
            Self::FileChunk { seq, chunk } => f.debug_struct("FileChunk")
                .field("seq", seq)
                .field("chunk_len", &chunk.len())
                .finish(),
            Self::FileEnd { seq } => f.debug_struct("FileEnd").field("seq", seq).finish(),
            Self::Ping { seq } => f.debug_struct("Ping").field("seq", seq).finish(),
            Self::TypingStart { seq } => f.debug_struct("TypingStart").field("seq", seq).finish(),
            Self::TypingStop { seq } => f.debug_struct("TypingStop").field("seq", seq).finish(),
        }
    }
}

/// Identity Proof for Encrypted Handshake (Protocol v3)
/// Sent inside the encrypted tunnel to prove identity.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct IdentityProof {
    /// Public Key PEM (Identity)
    pub public_key_pem: String,
    /// Signature of the Ephemeral Public Key used in this session
    /// Signed by the Identity Private Key.
    /// Format: Sign(SHA256("IDENTITY_PROOF" || ephemeral_pubkey))
    pub signature: Vec<u8>,
    /// Protocol Version
    pub version: u32,
    /// Chat ID (optional here, or separate message)
    pub chat_id: uuid::Uuid,
}

impl ProtocolMessage {
    /// Convert message to plain bytes with ASCII prefixes
    pub fn to_plain_bytes(&self) -> Vec<u8> {
        match self {
            Self::Version { version } => format!("VERSION:{}", version).into_bytes(),

            Self::EphemeralKey { public_key } => {
                let mut v = b"EPHEMERAL_KEY:".to_vec();
                v.extend_from_slice(public_key);
                v
            }

            Self::Text { text, seq, .. } => format!("TEXT:{}:{}", seq, text).into_bytes(),

            Self::FileMeta { filename, size, seq } => {
                format!("FILE_META|{}|{}|{}", seq, filename, size).into_bytes()
            }

            Self::FileChunk { chunk, .. } => {
                let mut v = b"FILE_CHUNK:".to_vec();
                v.extend_from_slice(chunk);
                v
            }

            Self::FileEnd { seq } => format!("FILE_END:{}", seq).into_bytes(),

            Self::Ping { seq } => format!("PING:{}", seq).into_bytes(),

            Self::TypingStart { seq } => format!("TYPING_START:{}", seq).into_bytes(),

            Self::TypingStop { seq } => format!("TYPING_STOP:{}", seq).into_bytes(),
        }
    }

    /// Parse message from plain bytes with ASCII prefixes
    pub fn from_plain_bytes(b: &[u8]) -> Option<Self> {
        if b.starts_with(b"VERSION:") {
            let version_str = String::from_utf8_lossy(&b[8..]);
            if let Ok(version) = version_str.trim().parse::<u8>() {
                return Some(Self::Version { version });
            }
            None
        } else if b.starts_with(b"EPHEMERAL_KEY:") {
            let public_key = b[14..].to_vec();
            Some(Self::EphemeralKey { public_key })
        } else if b.starts_with(b"TEXT:") {
            // Security: Enforce a limit on text messages to prevent memory exhaustion
            if b.len() > 64 * 1024 {
                // 64 KiB Limit
                return None; // Invalid/Too large
            }
            let s = String::from_utf8_lossy(&b[5..]);
            let parts: Vec<&str> = s.splitn(2, ':').collect();
            if parts.len() == 2 {
                let seq = parts[0].parse::<u64>().ok()?;
                let text = parts[1].to_string();
                Some(Self::Text {
                    text,
                    timestamp: crate::util::current_timestamp_millis(),
                    seq,
                })
            } else {
                None
            }
        } else if b.starts_with(b"FILE_META|") {
            let s = String::from_utf8_lossy(b);
            let parts: Vec<&str> = s.splitn(4, '|').collect();
            if parts.len() == 4 {
                let seq = parts[1].parse::<u64>().ok()?;
                let raw_filename = parts[2];
                let filename = crate::util::sanitize_filename(raw_filename);
                if let Ok(size) = parts[3].parse::<u64>() {
                    return Some(Self::FileMeta { filename, size, seq });
                }
            }
            None
        } else if b.starts_with(b"FILE_CHUNK:") {
            let chunk = b[11..].to_vec();
            Some(Self::FileChunk { chunk, seq: 0 })
        } else if b.starts_with(b"FILE_END:") {
            let s = String::from_utf8_lossy(&b[9..]);
            let seq = s.trim().parse::<u64>().ok()?;
            Some(Self::FileEnd { seq })
        } else if b.starts_with(b"PING:") {
            let s = String::from_utf8_lossy(&b[5..]);
            let seq = s.trim().parse::<u64>().ok()?;
            Some(Self::Ping { seq })
        } else if b.starts_with(b"TYPING_START:") {
            let s = String::from_utf8_lossy(&b[13..]);
            let seq = s.trim().parse::<u64>().ok()?;
            Some(Self::TypingStart { seq })
        } else if b.starts_with(b"TYPING_STOP:") {
            let s = String::from_utf8_lossy(&b[12..]);
            let seq = s.trim().parse::<u64>().ok()?;
            Some(Self::TypingStop { seq })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_message_roundtrip() {
        let msg = ProtocolMessage::Text {
            text: "Hello, world!".to_string(),
            timestamp: 1234567890,
            seq: 42,
        };

        let bytes = msg.to_plain_bytes();
        let parsed = ProtocolMessage::from_plain_bytes(&bytes).unwrap();

        match parsed {
            ProtocolMessage::Text { text, seq, .. } => {
                assert_eq!(text, "Hello, world!");
                assert_eq!(seq, 42);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_file_meta_roundtrip() {
        let msg = ProtocolMessage::FileMeta {
            filename: "test.txt".to_string(),
            size: 12345,
            seq: 1,
        };

        let bytes = msg.to_plain_bytes();
        let parsed = ProtocolMessage::from_plain_bytes(&bytes).unwrap();

        assert_eq!(msg, parsed);
    }

    #[test]
    fn test_file_chunk_roundtrip() {
        let chunk_data = vec![1, 2, 3, 4, 5];
        let msg = ProtocolMessage::FileChunk {
            chunk: chunk_data.clone(),
            seq: 0,
        };

        let bytes = msg.to_plain_bytes();
        let parsed = ProtocolMessage::from_plain_bytes(&bytes).unwrap();

        match parsed {
            ProtocolMessage::FileChunk { chunk, .. } => {
                assert_eq!(chunk, chunk_data);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_file_end() {
        let msg = ProtocolMessage::FileEnd { seq: 100 };
        let bytes = msg.to_plain_bytes();
        let parsed = ProtocolMessage::from_plain_bytes(&bytes).unwrap();

        assert_eq!(msg, parsed);
    }

    #[test]
    fn test_ping() {
        let msg = ProtocolMessage::Ping { seq: 5 };
        let bytes = msg.to_plain_bytes();
        let parsed = ProtocolMessage::from_plain_bytes(&bytes).unwrap();

        assert_eq!(msg, parsed);
    }

    #[test]
    fn test_invalid_message() {
        let invalid = b"INVALID:data";
        let parsed = ProtocolMessage::from_plain_bytes(invalid);

        assert!(parsed.is_none());
    }
}
