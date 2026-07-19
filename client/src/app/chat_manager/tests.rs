use super::*;
use base64::Engine;
use tempfile::tempdir;

#[test]
fn parse_invite_placeholder_is_ignored() {
    let mgr = ChatManager::default();

    let payload = serde_json::json!({
        "name": "Alice",
        "address": "YOUR_IP:PORT",
        "fingerprint": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "public_key": "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkq...\n-----END PUBLIC KEY-----",
    });

    let json = serde_json::to_string(&payload).unwrap();
    use base64::engine::general_purpose;
    let encoded = general_purpose::STANDARD.encode(json);
    let link = format!("chat-p2p://invite/{}", encoded);

    let contact = mgr.parse_invite_link(&link).expect("should parse invite");
    assert!(
        contact.address.is_none(),
        "placeholder address must be ignored"
    );
}

#[test]
fn host_prompts_for_an_unknown_incoming_fingerprint() {
    // Simulate an incoming connection: a fresh chat (no stored fingerprint)
    // mapped from the peer's chat id to the session id, plus a confirm channel.
    let mut mgr = ChatManager::new(Config::default());
    let session_id = Uuid::new_v4();
    let incoming = Uuid::new_v4();
    mgr.create_local_chat_for_test(incoming, "Peer".into());
    mgr.chat_id_mapping.insert(incoming, session_id);
    let (tx, mut rx) = mpsc::unbounded_channel();
    mgr.add_fingerprint_confirm_sender_for_test(session_id, tx);

    mgr.handle_tofu_verification(session_id, "UNKNOWN-FP", "Peer", "12-34-56");

    // The host must PROMPT for verification, not silently auto-trust.
    assert!(
        mgr.fingerprint_verification_request.is_some(),
        "host must prompt to verify an unknown incoming peer"
    );
    assert!(
        rx.try_recv().is_err(),
        "host must not auto-confirm an unknown incoming peer"
    );
}

#[test]
fn host_auto_accepts_a_returning_known_fingerprint() {
    // A prior chat already has this fingerprint verified (a returning peer).
    let mut mgr = ChatManager::new(Config::default());
    let prior = Uuid::new_v4();
    mgr.create_local_chat_for_test(prior, "Known".into());
    mgr.get_chat_mut(prior).unwrap().peer_fingerprint = Some("KNOWN-FP".into());

    let session_id = Uuid::new_v4();
    let incoming = Uuid::new_v4();
    mgr.create_local_chat_for_test(incoming, "Peer".into());
    mgr.chat_id_mapping.insert(incoming, session_id);
    let (tx, mut rx) = mpsc::unbounded_channel();
    mgr.add_fingerprint_confirm_sender_for_test(session_id, tx);

    mgr.handle_tofu_verification(session_id, "KNOWN-FP", "Peer", "12-34-56");

    // Returning peer: auto-confirmed without re-prompting.
    assert!(
        mgr.fingerprint_verification_request.is_none(),
        "a known fingerprint must not trigger another prompt"
    );
    assert_eq!(
        rx.try_recv().ok(),
        Some(true),
        "a known fingerprint must be auto-confirmed"
    );
}

#[test]
fn parse_invite_with_valid_address_keeps_it() {
    let mgr = ChatManager::default();

    let payload = serde_json::json!({
        "name": "Bob",
        "address": "127.0.0.1:54321",
        "fingerprint": "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210",
        "public_key": "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkq...\n-----END PUBLIC KEY-----",
    });

    let json = serde_json::to_string(&payload).unwrap();
    use base64::engine::general_purpose;
    let encoded = general_purpose::STANDARD.encode(json);
    let link = format!("chat-p2p://invite/{}", encoded);

    let contact = mgr.parse_invite_link(&link).expect("should parse invite");
    assert_eq!(contact.address, Some("127.0.0.1:54321".to_string()));
}

#[test]
fn parse_invite_invalid_address_no_port() {
    let mgr = ChatManager::default();

    let payload = serde_json::json!({
        "name": "Charlie",
        "address": "127.0.0.1",
        "fingerprint": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "public_key": "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkq...\n-----END PUBLIC KEY-----",
    });

    let json = serde_json::to_string(&payload).unwrap();
    use base64::engine::general_purpose;
    let encoded = general_purpose::STANDARD.encode(json);
    let link = format!("chat-p2p://invite/{}", encoded);

    let contact = mgr.parse_invite_link(&link).expect("should parse invite");
    assert!(
        contact.address.is_none(),
        "address without port should be None"
    );
}

#[test]
fn parse_invite_invalid_address_bad_port() {
    let mgr = ChatManager::default();

    let payload = serde_json::json!({
        "name": "Dana",
        "address": "127.0.0.1:notaport",
        "fingerprint": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "public_key": "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkq...\n-----END PUBLIC KEY-----",
    });

    let json = serde_json::to_string(&payload).unwrap();
    use base64::engine::general_purpose;
    let encoded = general_purpose::STANDARD.encode(json);
    let link = format!("chat-p2p://invite/{}", encoded);

    let contact = mgr.parse_invite_link(&link).expect("should parse invite");
    assert!(
        contact.address.is_none(),
        "address with non-numeric port should be None"
    );
}

