use super::*;
use base64::Engine;
use tempfile::tempdir;

/// A stand-in public key for the v1 (unsigned) invite tests. Its exact contents
/// do not matter — what matters is that the fingerprint in the link is derived
/// from *this* key, because `parse_invite_link` now rejects an invite whose
/// fingerprint does not belong to the key it carries.
const TEST_PUBKEY_PEM: &str = "-----BEGIN PUBLIC KEY-----
MIIBIjANBgkq...
-----END PUBLIC KEY-----";

/// A minimal contact, as an invite import would produce one.
fn sample_contact_for_test(name: &str) -> Contact {
    Contact {
        id: Uuid::new_v4(),
        name: name.to_string(),
        address: None,
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
    }
}

/// Build a legacy v1 invite link whose fingerprint matches its public key.
fn v1_invite_link(name: &str, address: &str) -> String {
    let payload = serde_json::json!({
        "name": name,
        "address": address,
        "fingerprint": crate::core::crypto::fingerprint_pubkey(TEST_PUBKEY_PEM.as_bytes()),
        "public_key": TEST_PUBKEY_PEM,
    });
    let json = serde_json::to_string(&payload).unwrap();
    let encoded = base64::engine::general_purpose::STANDARD.encode(json);
    format!("chat-p2p://invite/{}", encoded)
}

#[test]
fn parse_invite_placeholder_is_ignored() {
    let mgr = ChatManager::default();

    let link = v1_invite_link("Peer", "YOUR_IP:PORT");

    let contact = mgr.parse_invite_link(&link).expect("should parse invite");
    assert!(
        contact.address.is_none(),
        "placeholder address must be ignored"
    );
}

// ── Desktop notifications: the focus gate ───────────────────────────────────

/// The setting promises "notify when a message arrives in the background". An
/// OS popup for the conversation on screen is the fastest way to make a user
/// switch notifications off for good, so the gate has to hold.
#[test]
fn notifications_are_suppressed_only_for_the_visible_conversation() {
    let mut mgr = ChatManager::new(Config::default());
    let open = Uuid::new_v4();
    let other = Uuid::new_v4();
    mgr.create_local_chat_for_test(open, "Open".into());
    mgr.create_local_chat_for_test(other, "Other".into());
    assert!(
        mgr.config.enable_notifications,
        "test assumes notifications default to on"
    );

    // Window focused, `open` on screen: only that conversation is silent.
    mgr.set_ui_presence(true, Some(open));
    assert!(!mgr.should_notify_for(open));
    assert!(mgr.should_notify_for(other));

    // Window in the background: everything notifies, including the conversation
    // that is still nominally "open".
    mgr.set_ui_presence(false, Some(open));
    assert!(mgr.should_notify_for(open));
    assert!(mgr.should_notify_for(other));

    // The user's setting still wins over any presence state.
    mgr.config.enable_notifications = false;
    mgr.set_ui_presence(false, None);
    assert!(!mgr.should_notify_for(open));
}

/// A host stores an incoming peer's messages under the *client's* chat id while
/// the session has its own id. Presence is reported with the displayed id, so
/// the gate has to resolve the session id back to it — otherwise every message
/// on an incoming connection notifies even while you are reading it.
#[test]
fn notification_gate_resolves_incoming_session_ids() {
    let mut mgr = ChatManager::new(Config::default());
    let session_id = Uuid::new_v4();
    let incoming = Uuid::new_v4();
    mgr.create_local_chat_for_test(incoming, "Peer".into());
    mgr.chat_id_mapping.insert(incoming, session_id);

    mgr.set_ui_presence(true, Some(incoming));
    assert!(
        !mgr.should_notify_for(session_id),
        "a message on the session backing the open chat must stay silent"
    );
}

/// A front-end that never reports presence must keep notifying — silently
/// disabling notifications for it would be the worse failure.
#[test]
fn unreported_presence_defaults_to_notifying() {
    let mut mgr = ChatManager::new(Config::default());
    let chat = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat, "Peer".into());
    assert!(mgr.should_notify_for(chat));
}

// ── Read marks ──────────────────────────────────────────────────────────────

#[test]
fn marking_read_clears_unread_and_resolves_session_ids() {
    let mut mgr = ChatManager::new(Config::default());
    let session_id = Uuid::new_v4();
    let incoming = Uuid::new_v4();
    mgr.create_local_chat_for_test(incoming, "Peer".into());
    mgr.chat_id_mapping.insert(incoming, session_id);
    if let Some(chat) = mgr.get_chat_mut(incoming) {
        for _ in 0..4 {
            chat.messages.push(Message {
                id: Uuid::new_v4(),
                from_me: false,
                content: MessageContent::Text { text: "hi".into() },
                timestamp: chrono::Utc::now(),
                delivered: false,
            });
        }
    }
    assert_eq!(mgr.unread_count(incoming), 4);
    assert_eq!(mgr.total_unread(), 4);

    // Marking via the *session* id must clear the displayed conversation.
    mgr.mark_chat_read(session_id);
    assert_eq!(mgr.unread_count(incoming), 0);
    assert_eq!(mgr.total_unread(), 0);
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
        mgr.pending_fingerprint().is_some(),
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
        mgr.pending_fingerprint().is_none(),
        "a known fingerprint must not trigger another prompt"
    );
    assert_eq!(
        rx.try_recv().ok(),
        Some(true),
        "a known fingerprint must be auto-confirmed"
    );
}

