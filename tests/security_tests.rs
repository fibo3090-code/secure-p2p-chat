use encodeur_rsa_rust::core::ProtocolMessage;
use encodeur_rsa_rust::util::sanitize_filename;
use encodeur_rsa_rust::{FILE_CHUNK_SIZE, MAX_FILE_SIZE, MAX_TEXT_MESSAGE_BYTES, TEXT_CHUNK_BYTES};
use tempfile::TempDir;

#[test]
fn test_filename_sanitization_security() {
    let inputs = vec![
        "../../etc/passwd",
        "C:\\Windows\\System32\\cmd.exe",
        "file|with|pipes",
        "file?with?questions",
        "file*with*stars",
    ];

    for input in inputs {
        let sanitized = sanitize_filename(input);
        assert!(!sanitized.contains("..")); // Sanitization replaces separators, ruining ".."
        assert!(!sanitized.contains("/"));
        assert!(!sanitized.contains("\\"));
        assert!(!sanitized.contains("|"));
        assert!(!sanitized.contains("?"));
        assert!(!sanitized.contains("*"));
    }
}

#[test]
fn test_sanitized_filename_stays_within_directory() {
    let temp_dir = TempDir::new().unwrap();
    let filename = sanitize_filename("../../etc/passwd");
    let target_path = temp_dir.path().join(filename);

    std::fs::write(&target_path, b"test").unwrap();
    let base = std::fs::canonicalize(temp_dir.path()).unwrap();
    let file = std::fs::canonicalize(&target_path).unwrap();

    assert!(file.starts_with(base));
}

#[test]
fn test_protocol_message_input_limits() {
    // Ensure we don't crash on huge inputs
    let huge_data = vec![0u8; 10 * 1024 * 1024]; // 10MB
                                                 // ProtocolMessage::from_plain_bytes caps legacy text payloads.

    let mut text_msg = b"TEXT:".to_vec();
    text_msg.extend_from_slice(&huge_data);

    let parsed = ProtocolMessage::from_plain_bytes(&text_msg);
    assert!(parsed.is_none(), "Huge text message should be rejected");
}

#[test]
fn test_binary_protocol_limits() {
    let mut oversized_text = Vec::new();
    oversized_text.push(2u8); // Text tag
    oversized_text.extend_from_slice(&0u64.to_be_bytes());
    oversized_text.extend_from_slice(&0u64.to_be_bytes());
    let text_len = ((MAX_TEXT_MESSAGE_BYTES + 1) as u32).to_be_bytes();
    oversized_text.extend_from_slice(&text_len);
    oversized_text.extend_from_slice(&vec![b'a'; MAX_TEXT_MESSAGE_BYTES + 1]);
    assert!(ProtocolMessage::from_plain_bytes(&oversized_text).is_none());

    let mut oversized_text_chunk = Vec::new();
    oversized_text_chunk.push(11u8); // TextChunk tag
    oversized_text_chunk.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    oversized_text_chunk.extend_from_slice(&1u64.to_be_bytes());
    oversized_text_chunk.extend_from_slice(&0u64.to_be_bytes());
    oversized_text_chunk.extend_from_slice(&0u32.to_be_bytes());
    oversized_text_chunk.extend_from_slice(&1u32.to_be_bytes());
    oversized_text_chunk.extend_from_slice(&((TEXT_CHUNK_BYTES + 1) as u32).to_be_bytes());
    oversized_text_chunk.extend_from_slice(&vec![b'a'; TEXT_CHUNK_BYTES + 1]);
    assert!(ProtocolMessage::from_plain_bytes(&oversized_text_chunk).is_none());

    let mut oversized_meta = Vec::new();
    oversized_meta.push(3u8); // FileMeta tag
    oversized_meta.extend_from_slice(&1u64.to_be_bytes());
    oversized_meta.extend_from_slice(&(MAX_FILE_SIZE + 1).to_be_bytes());
    oversized_meta.extend_from_slice(&(4u32).to_be_bytes());
    oversized_meta.extend_from_slice(b"test");
    assert!(ProtocolMessage::from_plain_bytes(&oversized_meta).is_none());

    let mut oversized_chunk = Vec::new();
    oversized_chunk.push(4u8); // FileChunk tag
    oversized_chunk.extend_from_slice(&1u64.to_be_bytes());
    oversized_chunk.extend_from_slice(&((FILE_CHUNK_SIZE + 1) as u32).to_be_bytes());
    oversized_chunk.extend_from_slice(&vec![0u8; FILE_CHUNK_SIZE + 1]);
    assert!(ProtocolMessage::from_plain_bytes(&oversized_chunk).is_none());
}

#[test]
fn test_legacy_ephemeral_key_rejects_oversized_payload() {
    // Regression: a malicious peer could send an EPHEMERAL_KEY: payload larger
    // than 32 bytes, causing the legacy parser to allocate the full body before
    // any length check. Ensure such payloads are now rejected without copying.
    let mut oversized = b"EPHEMERAL_KEY:".to_vec();
    oversized.extend_from_slice(&vec![0u8; 8 * 1024 * 1024]);
    assert!(
        ProtocolMessage::from_plain_bytes(&oversized).is_none(),
        "Oversized EPHEMERAL_KEY payload must be rejected"
    );

    let mut short = b"EPHEMERAL_KEY:".to_vec();
    short.extend_from_slice(&[0u8; 16]);
    assert!(
        ProtocolMessage::from_plain_bytes(&short).is_none(),
        "Undersized EPHEMERAL_KEY payload must be rejected"
    );

    let mut valid = b"EPHEMERAL_KEY:".to_vec();
    valid.extend_from_slice(&[0u8; 32]);
    assert!(
        matches!(
            ProtocolMessage::from_plain_bytes(&valid),
            Some(ProtocolMessage::EphemeralKey { .. })
        ),
        "Exactly 32-byte EPHEMERAL_KEY payload must parse"
    );
}

#[test]
fn test_file_meta_parsing_robustness() {
    // Malformed metadata (size is not a number)
    let bad_meta = b"FILE_META|0|filename|not_a_number";
    let parsed = ProtocolMessage::from_plain_bytes(bad_meta);
    assert!(parsed.is_none());

    // Malicious filename that should be sanitized
    let injection = b"FILE_META|0|../../evil|100";
    let parsed = ProtocolMessage::from_plain_bytes(injection);
    // It parses, but filename should be sanitized
    if let Some(ProtocolMessage::FileMeta { filename, .. }) = parsed {
        assert_ne!(filename, "../../evil");
        assert!(!filename.contains("/"));
    } else {
        panic!("Should parse but sanitize");
    }
}