#[test]
fn placeholder_detection_works() {
    let mut mgr = ChatManager::new(Config::default());
    let port = 5001u16;
    assert!(!mgr.chats.values().any(|c| c.is_host_placeholder));
    let chat = Chat {
        id: Uuid::new_v4(),
        title: format!("Host on :{}", port),
        kind: ChatKind::Dm,
        transport: Transport::Direct,
        peer_fingerprint: None,
        participants: Vec::new(),
        messages: Vec::new(),
        created_at: chrono::Utc::now(),
        peer_typing: false,
        typing_since: None,
        send_seq: 0,
        recv_seq: 0,
        is_host_placeholder: true,
    };
    let id = chat.id;
    mgr.chats.insert(id, chat);
    assert!(mgr.chats.values().any(|c| c.is_host_placeholder));
}

#[test]
fn test_tofu_logic() {
    let mut mgr = ChatManager::default();
    let chat_id = Uuid::new_v4();
    let peer_name = "peer".to_string();
    let fingerprint1 = "fingerprint1".to_string();
    let fingerprint2 = "fingerprint2".to_string();

    // 1. First Use: No fingerprint exists.
    let chat = Chat {
        id: chat_id,
        title: "Test Chat".to_string(),
        kind: ChatKind::Dm,
        transport: Transport::Direct,
        peer_fingerprint: None,
        participants: Vec::new(),
        messages: Vec::new(),
        created_at: chrono::Utc::now(),
        peer_typing: false,
        typing_since: None,
        send_seq: 0,
        recv_seq: 0,
        is_host_placeholder: false,
    };
    // Add a dummy confirmation sender for the test
    let (tx, mut rx) = mpsc::unbounded_channel();
    mgr.fingerprint_confirm_senders.insert(chat_id, tx);
    mgr.chats.insert(chat_id, chat);
    // 1. First Use: No fingerprint exists -> UI prompt expected (no auto-confirm)
    let event1 = SessionEvent::ShowFingerprintVerification {
        fingerprint: fingerprint1.clone(),
        peer_name: peer_name.clone(),
        sas: String::new(),
        chat_id,
    };
    mgr.handle_session_event(chat_id, event1);

    // Assert: No auto-storage, request pending, and no confirmation sent automatically.
    assert_eq!(mgr.chats.get(&chat_id).unwrap().peer_fingerprint, None);
    assert!(mgr.fingerprint_verification_request.is_some());
    assert!(rx.try_recv().is_err());

    // Simulate user accepting the fingerprint via UI
    mgr.confirm_fingerprint(chat_id, true)
        .expect("confirm should succeed");
    // Now the session should receive confirmation
    assert_eq!(rx.try_recv(), Ok(true));
    // And the fingerprint should now be stored
    assert_eq!(
        mgr.chats.get(&chat_id).unwrap().peer_fingerprint,
        Some(fingerprint1.clone())
    );

    // 2. Second Use: Matching fingerprint -> auto-confirm
    let event2 = SessionEvent::ShowFingerprintVerification {
        fingerprint: fingerprint1.clone(),
        peer_name: peer_name.clone(),
        sas: String::new(),
        chat_id,
    };
    mgr.handle_session_event(chat_id, event2);

    // Assert: No UI request, and connection is confirmed automatically.
    assert!(mgr.fingerprint_verification_request.is_none());
    assert_eq!(rx.try_recv(), Ok(true));

    // 3. Third Use: Mismatched fingerprint -> UI prompt, no auto-confirm
    let event3 = SessionEvent::ShowFingerprintVerification {
        fingerprint: fingerprint2.clone(),
        peer_name: peer_name.clone(),
        sas: String::new(),
        chat_id,
    };
    mgr.handle_session_event(chat_id, event3);

    // Assert: A UI request IS made, and no confirmation is sent automatically.
    assert!(mgr.fingerprint_verification_request.is_some());
    let fp = mgr
        .fingerprint_verification_request
        .clone()
        .unwrap()
        .fingerprint;
    assert_eq!(fp, fingerprint2);
    assert!(rx.try_recv().is_err());
}