#[test]
fn blocked_contact_fingerprint_is_auto_rejected() {
    let mut mgr = ChatManager::new(Config::default());
    let contact_id = mgr.add_contact("Mallory".into(), None, Some("BLOCKED-FP".into()), None);
    mgr.block_contact(contact_id).unwrap();

    let session_id = Uuid::new_v4();
    let incoming = Uuid::new_v4();
    mgr.create_local_chat_for_test(incoming, "Peer".into());
    mgr.chat_id_mapping.insert(incoming, session_id);
    let (tx, mut rx) = mpsc::unbounded_channel();
    mgr.add_fingerprint_confirm_sender_for_test(session_id, tx);

    mgr.handle_tofu_verification(session_id, "BLOCKED-FP", "Mallory", "");

    assert!(
        mgr.pending_fingerprint().is_none(),
        "a blocked fingerprint must not prompt the user"
    );
    assert_eq!(
        rx.try_recv().ok(),
        Some(false),
        "a blocked fingerprint must be rejected automatically"
    );
}

#[test]
fn blocking_tears_down_the_live_session_completely() {
    let mut mgr = ChatManager::new(Config::default());
    let contact_id = mgr.add_contact("Mallory".into(), None, Some("BLOCKED-FP".into()), None);

    // Live session: chat with the blocked fingerprint, mapped to a session id
    // with a handle, an event receiver, and a confirm sender.
    let session_id = Uuid::new_v4();
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Mallory".into());
    mgr.get_chat_mut(chat_id).unwrap().peer_fingerprint = Some("BLOCKED-FP".into());
    mgr.chat_id_mapping.insert(chat_id, session_id);
    let (app_tx, _app_rx) = mpsc::unbounded_channel();
    let (file_tx, _file_rx) = mpsc::channel(4);
    mgr.add_session_for_test(
        session_id,
        SessionHandle {
            from_app_tx: app_tx,
            file_tx,
        },
    );
    let (event_tx, event_rx) = mpsc::unbounded_channel::<SessionEvent>();
    mgr.session_events
        .insert(session_id, Arc::new(Mutex::new(event_rx)));
    let (confirm_tx, _confirm_rx) = mpsc::unbounded_channel();
    mgr.add_fingerprint_confirm_sender_for_test(session_id, confirm_tx);

    mgr.block_contact(contact_id).unwrap();

    // Everything about the session must be gone: without this, the network
    // task's events kept being polled and the blocked peer could still
    // deliver messages on the established session.
    assert!(!mgr.sessions.contains_key(&session_id));
    assert!(!mgr.session_events.contains_key(&session_id));
    assert!(!mgr.fingerprint_confirm_senders.contains_key(&session_id));
    assert!(!mgr.chat_id_mapping.values().any(|v| *v == session_id));
    // The dropped receiver is what ends the network task's loop.
    assert!(event_tx.send(SessionEvent::Disconnected).is_err());
}

#[test]
fn tofu_accept_promotes_contact_to_verified() {
    let mut mgr = ChatManager::new(Config::default());
    // Contact known by name but not yet verified, bound to the chat.
    let contact_id = mgr.add_contact("Alice".into(), None, None, None);
    let session_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(session_id, "Alice".into());
    mgr.associate_contact_with_chat(contact_id, session_id);
    let (tx, _rx) = mpsc::unbounded_channel();
    mgr.add_fingerprint_confirm_sender_for_test(session_id, tx);

    mgr.handle_tofu_verification(session_id, "ALICE-FP", "Alice", "");
    assert!(mgr.pending_fingerprint().is_some());

    mgr.confirm_fingerprint(session_id, true).unwrap();

    let contact = mgr.get_contact(contact_id).unwrap();
    assert_eq!(contact.trust_state, TrustState::Verified);
    assert_eq!(contact.fingerprint.as_deref(), Some("ALICE-FP"));
}

#[test]
fn unblock_restores_verified_when_fingerprint_confirmed() {
    let mut mgr = ChatManager::new(Config::default());
    let contact_id = mgr.add_contact("Bob".into(), None, Some("BOB-FP".into()), None);
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Bob".into());
    mgr.get_chat_mut(chat_id).unwrap().peer_fingerprint = Some("BOB-FP".into());

    mgr.block_contact(contact_id).unwrap();
    assert_eq!(
        mgr.get_contact(contact_id).unwrap().trust_state,
        TrustState::Blocked
    );

    mgr.unblock_contact(contact_id).unwrap();
    assert_eq!(
        mgr.get_contact(contact_id).unwrap().trust_state,
        TrustState::Verified,
        "confirmed fingerprint restores Verified after unblock"
    );
}

#[tokio::test]
async fn connect_to_blocked_contact_is_refused() {
    let mut mgr = ChatManager::new(Config::default());
    let contact_id = mgr.add_contact(
        "Mallory".into(),
        Some("127.0.0.1:1".into()),
        Some("BLOCKED-FP".into()),
        None,
    );
    mgr.block_contact(contact_id).unwrap();

    let privkey = rsa::RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 512).unwrap();
    let err = mgr
        .connect_to_contact(contact_id, None, &privkey)
        .await
        .expect_err("connecting to a blocked contact must fail");
    assert!(err.to_string().contains("blocked"));
}

#[test]
fn parse_invite_with_valid_address_keeps_it() {
    let mgr = ChatManager::default();

    let link = v1_invite_link("Peer", "127.0.0.1:54321");

    let contact = mgr.parse_invite_link(&link).expect("should parse invite");
    assert_eq!(contact.address, Some("127.0.0.1:54321".to_string()));
}

#[test]
fn parse_invite_invalid_address_no_port() {
    let mgr = ChatManager::default();

    let link = v1_invite_link("Peer", "127.0.0.1");

    let contact = mgr.parse_invite_link(&link).expect("should parse invite");
    assert!(
        contact.address.is_none(),
        "address without port should be None"
    );
}

