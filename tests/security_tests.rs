use encodeur_rsa_rust::core::ProtocolMessage;
use encodeur_rsa_rust::util::sanitize_filename;

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
fn test_protocol_message_input_limits() {
    // Ensure we don't crash on huge inputs
    let huge_data = vec![0u8; 10 * 1024 * 1024]; // 10MB
                                                 // ProtocolMessage::from_plain_bytes has a check for TEXT: len > 64KB

    let mut text_msg = b"TEXT:".to_vec();
    text_msg.extend_from_slice(&huge_data);

    let parsed = ProtocolMessage::from_plain_bytes(&text_msg);
    assert!(parsed.is_none(), "Huge text message should be rejected");
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