/// PHASE 0 REGRESSION TEST: auto_trust_on_first_use must default to false.
/// If this test fails, the TOFU auto-trust MEDIUM-priority issue is present.
/// auto_trust_on_first_use=true silently accepts first-contact MITM without verification prompt.
#[test]
fn test_regression_auto_trust_default_off() {
    let config = Config::default();
    assert!(
        !config.auto_trust_on_first_use,
        "auto_trust_on_first_use MUST default to false for security"
    );

    // When auto_trust_on_first_use=false (the default), first fingerprint should require user verification.
    let mut mgr = ChatManager::new(config);
    let chat_id = Uuid::new_v4();

    let chat = Chat {
        id: chat_id,
        title: "Test Chat".to_string(),
        kind: ChatKind::Dm,
        transport: Transport::Direct,
        peer_fingerprint: None,
        participants: Vec::new(),
        messages: Vec::new(),
        created_at: chrono::Utc::now(),
        peer_typing: false,
        typing_since: None,
        send_seq: 0,
        recv_seq: 0,
        is_host_placeholder: false,
    };

    let (tx, _rx) = mpsc::unbounded_channel();
    mgr.fingerprint_confirm_senders.insert(chat_id, tx);
    mgr.chats.insert(chat_id, chat);

    // When a new fingerprint is received (first use), it should prompt the user
    let event = SessionEvent::ShowFingerprintVerification {
        fingerprint: "first_fingerprint".to_string(),
        peer_name: "Alice".to_string(),
        sas: String::new(),
        chat_id,
    };

    mgr.handle_session_event(chat_id, event);

    // With auto_trust=false, fingerprint should NOT be auto-stored
    assert_eq!(mgr.chats.get(&chat_id).unwrap().peer_fingerprint, None);
    // And a verification request should be pending
    assert!(
        mgr.fingerprint_verification_request.is_some(),
        "Should show fingerprint verification dialog when auto_trust_on_first_use=false"
    );
}

/// PHASE 0 REGRESSION TEST: mDNS discovery should be disabled by default.
/// If this test fails, the mDNS metadata exposure LOW-priority issue is present.
/// Enabling mDNS broadcasts fingerprint + hostname on the local network.
#[test]
fn test_regression_mdns_default_off() {
    let config = Config::default();
    assert!(
        !config.enable_mdns,
        "enable_mdns MUST default to false for privacy (LAN fingerprint disclosure risk)"
    );
}

// ============================================================================
// Signed Invite Link (v2) Parsing Tests
// ============================================================================

#[test]
fn parse_v2_signed_invite_link_valid() {
    use crate::identity::Identity;

    let mgr = ChatManager::default();
    let identity = Identity::new_with_plaintext("Test Signer".to_string()).unwrap();

    // Generate a v2 signed invite
    let link = identity
        .generate_signed_invite_link(Some("127.0.0.1:9001".to_string()))
        .unwrap();

    // Parse it
    let contact = mgr
        .parse_invite_link(&link)
        .expect("should parse v2 signed invite");

    // Verify contact fields
    assert_eq!(contact.name, "Test Signer");
    assert_eq!(contact.address, Some("127.0.0.1:9001".to_string()));
    assert_eq!(contact.fingerprint, Some(identity.fingerprint.clone()));
    assert_eq!(contact.public_key, Some(identity.public_key_pem.clone()));
    assert_eq!(contact.trust_state, TrustState::Unverified);
    // Single-address invite: no extra candidate list.
    assert!(contact.addresses.is_empty());
    assert_eq!(
        contact.candidate_addresses(),
        vec!["127.0.0.1:9001".to_string()]
    );
}

#[test]
fn parse_multi_address_invite_populates_ordered_candidates() {
    use crate::identity::Identity;

    let mgr = ChatManager::default();
    let identity = Identity::new_with_plaintext("Multi Homed".to_string()).unwrap();

    // External-first ordering, with a duplicate and an unparsable entry that
    // must be sanitized out without breaking signature verification.
    let link = identity
        .generate_signed_invite_link_with_addresses(
            vec![
                "203.0.113.7:12345".to_string(),
                "not an address".to_string(),
                "192.168.1.20:12345".to_string(),
                "203.0.113.7:12345".to_string(), // duplicate of the first
            ],
            None,
            None,
        )
        .unwrap();

    let contact = mgr
        .parse_invite_link(&link)
        .expect("multi-address invite must verify and parse");

    assert_eq!(contact.name, "Multi Homed");
    // Primary address is the first valid candidate…
    assert_eq!(contact.address, Some("203.0.113.7:12345".to_string()));
    // …and the ordered, deduplicated, sanitized list is preserved.
    assert_eq!(
        contact.addresses,
        vec![
            "203.0.113.7:12345".to_string(),
            "192.168.1.20:12345".to_string()
        ]
    );
    assert_eq!(contact.candidate_addresses(), contact.addresses);
}

