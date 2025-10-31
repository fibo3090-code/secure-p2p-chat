use serde::{Deserialize, Serialize};

/// Protocol version for forward compatibility
pub const PROTOCOL_VERSION: u8 = 2;

/// Protocol messages exchanged between peers
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum ProtocolMessage {
    /// Protocol version announcement (first message)
    Version { version: u8 },

    /// Ephemeral X25519 public key for forward secrecy
    EphemeralKey { public_key: Vec<u8> },

    /// Text message
    Text { text: String, timestamp: u64 },

    /// File metadata (sent before chunks)
    FileMeta { filename: String, size: u64 },

    /// File data chunk
    FileChunk { chunk: Vec<u8>, seq: u64 },

    /// File transfer complete
    FileEnd,

    /// Keep-alive ping
    Ping,

    /// Typing indicator - user started typing
    TypingStart,

    /// Typing indicator - user stopped typing
    TypingStop,
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

            Self::Text { text, .. } => format!("TEXT:{}", text).into_bytes(),

            Self::FileMeta { filename, size } => {
                format!("FILE_META|{}|{}", filename, size).into_bytes()
            }

            Self::FileChunk { chunk, .. } => {
                let mut v = b"FILE_CHUNK:".to_vec();
                v.extend_from_slice(chunk);
                v
            }

            Self::FileEnd => b"FILE_END:".to_vec(),

            Self::Ping => b"PING".to_vec(),

            Self::TypingStart => b"TYPING_START".to_vec(),

            Self::TypingStop => b"TYPING_STOP".to_vec(),
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
            let text = String::from_utf8_lossy(&b[5..]).into_owned();
            Some(Self::Text {
                text,
                timestamp: crate::util::current_timestamp_millis(),
            })
        } else if b.starts_with(b"FILE_META|") {
            let s = String::from_utf8_lossy(b);
            let parts: Vec<&str> = s.splitn(3, '|').collect();
            if parts.len() == 3 {
                let filename = parts[1].to_string();
                if let Ok(size) = parts[2].parse::<u64>() {
                    return Some(Self::FileMeta { filename, size });
                }
            }
            None
        } else if b.starts_with(b"FILE_CHUNK:") {
            let chunk = b[11..].to_vec();
            Some(Self::FileChunk { chunk, seq: 0 })
        } else if b == b"FILE_END:" {
            Some(Self::FileEnd)
        } else if b == b"PING" {
            Some(Self::Ping)
        } else if b == b"TYPING_START" {
            Some(Self::TypingStart)
        } else if b == b"TYPING_STOP" {
            Some(Self::TypingStop)
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
        };

        let bytes = msg.to_plain_bytes();
        let parsed = ProtocolMessage::from_plain_bytes(&bytes).unwrap();

        match parsed {
            ProtocolMessage::Text { text, .. } => {
                assert_eq!(text, "Hello, world!");
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_file_meta_roundtrip() {
        let msg = ProtocolMessage::FileMeta {
            filename: "test.txt".to_string(),
            size: 12345,
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
        let msg = ProtocolMessage::FileEnd;
        let bytes = msg.to_plain_bytes();
        let parsed = ProtocolMessage::from_plain_bytes(&bytes).unwrap();

        assert_eq!(msg, parsed);
    }

    #[test]
    fn test_ping() {
        let msg = ProtocolMessage::Ping;
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