#[test]
fn parse_invite_invalid_address_bad_port() {
    let mgr = ChatManager::default();

    let link = v1_invite_link("Peer", "127.0.0.1:notaport");

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
        read_count: 0,
        title_is_custom: false,
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
        read_count: 0,
        title_is_custom: false,
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
    assert!(mgr.pending_fingerprint().is_some());
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
    assert!(mgr.pending_fingerprint().is_none());
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
    assert!(mgr.pending_fingerprint().is_some());
    let fp = mgr.pending_fingerprint().unwrap().fingerprint.clone();
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
        read_count: 0,
        title_is_custom: false,
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
        mgr.pending_fingerprint().is_some(),
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
    let link = v1_invite_link("Legacy User", "192.168.1.50:8001");

    // Should still parse v1 unsigned invites (backward compatibility)
    let contact = mgr
        .parse_invite_link(&link)
        .expect("should parse v1 invite");
    assert_eq!(contact.name, "Legacy User");
    assert_eq!(contact.address, Some("192.168.1.50:8001".to_string()));
    assert_eq!(
        contact.fingerprint,
        Some(crate::core::crypto::fingerprint_pubkey(
            TEST_PUBKEY_PEM.as_bytes()
        ))
    );
    // …and it is *not* signed, which is what the UI warns about.
    assert!(!ChatManager::invite_link_is_signed(&link));
}

/// The signature on a v2 invite only proves that whoever built it holds the
/// private key for the key *inside* it. `fingerprint` is a separate field, and
/// it is the one every trust decision is made against — so an invite whose two
/// halves disagree must be refused rather than imported.
#[test]
fn an_invite_whose_fingerprint_does_not_match_its_key_is_rejected() {
    let mgr = ChatManager::default();
    let payload = serde_json::json!({
        "name": "Impostor",
        "address": "127.0.0.1:12345",
        "fingerprint": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        "public_key": TEST_PUBKEY_PEM,
    });
    let json = serde_json::to_string(&payload).unwrap();
    let encoded = base64::engine::general_purpose::STANDARD.encode(json);
    let link = format!("chat-p2p://invite/{}", encoded);

    let err = mgr
        .parse_invite_link(&link)
        .expect_err("a fingerprint that does not belong to the key must be refused");
    assert!(
        err.to_string().contains("inconsistent"),
        "the error should say what is wrong: {err}"
    );
}

/// Importing an invite must not pre-trust the fingerprint it names. It used to:
/// any contact carrying the fingerprint made the peer "known", so they
/// connected with no verification prompt at all.
#[test]
fn importing_an_invite_does_not_bypass_fingerprint_verification() {
    let mut mgr = ChatManager::new(Config::default());
    let fingerprint = "ab".repeat(32);
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Peer".into());

    // A contact straight from an invite link: fingerprint known, but nobody has
    // compared a safety code yet.
    let mut contact = sample_contact_for_test("Mum");
    contact.fingerprint = Some(fingerprint.clone());
    contact.trust_state = TrustState::Unverified;
    mgr.import_contact(contact).unwrap();

    let (tx, _rx) = mpsc::unbounded_channel();
    mgr.fingerprint_confirm_senders.insert(chat_id, tx);
    mgr.handle_tofu_verification(chat_id, &fingerprint, "Mum", "123456 🎃🎈🎁");

    assert!(
        mgr.pending_fingerprint().is_some(),
        "an unverified imported contact must still be verified on first contact"
    );
    assert!(
        mgr.get_chat(chat_id).unwrap().peer_fingerprint.is_none(),
        "trust must not be stored before the user accepts"
    );

    // Once that fingerprint *has* been verified, the peer is a returning one
    // and is not prompted again in another conversation.
    let second = Uuid::new_v4();
    mgr.create_local_chat_for_test(second, "Mum (2)".into());
    if let Some(c) = mgr.contacts.values_mut().next() {
        c.trust_state = TrustState::Verified;
    }
    let (tx2, _rx2) = mpsc::unbounded_channel();
    mgr.fingerprint_confirm_senders.insert(second, tx2);
    mgr.handle_tofu_verification(second, &fingerprint, "Mum", "123456 🎃🎈🎁");
    assert_eq!(
        mgr.get_chat(second).unwrap().peer_fingerprint.as_deref(),
        Some(fingerprint.as_str()),
        "a verified contact is a returning peer and is auto-accepted"
    );
}

/// Deleting a contact promised, in the confirmation dialog, that the peer would
/// have to be verified again. It has to actually be true.
#[test]
fn deleting_a_contact_also_revokes_its_stored_trust() {
    let mut mgr = ChatManager::new(Config::default());
    let fingerprint = "cd".repeat(32);
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Peer".into());
    mgr.chats.get_mut(&chat_id).unwrap().peer_fingerprint = Some(fingerprint.clone());

    let mut contact = sample_contact_for_test("Blocked Peer");
    contact.fingerprint = Some(fingerprint.clone());
    contact.trust_state = TrustState::Blocked;
    let id = mgr.import_contact(contact).unwrap();

    assert!(
        mgr.deleting_contact_would_unblock(id),
        "the UI must be able to warn"
    );
    mgr.remove_contact(id);

    assert!(
        mgr.get_chat(chat_id).unwrap().peer_fingerprint.is_none(),
        "the verified fingerprint must go with the contact"
    );
    assert!(
        !mgr.is_fingerprint_blocked(&fingerprint),
        "deleting a blocked contact does lift the block — the dialog says so"
    );
}