#[test]
fn contact_candidate_addresses_falls_back_to_legacy_single() {
    // Contacts imported before multi-address invites (or from old history
    // files) have only `address`; candidate_addresses() must still yield it.
    let contact = Contact {
        id: uuid::Uuid::new_v4(),
        name: "Legacy".to_string(),
        address: Some("10.0.0.9:6000".to_string()),
        addresses: Vec::new(),
        relay_server: None,
        relay_token: None,
        fingerprint: None,
        public_key: None,
        created_at: chrono::Utc::now(),
        trust_state: TrustState::Unverified,
        notes: String::new(),
        tags: Vec::new(),
        last_seen: None,
    };
    assert_eq!(
        contact.candidate_addresses(),
        vec!["10.0.0.9:6000".to_string()]
    );

    // And an old-format JSON blob (no `addresses` key) still deserializes.
    let old_json = serde_json::json!({
        "id": uuid::Uuid::new_v4(),
        "name": "Old JSON",
        "address": "10.0.0.9:6000",
        "fingerprint": null,
        "public_key": null,
        "created_at": chrono::Utc::now(),
        "last_seen": null,
    });
    let loaded: Contact = serde_json::from_value(old_json).unwrap();
    assert!(loaded.addresses.is_empty());
    assert_eq!(
        loaded.candidate_addresses(),
        vec!["10.0.0.9:6000".to_string()]
    );
}

#[test]
fn parse_v2_signed_invite_rejects_tampered_signature() {
    use crate::identity::Identity;
    use base64::Engine;

    let mgr = ChatManager::default();
    let identity = Identity::new_with_plaintext("Tamper Test".to_string()).unwrap();

    // Generate a v2 signed invite
    let link = identity.generate_signed_invite_link(None).unwrap();
    let encoded = link.strip_prefix("chat-p2p://invite/v2/").unwrap();

    // Decode, tamper with signature, re-encode
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .unwrap();
    let json_str = String::from_utf8(decoded).unwrap();

    #[derive(serde::Deserialize, serde::Serialize)]
    struct SignedInvite {
        payload: serde_json::Value,
        signature: Vec<u8>,
    }

    let mut invite: SignedInvite = serde_json::from_str(&json_str).unwrap();
    // Flip a bit in the signature to tamper with it
    if !invite.signature.is_empty() {
        invite.signature[0] ^= 0xFF;
    }

    let tampered_json = serde_json::to_string(&invite).unwrap();
    let tampered_encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&tampered_json);
    let tampered_link = format!("chat-p2p://invite/v2/{}", tampered_encoded);

    // Parsing should fail due to signature verification
    assert!(
        mgr.parse_invite_link(&tampered_link).is_err(),
        "should reject tampered signature"
    );
}

/// Re-sign a fresh invite with a rewritten timestamp, keeping the signature
/// valid, so parse-time expiry is exercised independently of signature checks.
#[cfg(test)]
fn resign_invite_with_timestamp(identity: &crate::identity::Identity, timestamp: u64) -> String {
    use base64::Engine;

    let link = identity.generate_signed_invite_link(None).unwrap();
    let encoded = link.strip_prefix("chat-p2p://invite/v2/").unwrap();
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .unwrap();

    // Mirror the parser's payload struct so the signature covers bytes with
    // the same field order it will re-serialize for verification.
    #[derive(serde::Serialize, serde::Deserialize)]
    struct Payload {
        version: u32,
        timestamp: u64,
        nonce: String,
        name: String,
        address: Option<String>,
        relay_server: Option<String>,
        relay_token: Option<String>,
        fingerprint: String,
        public_key: String,
    }
    #[derive(serde::Serialize, serde::Deserialize)]
    struct SignedInvite {
        payload: Payload,
        signature: Vec<u8>,
    }

    let mut invite: SignedInvite = serde_json::from_slice(&decoded).unwrap();
    invite.payload.timestamp = timestamp;

    let payload_json = serde_json::to_string(&invite.payload).unwrap();
    invite.signature = crate::core::crypto::rsa_sign_pss(
        &identity.private_key().unwrap(),
        payload_json.as_bytes(),
    )
    .unwrap();

    let re_encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_string(&invite).unwrap());
    format!("chat-p2p://invite/v2/{}", re_encoded)
}

#[test]
fn parse_signed_invite_rejects_expired_timestamp() {
    use crate::identity::Identity;

    let mgr = ChatManager::default();
    let identity = Identity::new_with_plaintext("Expiry Test".to_string()).unwrap();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let expired = now - crate::INVITE_MAX_AGE_SECS - 60;
    let link = resign_invite_with_timestamp(&identity, expired);

    let err = mgr
        .parse_invite_link(&link)
        .expect_err("expired invite must be rejected");
    assert!(
        err.to_string().contains("expired"),
        "error should mention expiry, got: {err}"
    );
}

#[test]
fn parse_signed_invite_rejects_future_timestamp() {
    use crate::identity::Identity;

    let mgr = ChatManager::default();
    let identity = Identity::new_with_plaintext("Skew Test".to_string()).unwrap();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let future = now + crate::INVITE_TIMESTAMP_SKEW_SECS + 60;
    let link = resign_invite_with_timestamp(&identity, future);

    let err = mgr
        .parse_invite_link(&link)
        .expect_err("future-dated invite must be rejected");
    assert!(
        err.to_string().contains("future"),
        "error should mention future timestamp, got: {err}"
    );
}

