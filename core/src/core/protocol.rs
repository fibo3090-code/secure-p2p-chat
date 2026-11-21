use super::SignatureScheme;
use serde::{Deserialize, Serialize};

/// Protocol version. v3: ECDH-first handshake; v4 adds the ephemeral-key commitment that
/// makes the SAS sound (audit CRYPTO-02). v3 peers are still accepted, with a warning.
pub const PROTOCOL_VERSION: u8 = 4;

/// Protocol messages exchanged between peers
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub enum ProtocolMessage {
    /// Protocol version announcement (first message)
    Version { version: u8 },

    /// Ephemeral X25519 public key for forward secrecy
    EphemeralKey { public_key: Vec<u8> },

    /// Supported signature schemes, sent before the identity proof.
    /// Encoded as: u8 count | [u8 scheme]*
    SupportedSignatureSchemes { schemes: Vec<u8> },

    /// Text message
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

    /// Abort the in-flight file transfer on this chat; sent by either side.
    FileCancel { seq: u64 },

    /// Keep-alive ping
    Ping { seq: u64 },

    /// Typing indicator: started
    TypingStart { seq: u64 },

    /// Typing indicator: stopped
    TypingStop { seq: u64 },

    /// Session key rotation: both peers derive the next key as
    /// `rekey_session_key(current_key, nonce)` and use it for all later frames.
    Rekey { nonce: Vec<u8>, seq: u64 },

    /// Delivery receipt for the peer's frame with transport sequence `acked_seq`; `seq`
    /// is this frame's own. Older peers drop the unknown tag (seq gaps are tolerated).
    Ack { acked_seq: u64, seq: u64 },
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
            Self::FileCancel { seq } => f.debug_struct("FileCancel").field("seq", seq).finish(),
            Self::Ping { seq } => f.debug_struct("Ping").field("seq", seq).finish(),
            Self::TypingStart { seq } => f.debug_struct("TypingStart").field("seq", seq).finish(),
            Self::TypingStop { seq } => f.debug_struct("TypingStop").field("seq", seq).finish(),
            Self::Rekey { nonce, seq } => f
                .debug_struct("Rekey")
                .field("seq", seq)
                .field("nonce_len", &nonce.len())
                .finish(),
            Self::Ack { acked_seq, seq } => f
                .debug_struct("Ack")
                .field("seq", seq)
                .field("acked_seq", acked_seq)
                .finish(),
        }
    }
}

/// Identity proof, sent inside the encrypted tunnel.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct IdentityProof {
    /// Identity public key PEM (RSA)
    pub public_key_pem: String,
    /// Signature over SHA256("IDENTITY_PROOF" || ephemeral_pubkey) by the identity key
    pub signature: Vec<u8>,
    pub version: u32,
    pub chat_id: uuid::Uuid,
    /// Signature scheme used; defaults to RSA-PSS for v3.0 compatibility.
    ///
    /// ⚠️ Never add fields here expecting `#[serde(default)]` to keep older peers
    /// reading it: bincode decodes positionally and they fail with "unexpected end of
    /// file". Scheme-specific material belongs inside [`Self::signature`] (see
    /// [`Ed25519Proof`]).
    #[serde(default = "default_signature_scheme")]
    pub signature_scheme: SignatureScheme,
}

/// What [`IdentityProof::signature`] carries under Ed25519: the signature, the subkey that
/// made it, and that subkey's binding to the RSA identity. Packed into `signature` so
/// `IdentityProof` stays byte-identical for RSA-PSS-only peers.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Ed25519Proof {
    /// Ed25519 signature over the identity-proof digest (64 bytes).
    pub signature: Vec<u8>,
    /// The Ed25519 signing subkey (32 bytes).
    pub public: Vec<u8>,
    /// RSA-PSS signature by the identity over the subkey; without it the subkey is not
    /// an identity and a replayed `public_key_pem` could pair with any signing key.
    pub binding: Vec<u8>,
}

fn default_signature_scheme() -> SignatureScheme {
    SignatureScheme::RsaPss
}

/// The longest decimal `u64`, which bounds every legacy numeric field before it is
/// decoded (so `PING:` plus 8 MiB of junk cannot allocate before the parse fails).
const MAX_LEGACY_NUMBER_BYTES: usize = 20;