/// Pasting the same invite twice used to add a second identical card, and
/// pasting your own added you to your own contacts.
#[test]
fn importing_an_invite_is_idempotent_and_refuses_your_own() {
    let mut mgr = ChatManager::new(Config::default());
    let fingerprint = "ef".repeat(32);

    let mut first = sample_contact_for_test("Sam");
    first.fingerprint = Some(fingerprint.clone());
    let a = mgr.import_contact(first).unwrap();

    let mut again = sample_contact_for_test("Sam (new address)");
    again.fingerprint = Some(fingerprint.clone());
    again.address = Some("10.0.0.9:12345".to_string());
    let b = mgr.import_contact(again).unwrap();

    assert_eq!(a, b, "the same peer must not become two contacts");
    assert_eq!(mgr.contacts.len(), 1);
    assert_eq!(
        mgr.get_contact(a).unwrap().address.as_deref(),
        Some("10.0.0.9:12345"),
        "a re-import refreshes how to reach them"
    );

    mgr.set_my_fingerprint(fingerprint.clone());
    let mut mine = sample_contact_for_test("Me");
    mine.fingerprint = Some(fingerprint);
    assert!(
        mgr.import_contact(mine).is_err(),
        "your own invite is not a contact"
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

/// Drain frames from the streaming send until `FileEnd` (or timeout), so the
/// spawned chunk task has a chance to run.
async fn drain_file_send(rx: &mut mpsc::Receiver<ProtocolMessage>) -> Vec<ProtocolMessage> {
    let mut frames = Vec::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while let Ok(Some(msg)) = tokio::time::timeout_at(deadline, rx.recv()).await {
        let done = matches!(
            msg,
            ProtocolMessage::FileEnd { .. } | ProtocolMessage::FileCancel { .. }
        );
        frames.push(msg);
        if done {
            break;
        }
    }
    frames
}

/// Build a `SessionHandle` plus its two receivers (control lane, bounded file
/// lane) for tests that drive `send_file` without a real session loop.
fn test_session_handle() -> (
    SessionHandle,
    mpsc::UnboundedReceiver<ProtocolMessage>,
    mpsc::Receiver<ProtocolMessage>,
) {
    let (from_app_tx, control_rx) = mpsc::unbounded_channel();
    let (file_tx, file_rx) = mpsc::channel(super::FILE_LANE_CAPACITY);
    (
        SessionHandle {
            from_app_tx,
            file_tx,
        },
        control_rx,
        file_rx,
    )
}

#[tokio::test]
async fn send_file_streams_meta_chunks_and_end_and_tracks_transfer() {
    let mut mgr = ChatManager::new(Config::default());
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "File Send Test".to_string());

    let (handle, _control_rx, mut file_rx) = test_session_handle();
    mgr.sessions.insert(chat_id, handle);

    let temp_file = tempfile::NamedTempFile::new().unwrap();
    let content = vec![b'x'; crate::FILE_CHUNK_SIZE * 2 + 13];
    std::fs::write(temp_file.path(), content).unwrap();

    mgr.send_file(chat_id, temp_file.path().to_path_buf())
        .await
        .expect("send_file should succeed");

    // An outgoing transfer is registered for the UI immediately.
    let outgoing = mgr
        .active_transfers_snapshot()
        .into_iter()
        .find(|t| t.direction == TransferDirection::Outgoing)
        .expect("outgoing transfer should be tracked");
    assert_eq!(outgoing.chat_id, chat_id);

    // Meta/chunks/End ride the bounded file lane, in order. (The session loop
    // owns wire sequencing, so ChatManager stamps placeholder seqs here.)
    let frames = drain_file_send(&mut file_rx).await;
    assert!(
        matches!(frames.first(), Some(ProtocolMessage::FileMeta { .. })),
        "first frame must be FileMeta"
    );
    assert!(
        matches!(frames.last(), Some(ProtocolMessage::FileEnd { .. })),
        "last frame must be FileEnd"
    );
    let chunks = frames
        .iter()
        .filter(|m| matches!(m, ProtocolMessage::FileChunk { .. }))
        .count();
    assert!(chunks >= 3, "multi-chunk file should emit >= 3 chunks");
}

#[tokio::test]
async fn cancel_outgoing_transfer_stops_stream_and_emits_file_cancel() {
    let mut mgr = ChatManager::new(Config::default());
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Cancel Test".to_string());

    let (handle, mut control_rx, mut file_rx) = test_session_handle();
    mgr.sessions.insert(chat_id, handle);

    let temp_file = tempfile::NamedTempFile::new().unwrap();
    // Large enough that streaming does not finish instantly.
    let content = vec![b'y'; crate::FILE_CHUNK_SIZE * 50];
    std::fs::write(temp_file.path(), content).unwrap();

    mgr.send_file(chat_id, temp_file.path().to_path_buf())
        .await
        .expect("send_file should succeed");

    let transfer_id = mgr
        .active_transfers_snapshot()
        .into_iter()
        .find(|t| t.direction == TransferDirection::Outgoing)
        .expect("outgoing transfer tracked")
        .id;

    // Keep draining the bounded file lane so the (otherwise backpressured)
    // stream task keeps cycling and can observe the cancel flag; assert it never
    // reaches FileEnd.
    let drain = tokio::spawn(async move {
        while let Some(msg) = file_rx.recv().await {
            assert!(
                !matches!(msg, ProtocolMessage::FileEnd { .. }),
                "stream should not complete after cancellation"
            );
        }
    });

    // Cancel it. The stream stops and a FileCancel goes out on the control lane.
    mgr.cancel_transfer(transfer_id);

    let mut saw_cancel = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while let Ok(Some(msg)) = tokio::time::timeout_at(deadline, control_rx.recv()).await {
        if matches!(msg, ProtocolMessage::FileCancel { .. }) {
            saw_cancel = true;
            break;
        }
    }
    drain.abort();
    assert!(saw_cancel, "cancellation must emit a FileCancel frame");

    let status = mgr
        .active_transfers_snapshot()
        .into_iter()
        .find(|t| t.id == transfer_id)
        .map(|t| t.status);
    assert_eq!(status, Some(TransferStatus::Cancelled));
}

/// The bounded file lane must pace the reader: with the consumer stalled, the
/// stream task can only run a bounded number of chunks ahead instead of
/// buffering the whole file in memory.
#[tokio::test]
async fn outgoing_stream_is_backpressured_by_the_bounded_lane() {
    let mut mgr = ChatManager::new(Config::default());
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Backpressure".to_string());

    let (handle, _control_rx, mut file_rx) = test_session_handle();
    mgr.sessions.insert(chat_id, handle);

    let total_chunks = 100u64;
    let content = vec![b'z'; crate::FILE_CHUNK_SIZE * total_chunks as usize];
    let file_size = content.len() as u64;
    let temp_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(temp_file.path(), content).unwrap();

    mgr.send_file(chat_id, temp_file.path().to_path_buf())
        .await
        .expect("send_file should succeed");
    let transfer_id = mgr
        .active_transfers_snapshot()
        .into_iter()
        .find(|t| t.direction == TransferDirection::Outgoing)
        .expect("outgoing transfer tracked")
        .id;

    // With nothing draining the lane, give the task time to run as far ahead as
    // it can, then confirm it is capped near the lane capacity — not the file.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    mgr.sync_outgoing_transfer_progress();
    let sent = |mgr: &ChatManager| {
        mgr.active_transfers_snapshot()
            .into_iter()
            .find(|t| t.id == transfer_id)
            .map(|t| t.received)
            .unwrap_or(0)
    };
    let stalled = sent(&mgr);
    let cap_bytes = super::FILE_LANE_CAPACITY as u64 * crate::FILE_CHUNK_SIZE as u64;
    assert!(
        stalled <= cap_bytes,
        "backpressure must cap in-flight bytes at ~lane capacity; got {stalled} > {cap_bytes}"
    );
    assert!(
        stalled < file_size,
        "the whole file must not be buffered when the consumer is stalled"
    );

    // Draining the lane must let the producer make further progress.
    for _ in 0..20 {
        let _ = file_rx.recv().await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    mgr.sync_outgoing_transfer_progress();
    assert!(
        sent(&mgr) > stalled,
        "draining the lane must let the backpressured reader advance"
    );
}

/// Queueing frames on the session is not delivery: the "File sent" toast
/// must wait for the session's wire-level confirmation event.
#[tokio::test]
async fn file_sent_toast_waits_for_wire_confirmation() {
    let mut mgr = ChatManager::new(Config::default());
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Honest Send".to_string());

    let (handle, _control_rx, _file_rx) = test_session_handle();
    mgr.sessions.insert(chat_id, handle);

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

    let (handle, _control_rx, _file_rx) = test_session_handle();
    mgr.sessions.insert(chat_id, handle);

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
        // This test covers the frictionless path; the acceptance gate has its
        // own tests below.
        auto_accept_files: true,
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

/// Build a manager with the acceptance gate on (auto_accept_files = false)
/// and feed it a complete incoming file (Meta + one chunk + End).
fn manager_with_held_incoming_file(temp_dir: &std::path::Path) -> (ChatManager, Uuid, Vec<u8>) {
    let config = Config {
        download_dir: temp_dir.join("downloads"),
        temp_dir: temp_dir.join("temp"),
        auto_accept_files: false,
        ..Config::default()
    };
    let mut mgr = ChatManager::new(config);
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Gated Files".to_string());

    let payload = b"held until accepted".to_vec();
    mgr.handle_session_event(
        chat_id,
        SessionEvent::MessageReceived(ProtocolMessage::FileMeta {
            filename: "offer.txt".to_string(),
            size: payload.len() as u64,
            seq: 1,
        }),
    );
    mgr.handle_session_event(
        chat_id,
        SessionEvent::MessageReceived(ProtocolMessage::FileChunk {
            chunk: payload.clone(),
            seq: 2,
        }),
    );
    mgr.handle_session_event(
        chat_id,
        SessionEvent::MessageReceived(ProtocolMessage::FileEnd { seq: 3 }),
    );
    (mgr, chat_id, payload)
}

#[test]
fn incoming_file_is_held_until_accepted() {
    let temp_dir = tempdir().unwrap();
    let (mut mgr, chat_id, payload) = manager_with_held_incoming_file(temp_dir.path());

    // Fully streamed, but not accepted: no chat message, no file in downloads,
    // transfer still awaiting.
    let transfers = mgr.active_transfers_snapshot();
    assert_eq!(transfers.len(), 1);
    assert_eq!(transfers[0].status, TransferStatus::AwaitingAcceptance);
    assert!(
        !mgr.chats
            .get(&chat_id)
            .unwrap()
            .messages
            .iter()
            .any(|m| { matches!(m.content, MessageContent::File { .. }) }),
        "no file message before acceptance"
    );
    assert!(
        !temp_dir.path().join("downloads").join("offer.txt").exists(),
        "file must not land in downloads before acceptance"
    );

    // Accept → finalized into downloads + recorded in the chat.
    mgr.accept_incoming_file(transfers[0].id).unwrap();
    let final_path = temp_dir.path().join("downloads").join("offer.txt");
    assert_eq!(std::fs::read(&final_path).unwrap(), payload);
    assert!(mgr.chats.get(&chat_id).unwrap().messages.iter().any(|m| {
        matches!(&m.content, MessageContent::File { filename, .. } if filename == "offer.txt")
    }));
    assert!(mgr.active_transfers.is_empty());
    assert!(mgr.incoming_files.is_empty());
}

#[test]
fn declined_incoming_file_is_deleted_and_discarded() {
    let temp_dir = tempdir().unwrap();
    let (mut mgr, chat_id, _payload) = manager_with_held_incoming_file(temp_dir.path());

    let transfer_id = mgr.active_transfers_snapshot()[0].id;
    mgr.reject_incoming_file(transfer_id).unwrap();

    // Spool deleted, nothing in downloads, no chat message, status cancelled.
    assert!(mgr.incoming_files.is_empty());
    let downloads = temp_dir.path().join("downloads");
    let spooled: Vec<_> = std::fs::read_dir(&downloads)
        .map(|it| it.flatten().collect())
        .unwrap_or_default();
    assert!(
        spooled.is_empty(),
        "declined transfer must leave nothing on disk, found: {:?}",
        spooled
    );
    assert!(!mgr
        .chats
        .get(&chat_id)
        .unwrap()
        .messages
        .iter()
        .any(|m| { matches!(m.content, MessageContent::File { .. }) }));
    assert_eq!(
        mgr.active_transfers_snapshot()[0].status,
        TransferStatus::Cancelled
    );

    // Late chunks for the declined transfer are discarded (no new spool file).
    mgr.handle_session_event(
        chat_id,
        SessionEvent::MessageReceived(ProtocolMessage::FileChunk {
            chunk: b"late".to_vec(),
            seq: 4,
        }),
    );
    assert!(mgr.incoming_files.is_empty());

    // Double-decline / accept-after-decline are rejected cleanly.
    assert!(mgr.reject_incoming_file(transfer_id).is_err());
    assert!(mgr.accept_incoming_file(transfer_id).is_err());

    // A fresh offer afterwards works (cancelled state is cleared).
    mgr.handle_session_event(
        chat_id,
        SessionEvent::MessageReceived(ProtocolMessage::FileMeta {
            filename: "second.txt".to_string(),
            size: 4,
            seq: 5,
        }),
    );
    assert_eq!(
        mgr.active_transfers_snapshot()
            .iter()
            .filter(|t| t.status == TransferStatus::AwaitingAcceptance)
            .count(),
        1
    );
}

#[test]
fn accept_before_file_end_continues_transfer() {
    let temp_dir = tempdir().unwrap();
    let config = Config {
        download_dir: temp_dir.path().join("downloads"),
        temp_dir: temp_dir.path().join("temp"),
        auto_accept_files: false,
        ..Config::default()
    };
    let mut mgr = ChatManager::new(config);
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Early Accept".to_string());

    let payload = b"accepted mid-stream";
    mgr.handle_session_event(
        chat_id,
        SessionEvent::MessageReceived(ProtocolMessage::FileMeta {
            filename: "early.txt".to_string(),
            size: payload.len() as u64,
            seq: 1,
        }),
    );

    // Accept while the stream is still going.
    let transfer_id = mgr.active_transfers_snapshot()[0].id;
    mgr.accept_incoming_file(transfer_id).unwrap();

    mgr.handle_session_event(
        chat_id,
        SessionEvent::MessageReceived(ProtocolMessage::FileChunk {
            chunk: payload.to_vec(),
            seq: 2,
        }),
    );
    mgr.handle_session_event(
        chat_id,
        SessionEvent::MessageReceived(ProtocolMessage::FileEnd { seq: 3 }),
    );

    let final_path = temp_dir.path().join("downloads").join("early.txt");
    assert_eq!(std::fs::read(&final_path).unwrap(), payload.to_vec());
    assert!(mgr.active_transfers.is_empty());
}

#[test]
fn sent_text_is_marked_delivered_by_peer_ack() {
    let mut mgr = ChatManager::new(Config::default());
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Receipts".to_string());
    let (tx, mut _rx) = mpsc::unbounded_channel();
    let (file_tx, _file_rx) = mpsc::channel(4);
    mgr.add_session_for_test(
        chat_id,
        SessionHandle {
            from_app_tx: tx,
            file_tx,
        },
    );

    mgr.send_message(chat_id, "hello".to_string()).unwrap();
    let msg_id = mgr.chats.get(&chat_id).unwrap().messages[0].id;
    assert!(!mgr.chats.get(&chat_id).unwrap().messages[0].delivered);

    // The session loop reports the wire seq it stamped on the frame…
    mgr.handle_session_event(chat_id, SessionEvent::TextSendComplete { seq: 7 });
    // …and the peer acknowledges that seq.
    mgr.handle_session_event(
        chat_id,
        SessionEvent::MessageReceived(ProtocolMessage::Ack {
            acked_seq: 7,
            seq: 1,
        }),
    );

    let msg = &mgr.chats.get(&chat_id).unwrap().messages[0];
    assert_eq!(msg.id, msg_id);
    assert!(msg.delivered, "peer ack must mark the message delivered");

    // A replayed/duplicate ack is harmless.
    mgr.handle_session_event(
        chat_id,
        SessionEvent::MessageReceived(ProtocolMessage::Ack {
            acked_seq: 7,
            seq: 2,
        }),
    );
    assert!(mgr.chats.get(&chat_id).unwrap().messages[0].delivered);
}

#[test]
fn received_text_queues_a_delivery_receipt() {
    let mut mgr = ChatManager::new(Config::default());
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Receipts".to_string());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (file_tx, _file_rx) = mpsc::channel(4);
    mgr.add_session_for_test(
        chat_id,
        SessionHandle {
            from_app_tx: tx,
            file_tx,
        },
    );

    mgr.handle_session_event(
        chat_id,
        SessionEvent::MessageReceived(ProtocolMessage::Text {
            text: "hi".to_string(),
            timestamp: 1,
            seq: 5,
        }),
    );

    match rx.try_recv() {
        Ok(ProtocolMessage::Ack { acked_seq, .. }) => assert_eq!(acked_seq, 5),
        other => panic!("expected a queued Ack for seq 5, got {:?}", other),
    }
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
    mgr.queue_fingerprint_request(PendingFingerprint {
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
    assert!(mgr.pending_fingerprint().is_none());
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

// ── Regressions: session teardown, TOFU queueing, reconnect, transfers ──────

/// Build a live session (handle + event receiver + confirm sender) for `id`.
/// Returns the receiving end of the app→session control lane so a test can
/// assert on the frames the manager queued.
fn wire_session_for_test(
    mgr: &mut ChatManager,
    session_id: Uuid,
) -> mpsc::UnboundedReceiver<ProtocolMessage> {
    let (app_tx, app_rx) = mpsc::unbounded_channel();
    let (file_tx, _file_rx) = mpsc::channel(4);
    mgr.add_session_for_test(
        session_id,
        SessionHandle {
            from_app_tx: app_tx,
            file_tx,
        },
    );
    let (_event_tx, event_rx) = mpsc::unbounded_channel::<SessionEvent>();
    mgr.session_events
        .insert(session_id, Arc::new(Mutex::new(event_rx)));
    let (confirm_tx, _confirm_rx) = mpsc::unbounded_channel();
    mgr.add_fingerprint_confirm_sender_for_test(session_id, confirm_tx);
    app_rx
}

/// Deleting a host-side conversation must close the session serving it. That
/// session is keyed by the *placeholder* id, not the chat's own — so removing
/// only `chats[chat_id]` left the socket up and the mapping intact, and the
/// peer's later messages were dropped with a log line while their client still
/// showed them as sent.
#[test]
fn deleting_a_host_side_chat_closes_its_mapped_session() {
    let mut mgr = ChatManager::default();
    let session_id = Uuid::new_v4();
    let incoming = Uuid::new_v4();
    mgr.create_local_chat_for_test(incoming, "Peer".into());
    mgr.chat_id_mapping.insert(incoming, session_id);
    let _app_rx = wire_session_for_test(&mut mgr, session_id);

    mgr.delete_chat(incoming);

    assert!(!mgr.chats.contains_key(&incoming));
    assert!(
        !mgr.sessions.contains_key(&session_id),
        "the session behind the deleted chat must be closed"
    );
    assert!(!mgr.session_events.contains_key(&session_id));
    assert!(!mgr.fingerprint_confirm_senders.contains_key(&session_id));
    assert!(
        mgr.chat_id_mapping.is_empty(),
        "a stale mapping would keep routing the peer's frames to nothing"
    );
}

/// Two peers mid-handshake at once must each get a prompt. With a single slot
/// the second overwrote the first, and that session sat blocked until its
/// 30-minute confirmation timeout with nothing on screen to explain it.
#[test]
fn concurrent_tofu_prompts_queue_instead_of_overwriting() {
    let mut mgr = ChatManager::default();
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    // Receivers must outlive the assertions: dropping one closes the channel
    // and `confirm_fingerprint` would fail for reasons unrelated to the queue.
    let mut confirm_rx = Vec::new();
    for (session, name) in [(first, "Peer A"), (second, "Peer B")] {
        mgr.create_local_chat_for_test(session, name.into());
        let (tx, rx) = mpsc::unbounded_channel();
        mgr.add_fingerprint_confirm_sender_for_test(session, tx);
        confirm_rx.push(rx);
    }

    mgr.handle_tofu_verification(first, "FP-A", "Peer A", "11-11-11");
    mgr.handle_tofu_verification(second, "FP-B", "Peer B", "22-22-22");

    assert_eq!(mgr.pending_fingerprint_count(), 2);
    assert_eq!(
        mgr.pending_fingerprint().unwrap().session_id,
        first,
        "the first peer to arrive is answered first"
    );

    // Answering the first surfaces the second rather than losing it.
    mgr.confirm_fingerprint(first, true).unwrap();
    assert_eq!(mgr.pending_fingerprint_count(), 1);
    assert_eq!(mgr.pending_fingerprint().unwrap().session_id, second);
    assert_eq!(
        mgr.get_chat(first).unwrap().peer_fingerprint.as_deref(),
        Some("FP-A"),
        "accepting must persist that peer's fingerprint"
    );

    // Rejecting the second clears the queue (that session is about to die).
    mgr.confirm_fingerprint(second, false).unwrap();
    assert_eq!(mgr.pending_fingerprint_count(), 0);
    drop(confirm_rx);
}

/// A prompt for a session that died can never be answered; leaving it queued
/// would block every peer behind it.
#[test]
fn a_dead_session_drops_its_queued_tofu_prompt() {
    let mut mgr = ChatManager::default();
    let session_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(session_id, "Peer".into());
    let (tx, _rx) = mpsc::unbounded_channel();
    mgr.add_fingerprint_confirm_sender_for_test(session_id, tx);
    mgr.handle_tofu_verification(session_id, "FP", "Peer", "");
    assert_eq!(mgr.pending_fingerprint_count(), 1);

    mgr.handle_session_event(session_id, SessionEvent::Disconnected);

    assert_eq!(mgr.pending_fingerprint_count(), 0);
}

/// Reconnecting must not relabel a conversation the user named themselves.
#[test]
fn reconnect_keeps_a_user_chosen_title() {
    let mut mgr = ChatManager::default();
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "192.168.1.20:12345".into());

    mgr.rename_chat(chat_id, "Mum".into()).unwrap();
    assert!(mgr.get_chat(chat_id).unwrap().title_is_custom);

    mgr.handle_session_event(
        chat_id,
        SessionEvent::Connected {
            peer: "192.168.1.20:12345".into(),
        },
    );

    assert_eq!(mgr.get_chat(chat_id).unwrap().title, "Mum");
}

/// A conversation the user has NOT renamed still gets the peer label, so the
/// fix does not freeze auto-generated titles.
#[test]
fn reconnect_still_labels_an_unnamed_chat_with_the_peer() {
    let mut mgr = ChatManager::default();
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "connecting".into());

    mgr.handle_session_event(
        chat_id,
        SessionEvent::Connected {
            peer: "10.0.0.5:9000".into(),
        },
    );

    assert_eq!(mgr.get_chat(chat_id).unwrap().title, "10.0.0.5:9000");
}

/// The wire sequence restarts at 1 for every session, but `recv_seq` lives on
/// the Chat. Left alone, everything the peer sends after a reconnect is at or
/// below the previous session's high-water mark and is discarded as a replay.
#[test]
fn reconnect_resets_the_replay_high_water_mark() {
    let mut mgr = ChatManager::default();
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Peer".into());
    mgr.get_chat_mut(chat_id).unwrap().recv_seq = 42;

    mgr.handle_session_event(
        chat_id,
        SessionEvent::Connected {
            peer: "10.0.0.5:9000".into(),
        },
    );
    assert_eq!(mgr.get_chat(chat_id).unwrap().recv_seq, 0);

    // And the peer's first message on the new session is accepted.
    mgr.handle_session_event(
        chat_id,
        SessionEvent::MessageReceived(ProtocolMessage::Text {
            text: "first message after reconnect".into(),
            timestamp: 0,
            seq: 1,
        }),
    );
    assert_eq!(mgr.get_chat(chat_id).unwrap().messages.len(), 1);
}

/// Same, host side: a returning peer reuses its chat id, so the entry already
/// exists and its counter must be reset along with the new session.
#[test]
fn host_side_reconnect_resets_the_replay_high_water_mark() {
    let mut mgr = ChatManager::default();
    let session_id = Uuid::new_v4();
    let incoming = Uuid::new_v4();
    mgr.create_local_chat_for_test(incoming, "Peer".into());
    mgr.get_chat_mut(incoming).unwrap().recv_seq = 99;
    let (tx, _rx) = mpsc::unbounded_channel();
    mgr.add_fingerprint_confirm_sender_for_test(session_id, tx);

    mgr.handle_session_event(
        session_id,
        SessionEvent::NewConnection {
            peer_addr: "10.0.0.5:40000".into(),
            fingerprint: "FP".into(),
            sas: String::new(),
            chat_id: incoming,
        },
    );

    assert_eq!(mgr.get_chat(incoming).unwrap().recv_seq, 0);
}

/// Declining an incoming file must tell the sender to stop. Without the
/// FileCancel the decision only ever reached our own disk: a declined 5 GB
/// offer still crossed the wire in full.
#[test]
fn declining_a_file_tells_the_sender_to_stop() {
    let mut mgr = ChatManager::default();
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Peer".into());
    let mut app_rx = wire_session_for_test(&mut mgr, chat_id);
    // auto_accept off (the default) holds the transfer for the user's decision.
    let transfer_id = mgr
        .start_receiving_file(chat_id, "huge.iso", 5_000_000_000)
        .unwrap();

    mgr.reject_incoming_file(transfer_id).unwrap();

    let frame = app_rx.try_recv().expect("a FileCancel must be queued");
    assert!(
        matches!(frame, ProtocolMessage::FileCancel { .. }),
        "expected FileCancel, got {:?}",
        frame
    );
}

/// A disconnect mid-receive must fail the transfer, not leave a progress row
/// stuck at its last byte count with the chat's incoming slot still occupied.
#[test]
fn disconnect_fails_an_in_flight_incoming_transfer() {
    let mut mgr = ChatManager::default();
    let session_id = Uuid::new_v4();
    let incoming = Uuid::new_v4();
    mgr.create_local_chat_for_test(incoming, "Peer".into());
    mgr.chat_id_mapping.insert(incoming, session_id);
    let _app_rx = wire_session_for_test(&mut mgr, session_id);
    let transfer_id = mgr
        .start_receiving_file(incoming, "big.bin", 1_000_000)
        .unwrap();

    mgr.handle_session_event(session_id, SessionEvent::Disconnected);

    let state = mgr
        .active_transfers_snapshot()
        .into_iter()
        .find(|t| t.id == transfer_id)
        .expect("the transfer is still listed, so its state must be honest");
    assert!(
        matches!(state.status, TransferStatus::Failed(_)),
        "expected Failed, got {:?}",
        state.status
    );
    assert!(
        mgr.active_incoming_transfer_id_for_chat(incoming).is_none(),
        "the chat's incoming slot must be free so the peer can retry"
    );
}

/// Two concurrent sends on one conversation interleave their chunks on the
/// wire (`FileChunk` carries no transfer id) and corrupt both files, so the
/// second must be refused rather than started.
#[tokio::test]
async fn a_second_concurrent_file_send_is_refused() {
    let dir = tempdir().unwrap();
    let a = dir.path().join("a.bin");
    let b = dir.path().join("b.bin");
    std::fs::write(&a, vec![1u8; 4096]).unwrap();
    std::fs::write(&b, vec![2u8; 4096]).unwrap();

    let mut mgr = ChatManager::default();
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Peer".into());
    let (app_tx, _app_rx) = mpsc::unbounded_channel();
    // Keep the file lane's receiver alive so the first send can queue FileMeta.
    let (file_tx, _file_rx) = mpsc::channel(FILE_LANE_CAPACITY);
    mgr.add_session_for_test(
        chat_id,
        SessionHandle {
            from_app_tx: app_tx,
            file_tx,
        },
    );

    mgr.send_file(chat_id, a)
        .await
        .expect("the first send starts");
    let err = mgr
        .send_file(chat_id, b)
        .await
        .expect_err("a second concurrent send must be refused, not interleaved");
    assert!(
        err.to_string().contains("Still sending"),
        "the error must name the conflict, got: {}",
        err
    );
}