#[test]
fn parse_signed_invite_accepts_recent_timestamp_within_window() {
    use crate::identity::Identity;

    let mgr = ChatManager::default();
    let identity = Identity::new_with_plaintext("Fresh Test".to_string()).unwrap();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    // A week old: well inside the 30-day window.
    let link = resign_invite_with_timestamp(&identity, now - 7 * 86_400);

    mgr.parse_invite_link(&link)
        .expect("invite inside the expiry window must still parse");
}

#[test]
fn parse_v1_invite_link_still_works_with_warning() {
    let mgr = ChatManager::default();

    let payload = serde_json::json!({
        "name": "Legacy User",
        "address": "192.168.1.50:8001",
        "fingerprint": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        "public_key": "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkq...\n-----END PUBLIC KEY-----",
    });

    let json = serde_json::to_string(&payload).unwrap();
    let encoded = base64::engine::general_purpose::STANDARD.encode(json);
    let link = format!("chat-p2p://invite/{}", encoded);

    // Should still parse v1 unsigned invites (backward compatibility)
    let contact = mgr
        .parse_invite_link(&link)
        .expect("should parse v1 invite");
    assert_eq!(contact.name, "Legacy User");
    assert_eq!(contact.address, Some("192.168.1.50:8001".to_string()));
    assert_eq!(
        contact.fingerprint,
        Some("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_string())
    );
}

#[test]
fn parse_v2_signed_invite_without_address() {
    use crate::identity::Identity;

    let mgr = ChatManager::default();
    let identity = Identity::new_with_plaintext("No Address User".to_string()).unwrap();

    // Generate a v2 signed invite without address
    let link = identity.generate_signed_invite_link(None).unwrap();

    // Parse it
    let contact = mgr
        .parse_invite_link(&link)
        .expect("should parse v2 signed invite without address");

    assert_eq!(contact.name, "No Address User");
    assert_eq!(contact.address, None);
    assert_eq!(contact.fingerprint, Some(identity.fingerprint.clone()));
}

#[test]
fn parse_v2_signed_invite_preserves_identity_fields() {
    use crate::identity::Identity;

    let mgr = ChatManager::default();
    let identity = Identity::new_with_plaintext("Complete Info User".to_string()).unwrap();

    // Generate with full info
    let link = identity
        .generate_signed_invite_link(Some("10.20.30.40:6500".to_string()))
        .unwrap();

    let contact = mgr
        .parse_invite_link(&link)
        .expect("should parse v2 signed invite");

    // All fields should match the identity
    assert_eq!(contact.name, "Complete Info User");
    assert_eq!(contact.address, Some("10.20.30.40:6500".to_string()));
    assert_eq!(contact.fingerprint, Some(identity.fingerprint.clone()));
    assert_eq!(contact.public_key, Some(identity.public_key_pem.clone()));
}

#[test]
fn v2_invite_signature_verification_prevents_fingerprint_swap() {
    use crate::identity::Identity;
    use base64::Engine;

    let mgr = ChatManager::default();
    let identity1 = Identity::new_with_plaintext("User One".to_string()).unwrap();
    let identity2 = Identity::new_with_plaintext("User Two".to_string()).unwrap();

    // Generate invite from identity1
    let link = identity1.generate_signed_invite_link(None).unwrap();
    let encoded = link.strip_prefix("chat-p2p://invite/v2/").unwrap();

    // Decode and try to swap the fingerprint with identity2's
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .unwrap();
    let json_str = String::from_utf8(decoded).unwrap();

    #[derive(serde::Deserialize, serde::Serialize)]
    struct SignedInvite {
        payload: serde_json::Value,
        signature: Vec<u8>,
    }

    let mut invite: SignedInvite = serde_json::from_str(&json_str).unwrap();

    // Swap fingerprint (attack attempt)
    invite.payload["fingerprint"] = serde_json::Value::String(identity2.fingerprint.clone());

    let tampered_json = serde_json::to_string(&invite).unwrap();
    let tampered_encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&tampered_json);
    let tampered_link = format!("chat-p2p://invite/v2/{}", tampered_encoded);

    // Should reject because signature won't verify with modified payload
    assert!(
        mgr.parse_invite_link(&tampered_link).is_err(),
        "should reject invite with swapped fingerprint"
    );
}