/// Decode a peer-supplied field as UTF-8, or refuse the frame. Never lossily: every cap
/// here counts wire bytes and U+FFFD triples each invalid byte, and lossy decoding
/// accepted frames the encoder cannot reproduce. A conforming peer only ever sends
/// UTF-8 (every field comes from a Rust `String`).
fn decode_text(b: &[u8]) -> Option<&str> {
    std::str::from_utf8(b).ok()
}

/// Decode a legacy arm's trailing decimal field, bounded before it is decoded.
fn decode_number_field(b: &[u8]) -> Option<&str> {
    if b.len() > MAX_LEGACY_NUMBER_BYTES {
        return None;
    }
    decode_text(b)
}

/// A bounds-checked cursor over one binary frame: every read yields exactly the bytes
/// asked for or `None`, never panics. Trailing bytes after the last field are left
/// unread, as the tagged format always has.
struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    fn new(frame: &'a [u8]) -> Self {
        Self { rest: frame }
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if n > self.rest.len() {
            return None;
        }
        let (head, tail) = self.rest.split_at(n);
        self.rest = tail;
        Some(head)
    }

    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }

    fn u8(&mut self) -> Option<u8> {
        self.array::<1>().map(|[b]| b)
    }

    fn u32(&mut self) -> Option<u32> {
        self.array().map(u32::from_be_bytes)
    }

    fn u64(&mut self) -> Option<u64> {
        self.array().map(u64::from_be_bytes)
    }

    /// A `u32` length prefix and the body it announces; the length is checked against
    /// `allowed` before the body is sliced.
    fn len_prefixed(&mut self, allowed: std::ops::RangeInclusive<usize>) -> Option<&'a [u8]> {
        let len = self.u32()? as usize;
        if !allowed.contains(&len) {
            return None;
        }
        self.take(len)
    }
}

/// Append `bytes` behind a big-endian `u32` length prefix (encoder half of
/// [`Reader::len_prefixed`]).
fn put_len_prefixed(v: &mut Vec<u8>, bytes: &[u8]) {
    v.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    v.extend_from_slice(bytes);
}

/// Parse a legacy ASCII arm whose payload is a single decimal number.
fn legacy_number<T: std::str::FromStr>(field: &[u8]) -> Option<T> {
    decode_number_field(field)?.trim().parse().ok()
}

impl ProtocolMessage {
    /// The transport sequence number; handshake-only variants have none.
    pub fn seq(&self) -> Option<u64> {
        match self {
            Self::Text { seq, .. }
            | Self::TextChunk { seq, .. }
            | Self::FileMeta { seq, .. }
            | Self::FileChunk { seq, .. }
            | Self::FileEnd { seq }
            | Self::FileCancel { seq }
            | Self::Ping { seq }
            | Self::TypingStart { seq }
            | Self::TypingStop { seq }
            | Self::Rekey { seq, .. }
            | Self::Ack { seq, .. } => Some(*seq),
            Self::Version { .. }
            | Self::EphemeralKey { .. }
            | Self::SupportedSignatureSchemes { .. } => None,
        }
    }

    /// Overwrite the transport sequence number on variants that carry one. The message
    /// loop stamps one monotonic counter onto every frame it sends (`Rekey` included).
    pub fn set_seq(&mut self, new_seq: u64) {
        match self {
            Self::Text { seq, .. }
            | Self::TextChunk { seq, .. }
            | Self::FileMeta { seq, .. }
            | Self::FileChunk { seq, .. }
            | Self::FileEnd { seq }
            | Self::FileCancel { seq }
            | Self::Ping { seq }
            | Self::TypingStart { seq }
            | Self::TypingStop { seq }
            | Self::Rekey { seq, .. }
            | Self::Ack { seq, .. } => *seq = new_seq,
            Self::Version { .. }
            | Self::EphemeralKey { .. }
            | Self::SupportedSignatureSchemes { .. } => {}
        }
    }