#[tokio::test]
async fn send_file_uses_monotonic_chat_sequence_space() {
    let mut mgr = ChatManager::new(Config::default());
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "File Seq Test".to_string());

    let (from_app_tx, mut from_app_rx) = mpsc::unbounded_channel();
    mgr.sessions.insert(chat_id, SessionHandle { from_app_tx });

    // Start from a non-zero value to ensure file transfer continues existing sequence space.
    mgr.chats.get_mut(&chat_id).unwrap().send_seq = 5;

    let temp_file = tempfile::NamedTempFile::new().unwrap();
    let content = vec![b'x'; crate::FILE_CHUNK_SIZE * 2 + 13];
    std::fs::write(temp_file.path(), content).unwrap();

    mgr.send_file(chat_id, temp_file.path().to_path_buf())
        .await
        .expect("send_file should succeed");

    let mut seqs = Vec::new();
    let mut saw_meta = false;
    let mut saw_end = false;
    let mut chunk_count = 0usize;
    while let Ok(msg) = from_app_rx.try_recv() {
        match msg {
            ProtocolMessage::FileMeta { seq, .. } => {
                saw_meta = true;
                seqs.push(seq);
            }
            ProtocolMessage::FileChunk { seq, .. } => {
                chunk_count += 1;
                seqs.push(seq);
            }
            ProtocolMessage::FileEnd { seq } => {
                saw_end = true;
                seqs.push(seq);
            }
            _ => {}
        }
    }

    assert!(saw_meta, "FileMeta should be emitted");
    assert!(saw_end, "FileEnd should be emitted");
    assert!(chunk_count >= 2, "Test file should produce multiple chunks");
    assert_eq!(
        seqs.first().copied(),
        Some(6),
        "Sequence should continue from chat.send_seq"
    );
    assert!(
        seqs.windows(2).all(|w| w[1] == w[0] + 1),
        "File transfer messages must use strictly increasing sequence numbers"
    );
}

/// Queueing frames on the session is not delivery: the "File sent" toast
/// must wait for the session's wire-level confirmation event.
#[tokio::test]
async fn file_sent_toast_waits_for_wire_confirmation() {
    let mut mgr = ChatManager::new(Config::default());
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Honest Send".to_string());

    let (from_app_tx, _from_app_rx) = mpsc::unbounded_channel();
    mgr.sessions.insert(chat_id, SessionHandle { from_app_tx });

    let temp_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(temp_file.path(), b"payload").unwrap();
    mgr.send_file(chat_id, temp_file.path().to_path_buf())
        .await
        .expect("send_file should succeed");

    assert!(
        !mgr.toasts.iter().any(|t| t.message.contains("File sent")),
        "success toast must not fire before the wire confirmation"
    );

    mgr.handle_session_event(chat_id, SessionEvent::FileSendComplete { seq: 42 });
    assert!(
        mgr.toasts.iter().any(|t| t.message.contains("File sent")),
        "success toast must fire once the final frame is on the wire"
    );
}

/// If the session dies before the final frame is written, the user must be
/// told the file may not have arrived — not shown a success message.
#[tokio::test]
async fn disconnect_fails_pending_file_sends_honestly() {
    let mut mgr = ChatManager::new(Config::default());
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Interrupted Send".to_string());

    let (from_app_tx, _from_app_rx) = mpsc::unbounded_channel();
    mgr.sessions.insert(chat_id, SessionHandle { from_app_tx });

    let temp_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(temp_file.path(), b"payload").unwrap();
    mgr.send_file(chat_id, temp_file.path().to_path_buf())
        .await
        .expect("send_file should succeed");

    mgr.handle_session_event(chat_id, SessionEvent::Disconnected);

    assert!(
        !mgr.toasts.iter().any(|t| t.message.contains("File sent")),
        "no success toast after a disconnect"
    );
    assert!(
        mgr.toasts
            .iter()
            .any(|t| t.message.contains("may not have been delivered")),
        "the user must see an honest delivery warning"
    );
    assert!(
        !mgr.pending_file_sends.contains_key(&chat_id),
        "pending sends must be cleared on disconnect"
    );
}

#[test]
fn large_incoming_message_reassembles_into_one_chat_message() {
    let mut mgr = ChatManager::new(Config::default());
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Chunked Incoming".to_string());

    let text = "hello large world ".repeat(8_000);
    let timestamp = 123_456_789u64;
    let mut seq = 0u64;
    let messages = ChatManager::build_text_protocol_messages(&mut seq, &text, timestamp)
        .expect("large text should chunk successfully");
    assert!(messages.len() > 1, "test message should be chunked");

    for msg in messages {
        mgr.handle_session_event(chat_id, SessionEvent::MessageReceived(msg));
    }

    let chat = mgr.chats.get(&chat_id).unwrap();
    assert_eq!(chat.messages.len(), 1);
    match &chat.messages[0].content {
        MessageContent::Text { text: reassembled } => assert_eq!(reassembled, &text),
        other => panic!("expected text message, got {:?}", other),
    }
}

#[test]
fn stale_incoming_large_message_is_discarded() {
    let mut mgr = ChatManager::new(Config::default());
    let chat_id = Uuid::new_v4();
    let message_id = Uuid::new_v4();

    mgr.incoming_text_messages.insert(
        (chat_id, message_id),
        IncomingTextMessage {
            timestamp_millis: 1,
            parts: vec![Some("partial".to_string()), None],
            updated_at: std::time::Instant::now() - Duration::from_secs(121),
        },
    );

    mgr.cleanup_stale_incoming_text_messages();

    assert!(!mgr
        .incoming_text_messages
        .contains_key(&(chat_id, message_id)));
    assert!(
        mgr.toasts
            .iter()
            .any(|toast| toast.message.contains("large incoming message")),
        "cleanup should surface a warning toast"
    );
}

#[test]
fn mapped_session_ping_updates_actual_chat() {
    let mut mgr = ChatManager::new(Config::default());
    let session_chat_id = Uuid::new_v4();
    let actual_chat_id = Uuid::new_v4();

    mgr.chat_id_mapping.insert(actual_chat_id, session_chat_id);
    mgr.create_local_chat_for_test(actual_chat_id, "Mapped Chat".to_string());
    mgr.chats.get_mut(&actual_chat_id).unwrap().recv_seq = 0;

    mgr.handle_session_event(
        session_chat_id,
        SessionEvent::MessageReceived(ProtocolMessage::Ping { seq: 1 }),
    );

    assert_eq!(
        mgr.chats.get(&actual_chat_id).unwrap().recv_seq,
        1,
        "Ping sequence must be applied to mapped actual chat"
    );
}