    /// Encode as `[type: u8][payload...]`, with big-endian integers and length-prefixed
    /// blobs.
    pub fn to_plain_bytes(&self) -> Vec<u8> {
        let mut v = Vec::new();
        match self {
            Self::Version { version } => {
                v.push(0u8);
                v.push(*version);
            }
            Self::EphemeralKey { public_key } => {
                v.push(1u8);
                put_len_prefixed(&mut v, public_key);
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
                put_len_prefixed(&mut v, text.as_bytes());
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
                put_len_prefixed(&mut v, text_part.as_bytes());
            }
            Self::FileMeta {
                filename,
                size,
                seq,
            } => {
                v.push(3u8);
                v.extend_from_slice(&seq.to_be_bytes());
                v.extend_from_slice(&size.to_be_bytes());
                put_len_prefixed(&mut v, filename.as_bytes());
            }
            Self::FileChunk { chunk, seq } => {
                v.push(4u8);
                v.extend_from_slice(&seq.to_be_bytes());
                put_len_prefixed(&mut v, chunk);
            }
            Self::FileEnd { seq } => {
                v.push(5u8);
                v.extend_from_slice(&seq.to_be_bytes());
            }
            Self::FileCancel { seq } => {
                v.push(12u8);
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
                put_len_prefixed(&mut v, nonce);
            }
            Self::Ack { acked_seq, seq } => {
                v.push(13u8);
                v.extend_from_slice(&seq.to_be_bytes());
                v.extend_from_slice(&acked_seq.to_be_bytes());
            }
        }
        v
    }

    /// Parse the binary tagged format, falling back to the legacy ASCII prefixes.
    pub fn from_plain_bytes(b: &[u8]) -> Option<Self> {
        if let Some(bin) = Self::from_binary_tagged(b) {
            return Some(bin);
        }
        // Legacy ASCII format for older peers and tests; kept limited and defensive.
        Self::from_legacy_ascii(b)
    }

    /// The pre-binary ASCII format. No prefix is a prefix of another, so the first match
    /// decides the frame.
    fn from_legacy_ascii(b: &[u8]) -> Option<Self> {
        if let Some(rest) = b.strip_prefix(b"VERSION:") {
            return legacy_number(rest).map(|version| Self::Version { version });
        }

        if let Some(public_key) = b.strip_prefix(b"EPHEMERAL_KEY:") {
            if public_key.len() != 32 {
                return None;
            }
            return Some(Self::EphemeralKey {
                public_key: public_key.to_vec(),
            });
        }

        if let Some(rest) = b.strip_prefix(b"TEXT:") {
            if b.len() > crate::MAX_TEXT_MESSAGE_BYTES {
                return None;
            }
            let (seq, text) = decode_text(rest)?.split_once(':')?;
            return Some(Self::Text {
                text: text.to_string(),
                timestamp: crate::util::current_timestamp_millis(),
                seq: seq.parse().ok()?,
            });
        }

        if let Some(rest) = b.strip_prefix(b"FILE_META|") {
            // The filename is kept, so it is decoded strictly (see `decode_text`).
            let mut parts = decode_text(rest)?.splitn(3, '|');
            let (seq, raw_filename, size) = (parts.next()?, parts.next()?, parts.next()?);
            let seq = seq.parse::<u64>().ok()?;
            let size = size.parse::<u64>().ok()?;
            if size > crate::MAX_FILE_SIZE {
                return None;
            }
            return Some(Self::FileMeta {
                filename: crate::util::sanitize_filename(raw_filename),
                size,
                seq,
            });
        }

        if let Some(rest) = b.strip_prefix(b"FILE_CHUNK:") {
            let mut parts = rest.splitn(2, |&c| c == b':');
            let seq_bytes = parts.next()?;
            let chunk = parts.next()?;
            let seq = std::str::from_utf8(seq_bytes).ok()?.parse::<u64>().ok()?;
            if chunk.len() > crate::FILE_CHUNK_SIZE {
                return None;
            }
            return Some(Self::FileChunk {
                chunk: chunk.to_vec(),
                seq,
            });
        }

        if let Some(rest) = b.strip_prefix(b"FILE_END:") {
            return legacy_number(rest).map(|seq| Self::FileEnd { seq });
        }
        if let Some(rest) = b.strip_prefix(b"PING:") {
            return legacy_number(rest).map(|seq| Self::Ping { seq });
        }
        if let Some(rest) = b.strip_prefix(b"TYPING_START:") {
            return legacy_number(rest).map(|seq| Self::TypingStart { seq });
        }
        if let Some(rest) = b.strip_prefix(b"TYPING_STOP:") {
            return legacy_number(rest).map(|seq| Self::TypingStop { seq });
        }

        None
    }