#[test]
fn sequential_incoming_files_do_not_reuse_completed_transfer_state() {
    let temp_dir = tempdir().unwrap();
    let download_dir = temp_dir.path().join("downloads");
    let temp_download_dir = temp_dir.path().join("temp");
    let config = Config {
        download_dir: download_dir.clone(),
        temp_dir: temp_download_dir,
        ..Config::default()
    };

    let mut mgr = ChatManager::new(config);
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Sequential Files".to_string());

    let first_payload = b"first file payload";
    let second_payload = b"second payload";

    mgr.handle_session_event(
        chat_id,
        SessionEvent::MessageReceived(ProtocolMessage::FileMeta {
            filename: "first.txt".to_string(),
            size: first_payload.len() as u64,
            seq: 1,
        }),
    );
    mgr.handle_session_event(
        chat_id,
        SessionEvent::MessageReceived(ProtocolMessage::FileChunk {
            chunk: first_payload.to_vec(),
            seq: 2,
        }),
    );
    mgr.handle_session_event(
        chat_id,
        SessionEvent::MessageReceived(ProtocolMessage::FileEnd { seq: 3 }),
    );

    mgr.handle_session_event(
        chat_id,
        SessionEvent::MessageReceived(ProtocolMessage::FileMeta {
            filename: "second.txt".to_string(),
            size: second_payload.len() as u64,
            seq: 4,
        }),
    );
    mgr.handle_session_event(
        chat_id,
        SessionEvent::MessageReceived(ProtocolMessage::FileChunk {
            chunk: second_payload.to_vec(),
            seq: 5,
        }),
    );
    mgr.handle_session_event(
        chat_id,
        SessionEvent::MessageReceived(ProtocolMessage::FileEnd { seq: 6 }),
    );

    let chat = mgr.chats.get(&chat_id).unwrap();
    let file_messages: Vec<_> = chat
        .messages
        .iter()
        .filter_map(|message| match &message.content {
            MessageContent::File {
                path: Some(path), ..
            } => Some(path.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(file_messages.len(), 2, "both files should be recorded");
    assert!(
        mgr.active_transfers.is_empty(),
        "completed transfers should not remain active"
    );
    assert!(
        mgr.incoming_files.is_empty(),
        "no incoming file handles should remain after completion"
    );
    assert_eq!(
        std::fs::read(&file_messages[0]).unwrap(),
        first_payload,
        "first file should keep its payload"
    );
    assert_eq!(
        std::fs::read(&file_messages[1]).unwrap(),
        second_payload,
        "second file should keep its payload"
    );
}

#[test]
fn parse_v2_signed_invite_normalizes_ipv6_address() {
    use crate::identity::Identity;

    let mgr = ChatManager::default();
    let identity = Identity::new_with_plaintext("IPv6 User".to_string()).unwrap();
    let link = identity
        .generate_signed_invite_link(Some("[2001:db8::1]:12345".to_string()))
        .unwrap();

    let contact = mgr.parse_invite_link(&link).expect("should parse invite");
    assert_eq!(contact.address.as_deref(), Some("[2001:db8::1]:12345"));
}

#[test]
fn parse_v2_signed_invite_drops_unbracketed_ipv6_with_port() {
    use crate::identity::Identity;

    let mgr = ChatManager::default();
    let identity = Identity::new_with_plaintext("Broken IPv6".to_string()).unwrap();
    let link = identity
        .generate_signed_invite_link(Some("2001:db8::1:12345".to_string()))
        .unwrap();

    let contact = mgr
        .parse_invite_link(&link)
        .expect("invite itself should remain valid");
    assert_eq!(
        contact.address, None,
        "invalid address payloads should be dropped instead of normalized"
    );
}

#[test]
fn parse_v3_signed_invite_keeps_relay_route() {
    use crate::identity::Identity;

    let mgr = ChatManager::default();
    let identity = Identity::new_with_plaintext("Relay Invite User".to_string()).unwrap();
    let relay_token = "0123456789abcdef0123456789abcdef".to_string();
    let link = identity
        .generate_signed_invite_link_with_route(
            None,
            Some("relay.example.com:23456".to_string()),
            Some(relay_token.clone()),
        )
        .unwrap();

    let contact = mgr
        .parse_invite_link(&link)
        .expect("should parse relay invite");
    assert_eq!(
        contact.relay_server.as_deref(),
        Some("relay.example.com:23456")
    );
    assert_eq!(contact.relay_token.as_deref(), Some(relay_token.as_str()));
    assert_eq!(contact.address, None);
}

#[test]
fn delete_all_data_removes_files_and_clears_state() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().join("data");
    let history_path = data_dir.join("history.json.enc");
    let identity_path = data_dir.join("identity.json");
    let crash_log_path = data_dir
        .join("diagnostics")
        .join("crashes")
        .join("panic.log");

    std::fs::create_dir_all(crash_log_path.parent().unwrap()).unwrap();

    std::fs::write(&history_path, b"encrypted-history").unwrap();
    std::fs::write(&identity_path, b"encrypted-identity").unwrap();
    std::fs::write(&crash_log_path, b"crash").unwrap();

    let mut mgr = ChatManager::new(Config::default());
    let chat_id = Uuid::new_v4();
    let contact_id = mgr.add_contact(
        "Contact".to_string(),
        Some("127.0.0.1:12345".to_string()),
        None,
        None,
    );
    mgr.create_local_chat_for_test(chat_id, "Chat".to_string());
    mgr.contact_to_chat.insert(contact_id, chat_id);
    mgr.fingerprint_verification_request = Some(PendingFingerprint {
        fingerprint: "fingerprint".to_string(),
        peer_name: "peer".to_string(),
        sas: String::new(),
        session_id: chat_id,
    });

    mgr.delete_all_data(&data_dir, &history_path, &identity_path)
        .unwrap();

    assert!(!data_dir.exists(), "app data directory should be deleted");
    assert!(!history_path.exists(), "history file should be deleted");
    assert!(!identity_path.exists(), "identity file should be deleted");
    assert!(!crash_log_path.exists(), "diagnostics should be deleted");
    assert!(mgr.chats.is_empty());
    assert!(mgr.contacts.is_empty());
    assert!(mgr.contact_to_chat.is_empty());
    assert!(mgr.fingerprint_verification_request.is_none());
}

#[test]
fn register_incoming_text_chunk_rejects_oversized_total_chunks() {
    // Defense in depth beyond the wire decoder: a bogus total_chunks must not
    // drive a giant reassembly-buffer allocation.
    let mut mgr = ChatManager::default();
    let chat_id = Uuid::new_v4();
    let message_id = Uuid::new_v4();

    let err = mgr
        .register_incoming_text_chunk(chat_id, message_id, 0, u32::MAX, "x".to_string(), 0)
        .expect_err("oversized total_chunks must be rejected");
    assert!(err.to_string().contains("Invalid chunk count"));
    // No buffer should have been created.
    assert!(mgr.incoming_text_messages.is_empty());
}

#[test]
fn register_incoming_text_chunk_caps_concurrent_partial_messages() {
    let mut mgr = ChatManager::default();
    let chat_id = Uuid::new_v4();

    // Open the maximum number of distinct partial messages (each 2 chunks, so
    // none completes and they all stay in flight).
    for _ in 0..crate::MAX_CONCURRENT_PARTIAL_TEXT_PER_CHAT {
        let message_id = Uuid::new_v4();
        mgr.register_incoming_text_chunk(chat_id, message_id, 0, 2, "a".to_string(), 0)
            .expect("within the concurrent cap");
    }

    // One more distinct message must be rejected.
    let err = mgr
        .register_incoming_text_chunk(chat_id, Uuid::new_v4(), 0, 2, "a".to_string(), 0)
        .expect_err("beyond the concurrent cap must be rejected");
    assert!(err.to_string().contains("Too many concurrent"));

    // But a further chunk for an already-tracked message is still accepted.
    let existing = *mgr
        .incoming_text_messages
        .keys()
        .find(|(c, _)| *c == chat_id)
        .expect("a partial message exists");
    mgr.register_incoming_text_chunk(existing.0, existing.1, 1, 2, "b".to_string(), 0)
        .expect("completing an existing message stays allowed");
}