    /// Parse the binary tagged format; `None` if the frame is not a valid one.
    fn from_binary_tagged(b: &[u8]) -> Option<Self> {
        let mut r = Reader::new(b);
        let msg = match r.u8()? {
            0 => Self::Version { version: r.u8()? },
            // Capped at 256 to prevent huge allocations (X25519 is 32 bytes).
            1 => Self::EphemeralKey {
                public_key: r.len_prefixed(0..=256)?.to_vec(),
            },
            9 => {
                // At most 8 signature schemes: there are two today.
                let count = r.u8()? as usize;
                if count > 8 {
                    return None;
                }
                Self::SupportedSignatureSchemes {
                    schemes: r.take(count)?.to_vec(),
                }
            }
            2 => {
                let seq = r.u64()?;
                let timestamp = r.u64()?;
                let text = r.len_prefixed(0..=crate::MAX_TEXT_MESSAGE_BYTES)?;
                Self::Text {
                    text: decode_text(text)?.to_string(),
                    timestamp,
                    seq,
                }
            }
            11 => {
                let message_id = uuid::Uuid::from_bytes(r.array()?);
                let seq = r.u64()?;
                let timestamp = r.u64()?;
                let chunk_index = r.u32()?;
                let total_chunks = r.u32()?;
                // The reassembler allocates one slot per chunk up front, so cap
                // `total_chunks` (remote-OOM guard) and require an in-range index.
                if total_chunks == 0
                    || total_chunks > crate::MAX_TEXT_CHUNKS
                    || chunk_index >= total_chunks
                {
                    return None;
                }
                let text_part = r.len_prefixed(0..=crate::TEXT_CHUNK_BYTES)?;
                Self::TextChunk {
                    message_id,
                    chunk_index,
                    total_chunks,
                    text_part: decode_text(text_part)?.to_string(),
                    timestamp,
                    seq,
                }
            }
            3 => {
                let seq = r.u64()?;
                let size = r.u64()?;
                if size > crate::MAX_FILE_SIZE {
                    return None;
                }
                // No cap of its own: the frame (`MAX_PACKET_SIZE`) bounds it, and
                // `sanitize_filename` bounds what is kept.
                let raw_filename = decode_text(r.len_prefixed(0..=usize::MAX)?)?;
                Self::FileMeta {
                    filename: crate::util::sanitize_filename(raw_filename),
                    size,
                    seq,
                }
            }
            4 => {
                let seq = r.u64()?;
                let chunk = r.len_prefixed(0..=crate::FILE_CHUNK_SIZE)?.to_vec();
                Self::FileChunk { chunk, seq }
            }
            5 => Self::FileEnd { seq: r.u64()? },
            12 => Self::FileCancel { seq: r.u64()? },
            6 => Self::Ping { seq: r.u64()? },
            7 => Self::TypingStart { seq: r.u64()? },
            8 => Self::TypingStop { seq: r.u64()? },
            10 => {
                let seq = r.u64()?;
                // At least 16 bytes of entropy, at most 256.
                let nonce = r.len_prefixed(16..=256)?.to_vec();
                Self::Rekey { nonce, seq }
            }
            13 => {
                let seq = r.u64()?;
                let acked_seq = r.u64()?;
                Self::Ack { acked_seq, seq }
            }
            _ => return None,
        };
        Some(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn decode(msg: &ProtocolMessage) -> Option<ProtocolMessage> {
        ProtocolMessage::from_plain_bytes(&msg.to_plain_bytes())
    }

    /// Encode then decode must yield an identical message.
    fn assert_roundtrip(msg: ProtocolMessage) {
        let decoded = decode(&msg).unwrap_or_else(|| panic!("decode failed for {:?}", msg));
        assert_eq!(decoded, msg, "round-trip mismatch for {:?}", msg);
    }

    fn text_chunk(chunk_index: u32, total_chunks: u32, text_part: String) -> ProtocolMessage {
        ProtocolMessage::TextChunk {
            message_id: Uuid::nil(),
            chunk_index,
            total_chunks,
            text_part,
            timestamp: 1,
            seq: 1,
        }
    }

    fn file_meta(filename: &str, size: u64) -> ProtocolMessage {
        ProtocolMessage::FileMeta {
            filename: filename.to_string(),
            size,
            seq: 1,
        }
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
        assert_roundtrip(file_meta("report.pdf", 1024));
        assert_roundtrip(ProtocolMessage::FileChunk {
            chunk: vec![1, 2, 3, 4, 5],
            seq: 2,
        });
        assert_roundtrip(ProtocolMessage::FileEnd { seq: 3 });
        assert_roundtrip(ProtocolMessage::FileCancel { seq: 4 });
        assert_roundtrip(ProtocolMessage::Ping { seq: 4 });
        assert_roundtrip(ProtocolMessage::TypingStart { seq: 5 });
        assert_roundtrip(ProtocolMessage::TypingStop { seq: 6 });
        assert_roundtrip(ProtocolMessage::Rekey {
            nonce: vec![0xAB; 16],
            seq: 7,
        });
        assert_roundtrip(ProtocolMessage::Ack {
            acked_seq: 42,
            seq: 8,
        });
    }

    #[test]
    fn roundtrip_edge_values() {
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
        assert_roundtrip(ProtocolMessage::FileChunk {
            chunk: Vec::new(),
            seq: 1,
        });
        assert_roundtrip(ProtocolMessage::FileChunk {
            chunk: vec![0xCD; crate::FILE_CHUNK_SIZE],
            seq: 2,
        });
        assert_roundtrip(text_chunk(0, 1, "x".repeat(crate::TEXT_CHUNK_BYTES)));
        assert_roundtrip(file_meta("big.bin", crate::MAX_FILE_SIZE));
    }

    #[test]
    fn filemeta_sanitizes_path_traversal_on_decode() {
        match decode(&file_meta("../../etc/passwd", 10)).unwrap() {
            ProtocolMessage::FileMeta { filename, .. } => {
                assert!(!filename.contains(".."));
                assert!(!filename.contains('/'));
            }
            other => panic!("expected FileMeta, got {:?}", other),
        }
    }

    #[test]
    fn malformed_inputs_return_none() {
        for frame in [
            &[][..],
            &[2u8, 0, 0],                       // Text missing its payload
            &[200u8, 1, 2, 3],                  // unknown tag, no legacy prefix
            &[5u8, 0, 0],                       // FileEnd needs 8 bytes of seq
            &[13u8, 0, 0, 0, 0, 0, 0, 0, 0, 0], // Ack needs seq + acked_seq
        ] {
            assert!(
                ProtocolMessage::from_plain_bytes(frame).is_none(),
                "{frame:?}"
            );
        }
    }

    #[test]
    fn oversized_payloads_are_rejected() {
        assert!(decode(&file_meta("x", crate::MAX_FILE_SIZE + 1)).is_none());
        assert!(decode(&ProtocolMessage::FileChunk {
            chunk: vec![0u8; crate::FILE_CHUNK_SIZE + 1],
            seq: 1,
        })
        .is_none());
        // EphemeralKey over the 256-byte cap.
        assert!(decode(&ProtocolMessage::EphemeralKey {
            public_key: vec![0u8; 300],
        })
        .is_none());
    }

    /// `total_chunks` must be non-zero and capped (it sizes the reassembly buffer, so
    /// `u32::MAX` was a remote-OOM vector), and the index in range.
    #[test]
    fn textchunk_invariants_enforced_on_decode() {
        let x = || "x".to_string();
        assert!(decode(&text_chunk(0, 0, x())).is_none());
        assert!(decode(&text_chunk(3, 3, x())).is_none());
        assert!(decode(&text_chunk(0, crate::MAX_TEXT_CHUNKS + 1, x())).is_none());
        assert!(decode(&text_chunk(0, u32::MAX, x())).is_none());
        assert!(decode(&text_chunk(0, crate::MAX_TEXT_CHUNKS, x())).is_some());
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
