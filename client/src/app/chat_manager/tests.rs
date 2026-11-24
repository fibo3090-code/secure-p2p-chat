use super::test_support::*;
use super::*;
use crate::identity::Identity;
use base64::Engine;
use tempfile::tempdir;

/// A real RSA-2048 public key for v1 (unsigned) invites: an invite's key is
/// parsed and its fingerprint must match, so a placeholder would be refused.
const TEST_PUBKEY_PEM: &str = "-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAiCkJE4AgjmiHxvGso7R/
Duw96YbDo3cRz1E7ShJzCSw7YnJapuV1yqxQN+hfPYoLrjoEDwa5DYPE8qVabfl4
cwvKyECzoGM0duq8lxRD6/0CLAIlV7QUZla2Zu0Y6eJQ2KDf8fT/CLlZQxAJ8Rua
XYDnjQTPhMIUfbZZFYSEBC/xhgvBP2wDVW/+E+5DUIy0VKvu1fEq+OEPrL3V6qcZ
RlkO7OmIbfSlnx4kC4dGyqX360N7vf8GGycd1ADTWKzs0IAkNr3FOmEKTnSuABL+
RM/QkXnZvmWWRYA5jlFgruDCfhnpq58Fn1IymsosFvHWWCBrADhR6YQEY6D3tP07
pQIDAQAB
-----END PUBLIC KEY-----";

fn test_pubkey_fingerprint() -> String {
    crate::core::crypto::fingerprint_pubkey(TEST_PUBKEY_PEM.as_bytes()).unwrap()
}

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

fn contact_with(name: &str, fingerprint: &str, trust_state: TrustState) -> Contact {
    Contact {
        fingerprint: Some(fingerprint.to_string()),
        trust_state,
        ..sample_contact_for_test(name)
    }
}

/// Encode a v1 (unsigned) invite payload as a link.
fn v1_link(payload: serde_json::Value) -> String {
    let json = serde_json::to_string(&payload).unwrap();
    let encoded = base64::engine::general_purpose::STANDARD.encode(json);
    format!("chat-p2p://invite/{}", encoded)
}

/// A legacy v1 invite link whose fingerprint matches its public key.
fn v1_invite_link(name: &str, address: &str) -> String {
    v1_link(serde_json::json!({
        "name": name,
        "address": address,
        "fingerprint": test_pubkey_fingerprint(),
        "public_key": TEST_PUBKEY_PEM,
    }))
}

fn identity(name: &str) -> Identity {
    Identity::new_with_plaintext(name.to_string()).unwrap()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[derive(serde::Deserialize, serde::Serialize)]
struct SignedInvite {
    payload: serde_json::Value,
    signature: Vec<u8>,
}

/// Decode a v2 link, let `tamper` edit it *without* re-signing, re-encode.
fn tamper_v2(link: &str, tamper: impl FnOnce(&mut SignedInvite)) -> String {
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let encoded = link.strip_prefix("chat-p2p://invite/v2/").unwrap();
    let json = String::from_utf8(b64.decode(encoded).unwrap()).unwrap();
    let mut invite: SignedInvite = serde_json::from_str(&json).unwrap();
    tamper(&mut invite);
    let tampered = serde_json::to_string(&invite).unwrap();
    format!("chat-p2p://invite/v2/{}", b64.encode(&tampered))
}

/// Re-sign a fresh invite with a rewritten timestamp (signature stays valid),
/// so parse-time expiry is exercised independently of signature checks.
fn resign_invite_with_timestamp(identity: &Identity, timestamp: u64) -> String {
    let link = identity.generate_signed_invite_link(None).unwrap();
    crate::identity::invite::resign_for_test(&link, &identity.private_key().unwrap(), |p| {
        p.timestamp = timestamp
    })
    .unwrap()
}

fn show_fingerprint(mgr: &mut ChatManager, chat_id: Uuid, fingerprint: &str, peer_name: &str) {
    mgr.handle_session_event(
        chat_id,
        SessionEvent::ShowFingerprintVerification {
            fingerprint: fingerprint.to_string(),
            peer_name: peer_name.to_string(),
            sas: String::new(),
            chat_id,
        },
    );
}

/// An unsigned v1 link is standard base64, which contains `/`, so it can carry
/// `/v2/` by chance; it must still be reported unsigned and parse as v1.
#[test]
fn a_v1_link_containing_v2_by_chance_is_still_unsigned() {
    // U+FFFF, "o", then a UTF-8 lead byte encode as ..BF BF 6F C3.., which on
    // the right 3-byte alignment is "/v2/" in base64. Try each alignment.
    let link = (0..3)
        .map(|pad| v1_invite_link(&format!("{}\u{FFFF}oé", "x".repeat(pad)), "10.0.0.1:12345"))
        .find(|link| link.contains("/v2/"))
        .expect("one alignment produces /v2/ in the payload");

    assert!(
        !ChatManager::invite_link_is_signed(&link),
        "an unsigned link was reported as signed"
    );
    let contact = ChatManager::default()
        .parse_invite_link(&link)
        .expect("a valid v1 link still parses");
    assert!(contact.name.ends_with("\u{FFFF}oé"));
}

/// Rejecting a *known* conversation's changed key (possible impostor) must not
/// delete it; only a conversation this connection created may go.
#[test]
fn rejecting_a_changed_key_keeps_the_conversation() {
    let (mut mgr, known) = manager_with_chat("Mum");
    {
        let chat = mgr.get_chat_mut(known).unwrap();
        chat.peer_fingerprint = Some("MUMS-REAL-KEY".into());
        chat.messages.push(Message::received(
            MessageContent::Text { text: "hi".into() },
            chrono::Utc::now(),
        ));
    }
    let _rx = confirm_channel(&mut mgr, known);
    mgr.reject_fingerprint(known).unwrap();
    assert!(
        mgr.get_chat(known).is_some(),
        "the conversation was deleted"
    );

    let fresh = Uuid::new_v4();
    mgr.create_local_chat_for_test(fresh, "127.0.0.1:12345".into());
    let _rx = confirm_channel(&mut mgr, fresh);
    mgr.reject_fingerprint(fresh).unwrap();
    assert!(
        mgr.get_chat(fresh).is_none(),
        "a refused first contact leaves nothing behind"
    );
}

#[test]
fn v1_invite_addresses_are_validated() {
    let cases = [
        ("YOUR_IP:PORT", None, "placeholder address must be ignored"),
        (
            "127.0.0.1:54321",
            Some("127.0.0.1:54321"),
            "valid address kept",
        ),
        ("127.0.0.1", None, "address without port should be None"),
        (
            "127.0.0.1:notaport",
            None,
            "address with non-numeric port should be None",
        ),
    ];
    for (address, expected, why) in cases {
        let contact = ChatManager::default()
            .parse_invite_link(&v1_invite_link("Peer", address))
            .expect("should parse invite");
        assert_eq!(contact.address.as_deref(), expected, "{why}");
    }
}

// ── Desktop notifications: the focus gate ───────────────────────────────────

#[test]
fn notifications_are_suppressed_only_for_the_visible_conversation() {
    let (mut mgr, open) = manager_with_chat("Open");
    let other = Uuid::new_v4();
    mgr.create_local_chat_for_test(other, "Other".into());
    assert!(
        mgr.config.enable_notifications,
        "test assumes notifications default to on"
    );
    // A front-end that never reports presence keeps notifying.
    assert!(mgr.should_notify_for(open));

    // Window focused, `open` on screen: only that conversation is silent.
    mgr.set_ui_presence(true, Some(open));
    assert!(!mgr.should_notify_for(open));
    assert!(mgr.should_notify_for(other));

    // Window in the background: everything notifies, even the "open" one.
    mgr.set_ui_presence(false, Some(open));
    assert!(mgr.should_notify_for(open));
    assert!(mgr.should_notify_for(other));

    // The user's setting still wins over any presence state.
    mgr.config.enable_notifications = false;
    mgr.set_ui_presence(false, None);
    assert!(!mgr.should_notify_for(open));
}

/// Presence names the displayed (incoming) chat id; the gate must resolve the
/// session id back to it.
#[test]
fn notification_gate_resolves_incoming_session_ids() {
    let mut mgr = ChatManager::default();
    let (session_id, incoming) = incoming_chat(&mut mgr);
    mgr.set_ui_presence(true, Some(incoming));
    assert!(
        !mgr.should_notify_for(session_id),
        "a message on the session backing the open chat must stay silent"
    );
}

#[test]
fn marking_read_clears_unread_and_resolves_session_ids() {
    let mut mgr = ChatManager::default();
    let (session_id, incoming) = incoming_chat(&mut mgr);
    let chat = mgr.get_chat_mut(incoming).unwrap();
    for _ in 0..4 {
        chat.messages.push(Message::received(
            MessageContent::Text { text: "hi".into() },
            chrono::Utc::now(),
        ));
    }
    assert_eq!(mgr.unread_count(incoming), 4);
    assert_eq!(mgr.total_unread(), 4);

    // Marking via the *session* id must clear the displayed conversation.
    mgr.mark_chat_read(session_id);
    assert_eq!(mgr.unread_count(incoming), 0);
    assert_eq!(mgr.total_unread(), 0);
}

// ── TOFU ────────────────────────────────────────────────────────────────────

/// Run TOFU for `fingerprint` on a new incoming session. Returns whether the
/// user was prompted, and the session's automatic verdict if there was one.
fn incoming_tofu(mgr: &mut ChatManager, fingerprint: &str, sas: &str) -> (bool, Option<bool>) {
    let (session_id, _) = incoming_chat(mgr);
    let mut rx = confirm_channel(mgr, session_id);
    mgr.handle_tofu_verification(session_id, fingerprint, "Peer", sas);
    (mgr.pending_fingerprint().is_some(), rx.try_recv().ok())
}

#[test]
fn host_prompts_for_an_unknown_incoming_fingerprint() {
    let mut mgr = ChatManager::default();
    let (prompted, verdict) = incoming_tofu(&mut mgr, "UNKNOWN-FP", "12-34-56");
    assert!(
        prompted,
        "host must prompt to verify an unknown incoming peer"
    );
    assert_eq!(
        verdict, None,
        "host must not auto-confirm an unknown incoming peer"
    );
}

#[test]
fn host_auto_accepts_a_returning_known_fingerprint() {
    let (mut mgr, prior) = manager_with_chat("Known");
    mgr.get_chat_mut(prior).unwrap().peer_fingerprint = Some("KNOWN-FP".into());
    let (prompted, verdict) = incoming_tofu(&mut mgr, "KNOWN-FP", "12-34-56");
    assert!(
        !prompted,
        "a known fingerprint must not trigger another prompt"
    );
    assert_eq!(
        verdict,
        Some(true),
        "a known fingerprint must be auto-confirmed"
    );
}

#[test]
fn blocked_contact_fingerprint_is_auto_rejected() {
    let mut mgr = ChatManager::default();
    let contact_id = mgr.add_contact("Mallory".into(), None, Some("BLOCKED-FP".into()), None);
    mgr.block_contact(contact_id).unwrap();
    let (prompted, verdict) = incoming_tofu(&mut mgr, "BLOCKED-FP", "");
    assert!(!prompted, "a blocked fingerprint must not prompt the user");
    assert_eq!(
        verdict,
        Some(false),
        "a blocked fingerprint must be rejected automatically"
    );
}

/// An invite may spell its fingerprint in uppercase while a handshake presents
/// lowercase hex; the block must still match.
#[test]
fn blocking_a_contact_imported_with_an_uppercase_fingerprint_holds() {
    let mut mgr = ChatManager::default();
    let mallory = identity("Mallory");
    let link = crate::identity::invite::resign_for_test(
        &mallory.generate_signed_invite_link(None).unwrap(),
        &mallory.private_key().unwrap(),
        |p| p.fingerprint = p.fingerprint.to_ascii_uppercase(),
    )
    .unwrap();
    let contact_id = mgr
        .import_contact(mgr.parse_invite_link(&link).unwrap())
        .unwrap();
    mgr.block_contact(contact_id).unwrap();
    let (prompted, verdict) = incoming_tofu(&mut mgr, &mallory.fingerprint, "");
    assert!(!prompted);
    assert_eq!(verdict, Some(false), "the block must hold");
}

/// Typed fingerprints are stored canonically, as the handshake presents them.
#[test]
fn typed_fingerprints_are_stored_canonically() {
    let mut mgr = ChatManager::default();
    let id = mgr.add_contact("Bob".into(), None, Some("  ABCDEF0123  ".into()), None);
    assert_eq!(mgr.contacts[&id].fingerprint.as_deref(), Some("abcdef0123"));
}

#[test]
fn blocking_tears_down_the_live_session_completely() {
    let mut mgr = ChatManager::default();
    let contact_id = mgr.add_contact("Mallory".into(), None, Some("BLOCKED-FP".into()), None);
    let (session_id, chat_id) = incoming_chat(&mut mgr);
    mgr.get_chat_mut(chat_id).unwrap().peer_fingerprint = Some("BLOCKED-FP".into());
    let (app_tx, _app_rx) = mpsc::unbounded_channel();
    mgr.add_session_for_test(session_id, SessionHandle::for_test_control(app_tx));
    let (event_tx, event_rx) = mpsc::unbounded_channel::<SessionEvent>();
    mgr.session_events
        .insert(session_id, Arc::new(Mutex::new(event_rx)));
    let _confirm_rx = confirm_channel(&mut mgr, session_id);

    mgr.block_contact(contact_id).unwrap();

    // Otherwise the blocked peer could keep delivering on the live session.
    assert!(!mgr.sessions.contains_key(&session_id));
    assert!(!mgr.session_events.contains_key(&session_id));
    assert!(!mgr.fingerprint_confirm_senders.contains_key(&session_id));
    assert!(!mgr.chat_id_mapping.values().any(|v| *v == session_id));
    // The dropped receiver is what ends the network task's loop.
    assert!(event_tx.send(SessionEvent::Disconnected).is_err());
}

#[test]
fn tofu_accept_promotes_contact_to_verified() {
    let (mut mgr, session_id) = manager_with_chat("Alice");
    let contact_id = mgr.add_contact("Alice".into(), None, None, None);
    mgr.associate_contact_with_chat(contact_id, session_id);
    let _rx = confirm_channel(&mut mgr, session_id);

    mgr.handle_tofu_verification(session_id, "ALICE-FP", "Alice", "");
    assert!(mgr.pending_fingerprint().is_some());
    mgr.confirm_fingerprint(session_id, true).unwrap();

    let contact = mgr.get_contact(contact_id).unwrap();
    assert_eq!(contact.trust_state, TrustState::Verified);
    assert_eq!(contact.fingerprint.as_deref(), Some("ALICE-FP"));
}

#[test]
fn unblock_restores_verified_when_fingerprint_confirmed() {
    let (mut mgr, chat_id) = manager_with_chat("Bob");
    let contact_id = mgr.add_contact("Bob".into(), None, Some("BOB-FP".into()), None);
    mgr.get_chat_mut(chat_id).unwrap().peer_fingerprint = Some("BOB-FP".into());
    let trust = |mgr: &ChatManager| mgr.get_contact(contact_id).unwrap().trust_state;

    mgr.block_contact(contact_id).unwrap();
    assert_eq!(trust(&mgr), TrustState::Blocked);
    mgr.unblock_contact(contact_id).unwrap();
    assert_eq!(
        trust(&mgr),
        TrustState::Verified,
        "confirmed fingerprint restores Verified after unblock"
    );
}

#[tokio::test]
async fn connect_to_blocked_contact_is_refused() {
    let mut mgr = ChatManager::default();
    let contact_id = mgr.add_contact(
        "Mallory".into(),
        Some("127.0.0.1:1".into()),
        Some("BLOCKED-FP".into()),
        None,
    );
    mgr.block_contact(contact_id).unwrap();

    let privkey = messenger_core::core::generate_rsa_keypair(1024).unwrap();
    let err = mgr
        .connect_to_contact(contact_id, None, &privkey)
        .await
        .expect_err("connecting to a blocked contact must fail");
    assert!(err.to_string().contains("blocked"));
}

fn throwaway_key() -> rsa::PrivateKey {
    // Never used in a handshake here: nothing these tests dial answers.
    // (1024 bits: rsa 0.10 refuses smaller keys.)
    messenger_core::core::generate_rsa_keypair(1024).unwrap()
}

/// `connect_to_contact` must not dial when something already reaches the
/// peer: dialling again replaces a session that is working.
#[tokio::test]
async fn connecting_to_a_contact_reuses_a_live_session_instead_of_dialling() {
    let mut mgr = ChatManager::new(Config::default());
    let fp = "ab".repeat(32);
    let contact = mgr.add_contact(
        "Ann".into(),
        Some("127.0.0.1:1".into()),
        Some(fp.clone()),
        None,
    );

    // Its own conversation is live: that is the answer.
    let own = Uuid::new_v4();
    mgr.create_local_chat_for_test(own, "Ann".into());
    let _lane = wire_session_for_test(&mut mgr, own);
    mgr.associate_contact_with_chat(contact, own);
    assert_eq!(
        mgr.connect_to_contact(contact, None, &throwaway_key())
            .await
            .unwrap(),
        own
    );
    assert_eq!(mgr.sessions_len(), 1, "a live session was dialled over");

    // Another conversation with the same verified peer is live: re-associate.
    let mut mgr = ChatManager::new(Config::default());
    let contact = mgr.add_contact(
        "Ann".into(),
        Some("127.0.0.1:1".into()),
        Some(fp.clone()),
        None,
    );
    let other = Uuid::new_v4();
    mgr.create_local_chat_for_test(other, "Peer ab12".into());
    mgr.chats.get_mut(&other).unwrap().peer_fingerprint = Some(fp);
    let _lane = wire_session_for_test(&mut mgr, other);
    assert_eq!(
        mgr.connect_to_contact(contact, None, &throwaway_key())
            .await
            .unwrap(),
        other
    );
    assert_eq!(mgr.contact_to_chat.get(&contact), Some(&other));
    assert_eq!(mgr.sessions_len(), 1, "a live session was dialled over");
}

/// With nothing live, the dial lands in the conversation this contact
/// already has — found by fingerprint when no association was ever recorded —
/// so reconnecting never fragments the history; direct addresses are tried
/// first, and the relay only when there is no usable one.
#[tokio::test]
async fn connecting_to_a_contact_dials_into_its_existing_conversation() {
    let fp = "cd".repeat(32);
    let existing = |mgr: &mut ChatManager| {
        let id = Uuid::new_v4();
        mgr.create_local_chat_for_test(id, "Bob".into());
        mgr.chats.get_mut(&id).unwrap().peer_fingerprint = Some(fp.clone());
        id
    };

    // Direct.
    let mut mgr = ChatManager::new(Config::default());
    let history = existing(&mut mgr);
    let contact = mgr.add_contact(
        "Bob".into(),
        Some("127.0.0.1:1".into()),
        Some(fp.clone()),
        None,
    );
    let chat = mgr
        .connect_to_contact(contact, None, &throwaway_key())
        .await
        .unwrap();
    assert_eq!(chat, history, "a reconnect opened a second conversation");
    assert_eq!(mgr.chats[&chat].transport, Transport::Direct);
    assert_eq!(mgr.contact_to_chat.get(&contact), Some(&history));

    // Relay, because the only address does not parse.
    let mut mgr = ChatManager::new(Config::default());
    let history = existing(&mut mgr);
    let contact = mgr.add_contact(
        "Bob".into(),
        Some("not an address".into()),
        Some(fp.clone()),
        None,
    );
    {
        let c = mgr.contacts.get_mut(&contact).unwrap();
        c.relay_server = Some("127.0.0.1:1".into());
        c.relay_token = Some("token".into());
    }
    let chat = mgr
        .connect_to_contact(contact, None, &throwaway_key())
        .await
        .unwrap();
    assert_eq!(chat, history);
    assert_eq!(
        mgr.chats[&chat].transport,
        Transport::Relay,
        "an unparsable address must not block the relay fallback"
    );
}

/// A contact with no address, no relay and no live session cannot be reached;
/// the error says what to do rather than pointing at an editor that does not
/// exist. An unknown contact is an error too.
#[tokio::test]
async fn a_contact_with_no_route_is_an_error_that_says_what_to_do() {
    let mut mgr = ChatManager::new(Config::default());
    let contact = mgr.add_contact("Cy".into(), None, Some("ef".repeat(32)), None);
    let err = mgr
        .connect_to_contact(contact, None, &throwaway_key())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("fresh invite link"), "{err}");
    assert_eq!(mgr.sessions_len(), 0);
    assert!(mgr
        .connect_to_contact(Uuid::new_v4(), None, &throwaway_key())
        .await
        .is_err());
}

/// CRYPTO-04. Dial a contact imported from an invite for key K1 and have K2
/// answer. Without an expected fingerprint the new chat started with
/// `peer_fingerprint: None`, so whatever key answered was shown as an ordinary
/// first-use prompt — the out-of-band invite did nothing to stop a MITM.
#[tokio::test]
async fn dialing_an_invited_contact_warns_when_a_different_key_answers() {
    let invite_fp = "aa".repeat(32);
    let presented_fp = "bb".repeat(32);
    let mut mgr = ChatManager::new(Config::default());
    // Nothing listens on port 1: the session task fails in the background, but
    // the handshake's outcome is what is driven here.
    let contact_id = mgr.add_contact(
        "Alice".into(),
        Some("127.0.0.1:1".into()),
        Some(invite_fp.clone()),
        None,
    );
    let privkey = messenger_core::core::generate_rsa_keypair(1024).unwrap();
    let chat_id = mgr
        .connect_to_contact(contact_id, None, &privkey)
        .await
        .unwrap();

    mgr.handle_tofu_verification(chat_id, &presented_fp, "127.0.0.1:1", "123456 🎃🎈🎁");

    let prompt = mgr
        .pending_fingerprint()
        .expect("a key that is not the invite's must be put to the user");
    assert_eq!(prompt.fingerprint, presented_fp);
    assert_eq!(
        prompt.warning,
        Some(FingerprintWarning::InviteMismatch {
            contact_id,
            contact_name: "Alice".into(),
            expected: invite_fp.clone(),
        }),
        "the prompt must say the key differs from the invite, not ask a first-use question"
    );
    assert!(
        mgr.get_chat(chat_id).unwrap().peer_fingerprint.is_none(),
        "nothing is trusted before the user decides"
    );
    assert_eq!(
        mgr.get_contact(contact_id).unwrap().fingerprint.as_deref(),
        Some(invite_fp.as_str()),
        "the contact keeps the invite's key until the user accepts another"
    );
}

/// The other half of CRYPTO-04: the invite's key answering is *not*
/// verification. It gets the ordinary SAS prompt, with no warning, and is not
/// auto-accepted.
#[tokio::test]
async fn dialing_an_invited_contact_still_prompts_when_the_invite_key_answers() {
    let invite_fp = "aa".repeat(32);
    let mut mgr = ChatManager::new(Config::default());
    let contact_id = mgr.add_contact(
        "Alice".into(),
        Some("127.0.0.1:1".into()),
        Some(invite_fp.clone()),
        None,
    );
    let privkey = messenger_core::core::generate_rsa_keypair(1024).unwrap();
    let chat_id = mgr
        .connect_to_contact(contact_id, None, &privkey)
        .await
        .unwrap();

    mgr.handle_tofu_verification(chat_id, &invite_fp, "127.0.0.1:1", "123456 🎃🎈🎁");

    let prompt = mgr
        .pending_fingerprint()
        .expect("an invite is not verification: the SAS must still be compared");
    assert_eq!(prompt.session_id, chat_id);
    assert_eq!(
        prompt.warning, None,
        "a matching key is an ordinary first use"
    );
    assert!(mgr.get_chat(chat_id).unwrap().peer_fingerprint.is_none());
    assert_eq!(
        mgr.get_contact(contact_id).unwrap().trust_state,
        TrustState::Unverified
    );
}

/// Auto-trust on first use is a convenience for peers nobody vouched for. It
/// must not wave through a key that contradicts the invite the user imported.
#[tokio::test]
async fn auto_trust_does_not_accept_a_key_that_contradicts_the_invite() {
    let invite_fp = "aa".repeat(32);
    let presented_fp = "bb".repeat(32);
    let mut mgr = ChatManager::new(Config {
        auto_trust_on_first_use: true,
        ..Config::default()
    });
    // A chat elsewhere already trusts K2, which would also make it "known".
    let elsewhere = Uuid::new_v4();
    mgr.create_local_chat_for_test(elsewhere, "Someone".into());
    mgr.get_chat_mut(elsewhere).unwrap().peer_fingerprint = Some(presented_fp.clone());
    let contact_id = mgr.add_contact(
        "Alice".into(),
        Some("127.0.0.1:1".into()),
        Some(invite_fp.clone()),
        None,
    );
    let privkey = messenger_core::core::generate_rsa_keypair(1024).unwrap();
    let chat_id = mgr
        .connect_to_contact(contact_id, None, &privkey)
        .await
        .unwrap();

    mgr.handle_tofu_verification(chat_id, &presented_fp, "127.0.0.1:1", "123456 🎃🎈🎁");

    assert!(matches!(
        mgr.pending_fingerprint().map(|p| &p.warning),
        Some(Some(FingerprintWarning::InviteMismatch { .. }))
    ));
    assert!(mgr.get_chat(chat_id).unwrap().peer_fingerprint.is_none());
}

/// Accepting a key after being told it is not the invite's is the user's
/// decision, made by comparing the SAS. The contact must follow it: left on
/// the invite's key, a later block would block a key that never connects,
/// and every reconnect would warn about the key the user already accepted.
#[tokio::test]
async fn accepting_a_key_over_the_invite_warning_rebinds_the_contact() {
    let invite_fp = "aa".repeat(32);
    let presented_fp = "bb".repeat(32);
    let mut mgr = ChatManager::new(Config::default());
    let contact_id = mgr.add_contact(
        "Alice".into(),
        Some("127.0.0.1:1".into()),
        Some(invite_fp.clone()),
        None,
    );
    let privkey = messenger_core::core::generate_rsa_keypair(1024).unwrap();
    let chat_id = mgr
        .connect_to_contact(contact_id, None, &privkey)
        .await
        .unwrap();
    mgr.handle_tofu_verification(chat_id, &presented_fp, "127.0.0.1:1", "123456 🎃🎈🎁");

    mgr.confirm_fingerprint(chat_id, true).unwrap();

    assert_eq!(
        mgr.get_chat(chat_id).unwrap().peer_fingerprint.as_deref(),
        Some(presented_fp.as_str())
    );
    let contact = mgr.get_contact(contact_id).unwrap();
    assert_eq!(contact.fingerprint.as_deref(), Some(presented_fp.as_str()));
    assert_eq!(contact.trust_state, TrustState::Verified);
}

/// The conversation's key changed *and* the invite named a third key: the
/// prompt leads with the changed key, but accepting it must still move the
/// contact to the accepted key. Otherwise the contact stays pinned to the
/// invite's key and every later dial warns about the key just accepted.
#[tokio::test]
async fn accepting_a_changed_key_that_also_contradicts_the_invite_rebinds_the_contact() {
    let invite_fp = "aa".repeat(32);
    let previous_fp = "cc".repeat(32);
    let presented_fp = "bb".repeat(32);
    let mut mgr = ChatManager::new(Config::default());
    let contact_id = mgr.add_contact(
        "Alice".into(),
        Some("127.0.0.1:1".into()),
        Some(invite_fp.clone()),
        None,
    );
    let privkey = messenger_core::core::generate_rsa_keypair(1024).unwrap();
    let chat_id = mgr
        .connect_to_contact(contact_id, None, &privkey)
        .await
        .unwrap();
    mgr.get_chat_mut(chat_id).unwrap().peer_fingerprint = Some(previous_fp.clone());
    mgr.handle_tofu_verification(chat_id, &presented_fp, "127.0.0.1:1", "123456 🎃🎈🎁");
    assert_eq!(
        mgr.pending_fingerprint().unwrap().warning,
        Some(FingerprintWarning::KeyChanged {
            previous: previous_fp.clone()
        })
    );

    mgr.confirm_fingerprint(chat_id, true).unwrap();

    assert_eq!(
        mgr.get_chat(chat_id).unwrap().peer_fingerprint.as_deref(),
        Some(presented_fp.as_str())
    );
    let contact = mgr.get_contact(contact_id).unwrap();
    assert_eq!(contact.fingerprint.as_deref(), Some(presented_fp.as_str()));
    assert_eq!(contact.trust_state, TrustState::Verified);
}

/// A changed key on a conversation that was dialled with the invite's own key
/// leaves the contact alone: the invite was right, only the chat's pin moved.
#[tokio::test]
async fn accepting_a_changed_key_that_matches_the_invite_keeps_the_contact() {
    let invite_fp = "aa".repeat(32);
    let mut mgr = ChatManager::new(Config::default());
    let contact_id = mgr.add_contact(
        "Alice".into(),
        Some("127.0.0.1:1".into()),
        Some(invite_fp.clone()),
        None,
    );
    let privkey = messenger_core::core::generate_rsa_keypair(1024).unwrap();
    let chat_id = mgr
        .connect_to_contact(contact_id, None, &privkey)
        .await
        .unwrap();
    mgr.get_chat_mut(chat_id).unwrap().peer_fingerprint = Some("cc".repeat(32));
    mgr.handle_tofu_verification(chat_id, &invite_fp, "127.0.0.1:1", "123456 🎃🎈🎁");

    mgr.confirm_fingerprint(chat_id, true).unwrap();

    let contact = mgr.get_contact(contact_id).unwrap();
    assert_eq!(contact.fingerprint.as_deref(), Some(invite_fp.as_str()));
}

/// A prompt for a conversation whose key changed says so in the prompt itself,
/// not only in a toast that disappears after four seconds.
#[test]
fn a_changed_key_prompt_carries_the_previous_fingerprint() {
    let mut mgr = ChatManager::new(Config::default());
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Bob".into());
    mgr.get_chat_mut(chat_id).unwrap().peer_fingerprint = Some("OLD-FP".into());
    let (tx, mut rx) = mpsc::unbounded_channel();
    mgr.add_fingerprint_confirm_sender_for_test(chat_id, tx);

    mgr.handle_tofu_verification(chat_id, "NEW-FP", "Bob", "");

    assert_eq!(
        mgr.pending_fingerprint().unwrap().warning,
        Some(FingerprintWarning::KeyChanged {
            previous: "OLD-FP".into()
        })
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn placeholder_detection_works() {
    let mut mgr = ChatManager::default();
    assert!(!mgr.chats.values().any(|c| c.is_host_placeholder));
    let chat = Chat {
        is_host_placeholder: true,
        ..Chat::new(
            Uuid::new_v4(),
            "Host on :5001",
            ChatKind::Dm,
            Transport::Direct,
        )
    };
    mgr.chats.insert(chat.id, chat);
    assert!(mgr.chats.values().any(|c| c.is_host_placeholder));
}

#[test]
fn test_tofu_logic() {
    let (mut mgr, chat_id) = manager_with_chat("Test Chat");
    let mut rx = confirm_channel(&mut mgr, chat_id);

    // 1. First use: prompt, nothing stored, no auto-confirm.
    show_fingerprint(&mut mgr, chat_id, "fingerprint1", "peer");
    assert_eq!(mgr.chats[&chat_id].peer_fingerprint, None);
    assert!(mgr.pending_fingerprint().is_some());
    assert!(rx.try_recv().is_err());

    // The user accepts: the session is confirmed and the fingerprint stored.
    mgr.confirm_fingerprint(chat_id, true)
        .expect("confirm should succeed");
    assert_eq!(rx.try_recv(), Ok(true));
    assert_eq!(
        mgr.chats[&chat_id].peer_fingerprint.as_deref(),
        Some("fingerprint1")
    );

    // 2. Matching fingerprint: auto-confirm, no prompt.
    show_fingerprint(&mut mgr, chat_id, "fingerprint1", "peer");
    assert!(mgr.pending_fingerprint().is_none());
    assert_eq!(rx.try_recv(), Ok(true));

    // 3. Mismatched fingerprint: prompt, no auto-confirm.
    show_fingerprint(&mut mgr, chat_id, "fingerprint2", "peer");
    assert_eq!(
        mgr.pending_fingerprint().unwrap().fingerprint,
        "fingerprint2"
    );
    assert!(rx.try_recv().is_err());
}

/// Security defaults: auto-trust-on-first-use (silent first-contact MITM) and
/// mDNS (LAN fingerprint disclosure) are both off, and first use prompts.
#[test]
fn test_regression_security_defaults_off() {
    let config = Config::default();
    assert!(
        !config.auto_trust_on_first_use,
        "auto_trust_on_first_use MUST default to false for security"
    );
    assert!(
        !config.enable_mdns,
        "enable_mdns MUST default to false for privacy (LAN fingerprint disclosure risk)"
    );

    let (mut mgr, chat_id) = manager_with_chat("Test Chat");
    let _rx = confirm_channel(&mut mgr, chat_id);
    show_fingerprint(&mut mgr, chat_id, "first_fingerprint", "Alice");
    assert_eq!(mgr.chats[&chat_id].peer_fingerprint, None);
    assert!(
        mgr.pending_fingerprint().is_some(),
        "Should show fingerprint verification dialog when auto_trust_on_first_use=false"
    );
}

// ── Signed invite links ─────────────────────────────────────────────────────

#[test]
fn parse_v2_signed_invite_link_valid() {
    let identity = identity("Test Signer");
    let link = identity
        .generate_signed_invite_link(Some("127.0.0.1:9001".to_string()))
        .unwrap();

    let contact = ChatManager::default()
        .parse_invite_link(&link)
        .expect("should parse v2 signed invite");

    assert_eq!(contact.name, "Test Signer");
    assert_eq!(contact.address.as_deref(), Some("127.0.0.1:9001"));
    assert_eq!(contact.fingerprint, Some(identity.fingerprint.clone()));
    assert_eq!(contact.public_key, Some(identity.public_key_pem.clone()));
    assert_eq!(contact.trust_state, TrustState::Unverified);
    // Single-address invite: no extra candidate list.
    assert!(contact.addresses.is_empty());
    assert_eq!(contact.candidate_addresses(), vec!["127.0.0.1:9001"]);
}

#[test]
fn parse_v2_signed_invite_without_address() {
    let identity = identity("No Address User");
    let link = identity.generate_signed_invite_link(None).unwrap();
    let contact = ChatManager::default()
        .parse_invite_link(&link)
        .expect("should parse v2 signed invite without address");
    assert_eq!(contact.name, "No Address User");
    assert_eq!(contact.address, None);
    assert_eq!(contact.fingerprint, Some(identity.fingerprint.clone()));
}

#[test]
fn parse_multi_address_invite_populates_ordered_candidates() {
    let identity = identity("Multi Homed");
    // External-first ordering, with a duplicate and an unparsable entry that
    // must be sanitized out without breaking signature verification.
    let link = identity
        .generate_signed_invite_link_with_addresses(
            vec![
                "203.0.113.7:12345".to_string(),
                "not an address".to_string(),
                "192.168.1.20:12345".to_string(),
                "203.0.113.7:12345".to_string(),
            ],
            None,
            None,
        )
        .unwrap();

    let contact = ChatManager::default()
        .parse_invite_link(&link)
        .expect("multi-address invite must verify and parse");

    assert_eq!(contact.name, "Multi Homed");
    assert_eq!(contact.address.as_deref(), Some("203.0.113.7:12345"));
    assert_eq!(
        contact.addresses,
        vec!["203.0.113.7:12345", "192.168.1.20:12345"]
    );
    assert_eq!(contact.candidate_addresses(), contact.addresses);
}

#[test]
fn contact_candidate_addresses_falls_back_to_legacy_single() {
    // Contacts from before multi-address invites have only `address`.
    let contact = Contact {
        address: Some("10.0.0.9:6000".to_string()),
        ..sample_contact_for_test("Legacy")
    };
    assert_eq!(contact.candidate_addresses(), vec!["10.0.0.9:6000"]);

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
    assert_eq!(loaded.candidate_addresses(), vec!["10.0.0.9:6000"]);
}

#[test]
fn parse_v2_signed_invite_rejects_tampering() {
    let mgr = ChatManager::default();
    let link = identity("Tamper Test")
        .generate_signed_invite_link(None)
        .unwrap();
    let flipped = tamper_v2(&link, |invite| {
        if !invite.signature.is_empty() {
            invite.signature[0] ^= 0xFF;
        }
    });
    assert!(
        mgr.parse_invite_link(&flipped).is_err(),
        "should reject tampered signature"
    );

    // Swapping in another identity's fingerprint breaks the signature too.
    let other = identity("User Two").fingerprint;
    let swapped = tamper_v2(&link, |invite| {
        invite.payload["fingerprint"] = serde_json::Value::String(other);
    });
    assert!(
        mgr.parse_invite_link(&swapped).is_err(),
        "should reject invite with swapped fingerprint"
    );
}

#[test]
fn parse_signed_invite_enforces_the_timestamp_window() {
    let mgr = ChatManager::default();
    let identity = identity("Expiry Test");

    let expired = now_secs() - crate::INVITE_MAX_AGE_SECS - 60;
    let err = mgr
        .parse_invite_link(&resign_invite_with_timestamp(&identity, expired))
        .expect_err("expired invite must be rejected");
    assert!(
        err.to_string().contains("expired"),
        "error should mention expiry, got: {err}"
    );

    let future = now_secs() + crate::INVITE_TIMESTAMP_SKEW_SECS + 60;
    let err = mgr
        .parse_invite_link(&resign_invite_with_timestamp(&identity, future))
        .expect_err("future-dated invite must be rejected");
    assert!(
        err.to_string().contains("future"),
        "error should mention future timestamp, got: {err}"
    );

    // A week old: well inside the 30-day window.
    let recent = resign_invite_with_timestamp(&identity, now_secs() - 7 * 86_400);
    mgr.parse_invite_link(&recent)
        .expect("invite inside the expiry window must still parse");
}

#[test]
fn parse_v1_invite_link_still_works_with_warning() {
    let link = v1_invite_link("Legacy User", "192.168.1.50:8001");
    let contact = ChatManager::default()
        .parse_invite_link(&link)
        .expect("should parse v1 invite");
    assert_eq!(contact.name, "Legacy User");
    assert_eq!(contact.address.as_deref(), Some("192.168.1.50:8001"));
    assert_eq!(contact.fingerprint, Some(test_pubkey_fingerprint()));
    // …and it is *not* signed, which is what the UI warns about.
    assert!(!ChatManager::invite_link_is_signed(&link));
}

/// A signature only proves possession of the key *inside* the invite;
/// `fingerprint` is what trust is decided on, so the two must agree.
#[test]
fn an_invite_whose_fingerprint_does_not_match_its_key_is_rejected() {
    let link = v1_link(serde_json::json!({
        "name": "Impostor",
        "address": "127.0.0.1:12345",
        "fingerprint": "c".repeat(64),
        "public_key": TEST_PUBKEY_PEM,
    }));
    let err = ChatManager::default()
        .parse_invite_link(&link)
        .expect_err("a fingerprint that does not belong to the key must be refused");
    assert!(
        err.to_string().contains("inconsistent"),
        "the error should say what is wrong: {err}"
    );
}

#[test]
fn signed_invite_addresses_are_normalized_or_dropped() {
    let cases = [
        ("[2001:db8::1]:12345", Some("[2001:db8::1]:12345")),
        // Unbracketed IPv6 with a port is dropped, not guessed at.
        ("2001:db8::1:12345", None),
    ];
    for (address, expected) in cases {
        let link = identity("IPv6 User")
            .generate_signed_invite_link(Some(address.to_string()))
            .unwrap();
        let contact = ChatManager::default()
            .parse_invite_link(&link)
            .expect("invite itself should remain valid");
        assert_eq!(contact.address.as_deref(), expected, "{address}");
    }
}

#[test]
fn parse_v3_signed_invite_keeps_relay_route() {
    let relay_token = "0123456789abcdef0123456789abcdef";
    let link = identity("Relay Invite User")
        .generate_signed_invite_link_with_route(
            None,
            Some("relay.example.com:23456".to_string()),
            Some(relay_token.to_string()),
        )
        .unwrap();

    let contact = ChatManager::default()
        .parse_invite_link(&link)
        .expect("should parse relay invite");
    assert_eq!(
        contact.relay_server.as_deref(),
        Some("relay.example.com:23456")
    );
    assert_eq!(contact.relay_token.as_deref(), Some(relay_token));
    assert_eq!(contact.address, None);
}

// ── Contacts ────────────────────────────────────────────────────────────────

/// An imported invite is not verification: the peer is still prompted until
/// the contact is `Verified`, after which it is a returning peer.
#[test]
fn importing_an_invite_does_not_bypass_fingerprint_verification() {
    let (mut mgr, chat_id) = manager_with_chat("Peer");
    let fingerprint = "ab".repeat(32);
    mgr.import_contact(contact_with("Mum", &fingerprint, TrustState::Unverified))
        .unwrap();

    let _rx = confirm_channel(&mut mgr, chat_id);
    mgr.handle_tofu_verification(chat_id, &fingerprint, "Mum", "123456 🎃🎈🎁");
    assert!(
        mgr.pending_fingerprint().is_some(),
        "an unverified imported contact must still be verified on first contact"
    );
    assert!(
        mgr.get_chat(chat_id).unwrap().peer_fingerprint.is_none(),
        "trust must not be stored before the user accepts"
    );

    let second = Uuid::new_v4();
    mgr.create_local_chat_for_test(second, "Mum (2)".into());
    if let Some(c) = mgr.contacts.values_mut().next() {
        c.trust_state = TrustState::Verified;
    }
    let _rx2 = confirm_channel(&mut mgr, second);
    mgr.handle_tofu_verification(second, &fingerprint, "Mum", "123456 🎃🎈🎁");
    assert_eq!(
        mgr.get_chat(second).unwrap().peer_fingerprint.as_deref(),
        Some(fingerprint.as_str()),
        "a verified contact is a returning peer and is auto-accepted"
    );
}

/// Deleting a contact revokes its stored trust (and lifts its block).
#[test]
fn deleting_a_contact_also_revokes_its_stored_trust() {
    let (mut mgr, chat_id) = manager_with_chat("Peer");
    let fingerprint = "cd".repeat(32);
    mgr.chats.get_mut(&chat_id).unwrap().peer_fingerprint = Some(fingerprint.clone());
    let id = mgr
        .import_contact(contact_with(
            "Blocked Peer",
            &fingerprint,
            TrustState::Blocked,
        ))
        .unwrap();

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

#[test]
fn importing_an_invite_is_idempotent_and_refuses_your_own() {
    let mut mgr = ChatManager::default();
    let fingerprint = "ef".repeat(32);

    let a = mgr
        .import_contact(contact_with("Sam", &fingerprint, TrustState::Unverified))
        .unwrap();
    let again = Contact {
        address: Some("10.0.0.9:12345".to_string()),
        ..contact_with("Sam (new address)", &fingerprint, TrustState::Unverified)
    };
    let b = mgr.import_contact(again).unwrap();

    assert_eq!(a, b, "the same peer must not become two contacts");
    assert_eq!(mgr.contacts.len(), 1);
    assert_eq!(
        mgr.get_contact(a).unwrap().address.as_deref(),
        Some("10.0.0.9:12345"),
        "a re-import refreshes how to reach them"
    );

    mgr.set_my_fingerprint(fingerprint.clone());
    assert!(
        mgr.import_contact(contact_with("Me", &fingerprint, TrustState::Unverified))
            .is_err(),
        "your own invite is not a contact"
    );
}

// ── Outgoing files ──────────────────────────────────────────────────────────

/// Drain frames until `FileEnd`/`FileCancel` (or timeout).
async fn drain_file_send(rx: &mut mpsc::Receiver<ProtocolMessage>) -> Vec<ProtocolMessage> {
    let mut frames = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
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

/// A chat with a session whose control and file lanes are both observable.
struct FileSendFixture {
    mgr: ChatManager,
    chat_id: Uuid,
    control_rx: mpsc::UnboundedReceiver<ProtocolMessage>,
    file_rx: mpsc::Receiver<ProtocolMessage>,
}

fn file_send_fixture() -> FileSendFixture {
    let (mut mgr, chat_id) = manager_with_chat("Peer");
    let (from_app_tx, control_rx) = mpsc::unbounded_channel();
    let (file_tx, file_rx) = mpsc::channel(FILE_LANE_CAPACITY);
    mgr.sessions.insert(
        chat_id,
        SessionHandle {
            from_app_tx,
            file_tx,
        },
    );
    FileSendFixture {
        mgr,
        chat_id,
        control_rx,
        file_rx,
    }
}

/// Start sending a temp file of `content`; returns the file (keep it alive)
/// and the outgoing transfer id.
async fn start_send(
    mgr: &mut ChatManager,
    chat_id: Uuid,
    content: &[u8],
) -> (tempfile::NamedTempFile, Uuid) {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), content).unwrap();
    mgr.send_file(chat_id, file.path().to_path_buf())
        .await
        .expect("send_file should succeed");
    let outgoing = mgr
        .active_transfers_snapshot()
        .into_iter()
        .find(|t| t.direction == TransferDirection::Outgoing)
        .expect("outgoing transfer should be tracked");
    assert_eq!(outgoing.chat_id, chat_id);
    (file, outgoing.id)
}

#[tokio::test]
async fn send_file_streams_meta_chunks_and_end_and_tracks_transfer() {
    let mut f = file_send_fixture();
    let content = vec![b'x'; crate::FILE_CHUNK_SIZE * 2 + 13];
    let _file = start_send(&mut f.mgr, f.chat_id, &content).await;

    // Meta/chunks/End ride the bounded file lane, in order.
    let frames = drain_file_send(&mut f.file_rx).await;
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
    let mut f = file_send_fixture();
    // Large enough that streaming does not finish instantly.
    let content = vec![b'y'; crate::FILE_CHUNK_SIZE * 50];
    let (_file, transfer_id) = start_send(&mut f.mgr, f.chat_id, &content).await;

    // Keep draining so the backpressured task can observe the cancel flag.
    let mut file_rx = f.file_rx;
    let drain = tokio::spawn(async move {
        while let Some(msg) = file_rx.recv().await {
            assert!(
                !matches!(msg, ProtocolMessage::FileEnd { .. }),
                "stream should not complete after cancellation"
            );
        }
    });

    f.mgr.cancel_transfer(transfer_id);

    let mut saw_cancel = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while let Ok(Some(msg)) = tokio::time::timeout_at(deadline, f.control_rx.recv()).await {
        if matches!(msg, ProtocolMessage::FileCancel { .. }) {
            saw_cancel = true;
            break;
        }
    }
    drain.abort();
    assert!(saw_cancel, "cancellation must emit a FileCancel frame");
    assert_eq!(
        transfer_status(&f.mgr, transfer_id),
        Some(TransferStatus::Cancelled)
    );
}

/// With the consumer stalled, the stream task runs only ~lane capacity ahead.
#[tokio::test]
async fn outgoing_stream_is_backpressured_by_the_bounded_lane() {
    let mut f = file_send_fixture();
    let content = vec![b'z'; crate::FILE_CHUNK_SIZE * 100];
    let file_size = content.len() as u64;
    let (_file, transfer_id) = start_send(&mut f.mgr, f.chat_id, &content).await;

    let sent = |mgr: &mut ChatManager| {
        mgr.sync_outgoing_transfer_progress();
        mgr.active_transfers_snapshot()
            .into_iter()
            .find(|t| t.id == transfer_id)
            .map(|t| t.received)
            .unwrap_or(0)
    };
    // Wait for the one state in which the reader is provably blocked: the
    // lane full and nothing draining it. A fixed sleep instead passed on a
    // slow runner where the reader had not run at all — "stalled at 0 bytes"
    // satisfies every bound below without anything having been backpressured.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while f.file_rx.len() < FILE_LANE_CAPACITY {
        assert!(
            std::time::Instant::now() < deadline,
            "the lane never filled"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // Let a blocked reader prove it stays blocked.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let stalled = sent(&mut f.mgr);
    let cap_bytes = FILE_LANE_CAPACITY as u64 * crate::FILE_CHUNK_SIZE as u64;
    assert!(stalled > 0, "the reader made no progress at all");
    assert!(
        stalled <= cap_bytes,
        "in-flight bytes must be capped at the lane capacity; got {stalled} > {cap_bytes}"
    );
    assert_eq!(
        f.file_rx.len(),
        FILE_LANE_CAPACITY,
        "nothing may queue past a full lane"
    );
    assert!(stalled < file_size);

    // Draining the lane lets the reader advance.
    for _ in 0..20 {
        let _ = f.file_rx.recv().await;
    }
    while sent(&mut f.mgr) <= stalled {
        assert!(
            std::time::Instant::now() < deadline,
            "draining the lane did not let the reader advance"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn has_toast(mgr: &ChatManager, needle: &str) -> bool {
    mgr.toasts.iter().any(|t| t.message.contains(needle))
}

/// Queueing is not delivery: "File sent" waits for the wire confirmation.
#[tokio::test]
async fn file_sent_toast_waits_for_wire_confirmation() {
    let mut f = file_send_fixture();
    let _file = start_send(&mut f.mgr, f.chat_id, b"payload").await;
    assert!(
        !has_toast(&f.mgr, "File sent"),
        "success toast must not fire before the wire confirmation"
    );
    f.mgr
        .handle_session_event(f.chat_id, SessionEvent::FileSendComplete { seq: 42 });
    assert!(
        has_toast(&f.mgr, "File sent"),
        "success toast must fire once the final frame is on the wire"
    );
}

/// A session dying before the final frame warns instead of claiming success.
#[tokio::test]
async fn disconnect_fails_pending_file_sends_honestly() {
    let mut f = file_send_fixture();
    let _file = start_send(&mut f.mgr, f.chat_id, b"payload").await;
    f.mgr
        .handle_session_event(f.chat_id, SessionEvent::Disconnected);
    assert!(
        !has_toast(&f.mgr, "File sent"),
        "no success toast after a disconnect"
    );
    assert!(
        has_toast(&f.mgr, "may not have been delivered"),
        "the user must see an honest delivery warning"
    );
    assert!(
        !f.mgr.pending_file_sends.contains_key(&f.chat_id),
        "pending sends must be cleared on disconnect"
    );
}

/// `FileChunk` carries no transfer id, so a second concurrent send on one
/// conversation must be refused rather than interleaved.
#[tokio::test]
async fn a_second_concurrent_file_send_is_refused() {
    let mut f = file_send_fixture();
    let _first = start_send(&mut f.mgr, f.chat_id, &[1u8; 4096]).await;
    let second = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(second.path(), [2u8; 4096]).unwrap();
    let err = f
        .mgr
        .send_file(f.chat_id, second.path().to_path_buf())
        .await
        .expect_err("a second concurrent send must be refused, not interleaved");
    assert!(
        err.to_string().contains("Still sending"),
        "the error must name the conflict, got: {}",
        err
    );
}

// ── Incoming text ───────────────────────────────────────────────────────────

#[test]
fn large_incoming_message_reassembles_into_one_chat_message() {
    let (mut mgr, chat_id) = manager_with_chat("Chunked Incoming");
    let text = "hello large world ".repeat(8_000);
    let mut seq = 0u64;
    let messages = ChatManager::build_text_protocol_messages(&mut seq, &text, 123_456_789)
        .expect("large text should chunk successfully");
    assert!(messages.len() > 1, "test message should be chunked");
    for msg in messages {
        frame(&mut mgr, chat_id, msg);
    }
    assert_eq!(texts(&mgr, chat_id), [text]);
    assert_eq!(mgr.chats[&chat_id].messages.len(), 1);
}

#[test]
fn stale_incoming_large_message_is_discarded() {
    let mut mgr = ChatManager::default();
    let key = (Uuid::new_v4(), Uuid::new_v4());
    mgr.incoming_text_messages.insert(
        key,
        IncomingTextMessage {
            timestamp_millis: 1,
            parts: vec![Some("partial".to_string()), None],
            updated_at: std::time::Instant::now() - Duration::from_secs(121),
            started_at: std::time::Instant::now() - Duration::from_secs(121),
            bytes: 7,
        },
    );

    mgr.cleanup_stale_incoming_text_messages();

    assert!(!mgr.incoming_text_messages.contains_key(&key));
    assert!(
        has_toast(&mgr, "large incoming message"),
        "cleanup should surface a warning toast"
    );
}

/// APP-09: the 16 partial-message slots per chat were the only bound, and
/// each could hold 511 of 512 parts — about 384 MiB from one verified peer.
/// The buffered bytes are budgeted: the chunk that would cross the budget is
/// refused and its message dropped, so the total never passes it.
#[test]
fn partial_large_messages_are_held_to_a_byte_budget() {
    let mut mgr = ChatManager::new(Config::default());
    let chat = Uuid::new_v4();
    let part = "x".repeat(crate::TEXT_CHUNK_BYTES);
    let buffered =
        |mgr: &ChatManager| -> usize { mgr.incoming_text_messages.values().map(|m| m.bytes).sum() };
    let mut refused = 0;
    // Spread over the 16 slots, as a peer would, never completing any.
    'outer: for message in 0..crate::MAX_CONCURRENT_PARTIAL_TEXT_PER_CHAT {
        let id = Uuid::from_u128(message as u128 + 1);
        for index in 0..(crate::MAX_TEXT_CHUNKS - 1) {
            match mgr.register_incoming_text_chunk(
                chat,
                id,
                index,
                crate::MAX_TEXT_CHUNKS,
                part.clone(),
                1,
            ) {
                Ok(_) => {}
                Err(_) => {
                    refused += 1;
                    assert!(
                        !mgr.incoming_text_messages.contains_key(&(chat, id)),
                        "the message that crossed the budget is dropped, not kept half-open"
                    );
                    if refused > 3 {
                        break 'outer;
                    }
                    continue 'outer;
                }
            }
            assert!(buffered(&mgr) <= super::text::MAX_PARTIAL_TEXT_BYTES_PER_CHAT);
        }
    }
    assert!(refused > 0, "nothing was ever refused");

    // Another conversation still has its own room — up to the global budget.
    assert!(mgr
        .register_incoming_text_chunk(Uuid::new_v4(), Uuid::new_v4(), 0, 2, part.clone(), 1)
        .is_ok());
}

/// The budget must not refuse what it exists to allow: one message of the
/// largest size the protocol permits still arrives whole.
#[test]
fn a_maximum_size_message_still_fits_the_budget() {
    assert!(
        crate::MAX_TEXT_CHUNKS as usize * crate::TEXT_CHUNK_BYTES
            <= super::text::MAX_PARTIAL_TEXT_BYTES_PER_CHAT
    );
    let mut mgr = ChatManager::new(Config::default());
    let (chat, id) = (Uuid::new_v4(), Uuid::new_v4());
    let part = "y".repeat(crate::TEXT_CHUNK_BYTES);
    let mut done = None;
    for index in 0..crate::MAX_TEXT_CHUNKS {
        done = mgr
            .register_incoming_text_chunk(chat, id, index, crate::MAX_TEXT_CHUNKS, part.clone(), 1)
            .unwrap();
    }
    let (text, _) = done.expect("reassembled");
    assert_eq!(
        text.len(),
        crate::MAX_TEXT_CHUNKS as usize * crate::TEXT_CHUNK_BYTES
    );
    assert!(mgr.incoming_text_messages.is_empty());
}

/// APP-09: the timeout restarted with every chunk, so one chunk a minute kept
/// a buffer forever. A message has `super::text::INCOMING_TEXT_MAX_AGE` to arrive in all.
#[test]
fn a_trickled_large_message_cannot_stay_open_forever() {
    let mut mgr = ChatManager::new(Config::default());
    let (chat, id) = (Uuid::new_v4(), Uuid::new_v4());
    mgr.register_incoming_text_chunk(chat, id, 0, 4, "a".into(), 1)
        .unwrap();
    // Recently active, but started long ago.
    let entry = mgr.incoming_text_messages.get_mut(&(chat, id)).unwrap();
    entry.started_at =
        std::time::Instant::now() - super::text::INCOMING_TEXT_MAX_AGE - Duration::from_secs(1);
    entry.updated_at = std::time::Instant::now();
    mgr.cleanup_stale_incoming_text_messages();
    assert!(mgr.incoming_text_messages.is_empty());
}

#[test]
fn mapped_session_ping_updates_actual_chat() {
    let mut mgr = ChatManager::default();
    let (session_id, incoming) = incoming_chat(&mut mgr);
    mgr.chats.get_mut(&incoming).unwrap().recv_seq = 0;
    frame(&mut mgr, session_id, ProtocolMessage::Ping { seq: 1 });
    assert_eq!(
        mgr.chats[&incoming].recv_seq, 1,
        "Ping sequence must be applied to mapped actual chat"
    );
}

/// PERF-07: `finalize` — `sync_all` of up to 10 GiB, then the rename — ran
/// inside `handle_session_event`, i.e. under the manager lock, so every
/// command and the poll loop waited for the disk. It runs on a blocking
/// thread now and completes on a later poll. Until then the file is not in
/// the chat, and a disconnect in that window does not fail it: the last byte
/// has arrived, only the write to disk is left.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn finalizing_a_received_file_happens_off_the_manager() {
    let temp_dir = tempdir().unwrap();
    let config = Config {
        download_dir: temp_dir.path().join("downloads"),
        temp_dir: temp_dir.path().join("temp"),
        auto_accept_files: true,
        ..Config::default()
    };
    let mut mgr = ChatManager::new(config);
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Big file".to_string());
    let payload = b"the whole file";
    mgr.handle_session_event(
        chat_id,
        SessionEvent::MessageReceived(ProtocolMessage::FileMeta {
            filename: "big.bin".to_string(),
            size: payload.len() as u64,
            seq: 1,
        }),
    );
    mgr.handle_session_event(
        chat_id,
        SessionEvent::MessageReceived(ProtocolMessage::FileChunk {
            chunk: payload.to_vec(),
            seq: 2,
        }),
    );

    // Hold the disk write until the test says so.
    let (release, gate) = std::sync::mpsc::channel::<()>();
    *files::FINALIZE_GATE.lock().unwrap() = Some(gate);
    mgr.handle_session_event(
        chat_id,
        SessionEvent::MessageReceived(ProtocolMessage::FileEnd { seq: 3 }),
    );
    let in_chat = |mgr: &ChatManager| {
        mgr.chats[&chat_id]
            .messages
            .iter()
            .any(|m| matches!(&m.content, MessageContent::File { path: Some(_), .. }))
    };
    assert!(
        !in_chat(&mgr),
        "the file was finalized inline, under the caller"
    );

    // The sender hangs up while the write is still in progress.
    mgr.handle_session_event(chat_id, SessionEvent::Disconnected);
    release.send(()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !in_chat(&mgr) {
        assert!(
            std::time::Instant::now() < deadline,
            "the finalized file never landed"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        mgr.poll_session_events();
    }
    let path = mgr.chats[&chat_id]
        .messages
        .iter()
        .find_map(|m| match &m.content {
            MessageContent::File { path: Some(p), .. } => Some(p.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(std::fs::read(path).unwrap(), payload);
    assert!(mgr.active_transfers.is_empty());
}

#[test]
fn register_incoming_text_chunk_rejects_oversized_total_chunks() {
    // Defense in depth beyond the wire decoder: no giant reassembly buffer.
    let mut mgr = ChatManager::default();
    let err = mgr
        .register_incoming_text_chunk(
            Uuid::new_v4(),
            Uuid::new_v4(),
            0,
            u32::MAX,
            "x".to_string(),
            0,
        )
        .expect_err("oversized total_chunks must be rejected");
    assert!(err.to_string().contains("Invalid chunk count"));
    assert!(mgr.incoming_text_messages.is_empty());
}

#[test]
fn register_incoming_text_chunk_caps_concurrent_partial_messages() {
    let mut mgr = ChatManager::default();
    let chat_id = Uuid::new_v4();

    // Fill the cap with 2-chunk messages that never complete.
    for _ in 0..crate::MAX_CONCURRENT_PARTIAL_TEXT_PER_CHAT {
        mgr.register_incoming_text_chunk(chat_id, Uuid::new_v4(), 0, 2, "a".to_string(), 0)
            .expect("within the concurrent cap");
    }
    let err = mgr
        .register_incoming_text_chunk(chat_id, Uuid::new_v4(), 0, 2, "a".to_string(), 0)
        .expect_err("beyond the concurrent cap must be rejected");
    assert!(err.to_string().contains("Too many concurrent"));

    // A further chunk for an already-tracked message is still accepted.
    let existing = *mgr
        .incoming_text_messages
        .keys()
        .find(|(c, _)| *c == chat_id)
        .expect("a partial message exists");
    mgr.register_incoming_text_chunk(existing.0, existing.1, 1, 2, "b".to_string(), 0)
        .expect("completing an existing message stays allowed");
}

// ── Incoming files ──────────────────────────────────────────────────────────

#[test]
fn sequential_incoming_files_do_not_reuse_completed_transfer_state() {
    let temp_dir = tempdir().unwrap();
    let mut mgr = manager_in(temp_dir.path(), true);
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Sequential Files".to_string());

    feed_file(&mut mgr, chat_id, "first.txt", b"first file payload", 1);
    feed_file(&mut mgr, chat_id, "second.txt", b"second payload", 4);

    let paths = file_paths(&mgr, chat_id);
    assert_eq!(paths.len(), 2, "both files should be recorded");
    assert!(
        mgr.active_transfers.is_empty(),
        "completed transfers should not remain active"
    );
    assert!(
        mgr.incoming_files.is_empty(),
        "no incoming file handles should remain after completion"
    );
    assert_eq!(std::fs::read(&paths[0]).unwrap(), b"first file payload");
    assert_eq!(std::fs::read(&paths[1]).unwrap(), b"second payload");
}

/// A gated manager holding one complete incoming offer ("offer.txt").
fn manager_with_held_incoming_file(temp_dir: &Path) -> (ChatManager, Uuid, Vec<u8>) {
    let mut mgr = manager_in(temp_dir, false);
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Gated Files".to_string());
    let payload = b"held until accepted".to_vec();
    feed_file(&mut mgr, chat_id, "offer.txt", &payload, 1);
    (mgr, chat_id, payload)
}

/// Held offers share one spool cap per conversation, so a peer cannot queue
/// many just-under-the-cap files onto the disk of someone who agreed to none.
#[test]
fn held_offers_share_one_spool_cap_per_conversation() {
    let temp_dir = tempdir().unwrap();
    let mut mgr = manager_in(temp_dir.path(), false);
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Peer".to_string());
    let _app_rx = wire_session_for_test(&mut mgr, chat_id);

    // Each offer is under the cap on its own; together they are over it.
    let offer = files::MAX_UNACCEPTED_SPOOL_BYTES * 3 / 5;
    let block = vec![0u8; crate::FILE_CHUNK_SIZE];
    let mut seq = 0;
    for name in ["first.bin", "second.bin"] {
        seq += 1;
        frame(&mut mgr, chat_id, meta(name, offer, seq));
        let mut left = offer as usize;
        while left > 0 {
            let n = left.min(block.len());
            left -= n;
            seq += 1;
            frame(&mut mgr, chat_id, chunk(&block[..n], seq));
        }
        seq += 1;
        frame(&mut mgr, chat_id, ProtocolMessage::FileEnd { seq });
    }

    let status = |name: &str| {
        mgr.active_transfers_snapshot()
            .into_iter()
            .find(|t| t.filename == name)
            .map(|t| t.status)
    };
    assert_eq!(
        status("first.bin"),
        Some(TransferStatus::AwaitingAcceptance)
    );
    assert_eq!(
        status("second.bin"),
        Some(TransferStatus::Cancelled),
        "the offer that took the conversation past the cap must be declined"
    );
}

#[test]
fn incoming_file_is_held_until_accepted() {
    let temp_dir = tempdir().unwrap();
    let (mut mgr, chat_id, payload) = manager_with_held_incoming_file(temp_dir.path());
    let final_path = temp_dir.path().join("downloads").join("offer.txt");

    // Fully streamed, but not accepted: nothing in the chat or downloads.
    let transfers = mgr.active_transfers_snapshot();
    assert_eq!(transfers.len(), 1);
    assert_eq!(transfers[0].status, TransferStatus::AwaitingAcceptance);
    assert!(
        !has_file_message(&mgr, chat_id),
        "no file message before acceptance"
    );
    assert!(
        !final_path.exists(),
        "file must not land in downloads before acceptance"
    );

    // Accept → finalized into downloads + recorded in the chat.
    mgr.accept_incoming_file(transfers[0].id).unwrap();
    assert_eq!(std::fs::read(&final_path).unwrap(), payload);
    assert!(mgr.chats[&chat_id].messages.iter().any(|m| {
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
    let spooled = downloads_left(temp_dir.path());
    assert!(
        spooled.is_empty(),
        "declined transfer must leave nothing on disk, found: {:?}",
        spooled
    );
    assert!(!has_file_message(&mgr, chat_id));
    assert_eq!(
        mgr.active_transfers_snapshot()[0].status,
        TransferStatus::Cancelled
    );

    // Late chunks for the declined transfer are discarded (no new spool file).
    frame(&mut mgr, chat_id, chunk(b"late", 4));
    assert!(mgr.incoming_files.is_empty());

    // Double-decline / accept-after-decline are rejected cleanly.
    assert!(mgr.reject_incoming_file(transfer_id).is_err());
    assert!(mgr.accept_incoming_file(transfer_id).is_err());

    // A fresh offer afterwards works (cancelled state is cleared).
    frame(&mut mgr, chat_id, meta("second.txt", 4, 5));
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
    let mut mgr = manager_in(temp_dir.path(), false);
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Early Accept".to_string());

    let payload = b"accepted mid-stream";
    frame(
        &mut mgr,
        chat_id,
        meta("early.txt", payload.len() as u64, 1),
    );
    // Accept while the stream is still going.
    let transfer_id = mgr.active_transfers_snapshot()[0].id;
    mgr.accept_incoming_file(transfer_id).unwrap();
    frame(&mut mgr, chat_id, chunk(payload, 2));
    frame(&mut mgr, chat_id, ProtocolMessage::FileEnd { seq: 3 });

    let final_path = temp_dir.path().join("downloads").join("early.txt");
    assert_eq!(std::fs::read(&final_path).unwrap(), payload.to_vec());
    assert!(mgr.active_transfers.is_empty());
}

/// Declining must send `FileCancel`, or a declined offer still crosses the
/// wire in full.
#[test]
fn declining_a_file_tells_the_sender_to_stop() {
    let (mut mgr, chat_id) = manager_with_chat("Peer");
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

/// A disconnect mid-receive fails the transfer and frees the incoming slot.
#[test]
fn disconnect_fails_an_in_flight_incoming_transfer() {
    let mut mgr = ChatManager::default();
    let (session_id, incoming) = incoming_chat(&mut mgr);
    let _app_rx = wire_session_for_test(&mut mgr, session_id);
    let transfer_id = mgr
        .start_receiving_file(incoming, "big.bin", 1_000_000)
        .unwrap();

    mgr.handle_session_event(session_id, SessionEvent::Disconnected);

    let status = transfer_status(&mgr, transfer_id)
        .expect("the transfer is still listed, so its state must be honest");
    assert!(
        matches!(status, TransferStatus::Failed(_)),
        "expected Failed, got {:?}",
        status
    );
    assert!(
        mgr.streaming_transfer(incoming).is_none(),
        "the chat's incoming slot must be free so the peer can retry"
    );
}

// ── Delivery receipts ───────────────────────────────────────────────────────

fn ack(acked_seq: u64, seq: u64) -> ProtocolMessage {
    ProtocolMessage::Ack { acked_seq, seq }
}

#[test]
fn sent_text_is_marked_delivered_by_peer_ack() {
    let mut mgr = ChatManager::default();
    let (chat_id, _rx) = chat_with_session(&mut mgr, "Receipts");

    mgr.send_message(chat_id, "hello".to_string()).unwrap();
    let msg_id = mgr.chats[&chat_id].messages[0].id;
    assert!(!mgr.chats[&chat_id].messages[0].delivered);

    // The session loop reports the stamped wire seq; the peer acks that seq.
    mgr.handle_session_event(chat_id, SessionEvent::TextSendComplete { seq: 7 });
    frame(&mut mgr, chat_id, ack(7, 1));

    let msg = &mgr.chats[&chat_id].messages[0];
    assert_eq!(msg.id, msg_id);
    assert!(msg.delivered, "peer ack must mark the message delivered");

    // A replayed/duplicate ack is harmless.
    frame(&mut mgr, chat_id, ack(7, 2));
    assert!(mgr.chats[&chat_id].messages[0].delivered);
}

#[test]
fn received_text_queues_a_delivery_receipt() {
    let mut mgr = ChatManager::default();
    let (chat_id, mut rx) = chat_with_session(&mut mgr, "Receipts");
    frame(
        &mut mgr,
        chat_id,
        ProtocolMessage::Text {
            text: "hi".to_string(),
            timestamp: 1,
            seq: 5,
        },
    );
    match rx.try_recv() {
        Ok(ProtocolMessage::Ack { acked_seq, .. }) => assert_eq!(acked_seq, 5),
        other => panic!("expected a queued Ack for seq 5, got {:?}", other),
    }
}

// ── Data deletion ───────────────────────────────────────────────────────────

#[test]
fn delete_all_data_removes_files_and_clears_state() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().join("data");
    let history_path = data_dir.join("history.json.enc");
    let identity_path = data_dir.join("identity.json");
    let crash_log_path = data_dir.join("diagnostics/crashes/panic.log");
    std::fs::create_dir_all(crash_log_path.parent().unwrap()).unwrap();
    std::fs::write(&history_path, b"encrypted-history").unwrap();
    std::fs::write(&identity_path, b"encrypted-identity").unwrap();
    std::fs::write(&crash_log_path, b"crash").unwrap();

    let (mut mgr, chat_id) = manager_with_chat("Chat");
    let contact_id = mgr.add_contact(
        "Contact".to_string(),
        Some("127.0.0.1:12345".to_string()),
        None,
        None,
    );
    mgr.contact_to_chat.insert(contact_id, chat_id);
    mgr.queue_fingerprint_request(PendingFingerprint {
        fingerprint: "fingerprint".to_string(),
        peer_name: "peer".to_string(),
        sas: String::new(),
        session_id: chat_id,
        warning: None,
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

    // The shells flush history on close; after a wipe that must not write.
    assert!(mgr.data_wiped());
    assert!(mgr.save_history(&history_path).is_err());
    assert!(!history_path.exists());
}

/// A snapshot taken under the lock before a wipe must not be written after it
/// (Codex review on PR #191).
#[test]
fn a_snapshot_taken_before_a_wipe_cannot_be_written_after_it() {
    let dir = tempdir().unwrap();
    let history_path = dir.path().join("history.json.enc");
    let identity_path = dir.path().join("identity.json");
    let (mut mgr, _) = manager_with_chat("Chat");
    mgr.set_history_key(rand::random());

    let (late, late_key) = mgr.history_snapshot().unwrap();
    mgr.delete_all_data(dir.path(), &history_path, &identity_path)
        .unwrap();
    assert!(late.save_encrypted(&history_path, &late_key).is_err());
    assert!(!history_path.exists());
}

/// `P2PEM_DATA_DIR` can be shared: only what the app wrote is removed.
#[test]
fn delete_all_data_never_removes_files_it_did_not_write() {
    let dir = tempdir().unwrap();
    let data_dir = dir.path().to_path_buf();
    let history_path = data_dir.join("history.json.enc");
    let identity_path = data_dir.join("identity.json");
    for name in [
        "history.json.enc",
        "identity.json",
        "parties.json",
        "history.json.enc.unreadable-20261104T000000Z",
        "thesis.docx",
    ] {
        std::fs::write(data_dir.join(name), b"x").unwrap();
    }

    ChatManager::default()
        .delete_all_data(&data_dir, &history_path, &identity_path)
        .unwrap();

    let left: Vec<_> = std::fs::read_dir(&data_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, vec!["thesis.docx".to_string()]);
}

// ── Session teardown, TOFU queueing, reconnect ──────────────────────────────

/// Deleting a host-side chat must close the session it maps to.
#[test]
fn deleting_a_host_side_chat_closes_its_mapped_session() {
    let mut mgr = ChatManager::default();
    let (session_id, incoming) = incoming_chat(&mut mgr);
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

/// Two peers mid-handshake at once must each get a prompt.
#[test]
fn concurrent_tofu_prompts_queue_instead_of_overwriting() {
    let mut mgr = ChatManager::default();
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    // Receivers must outlive the assertions, or `confirm_fingerprint` fails.
    let mut confirm_rx = Vec::new();
    for (session, name) in [(first, "Peer A"), (second, "Peer B")] {
        mgr.create_local_chat_for_test(session, name.into());
        confirm_rx.push(confirm_channel(&mut mgr, session));
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

    mgr.confirm_fingerprint(second, false).unwrap();
    assert_eq!(mgr.pending_fingerprint_count(), 0);
    drop(confirm_rx);
}

/// NET-05: any machine can complete the handshake with a throwaway key, and
/// each such caller queued a safety-code prompt. Nothing bounded the queue,
/// so one machine could bury the real prompts under hundreds. Past the cap a
/// caller is refused (its session told to abort) and the user is told why.
#[test]
fn unverified_callers_cannot_queue_prompts_without_limit() {
    let mut mgr = ChatManager::default();
    let mut answers = Vec::new();
    for i in 0..(MAX_QUEUED_PROMPTS + 5) {
        let session = Uuid::new_v4();
        mgr.create_local_chat_for_test(session, format!("Peer {i}"));
        let (tx, rx) = mpsc::unbounded_channel();
        mgr.add_fingerprint_confirm_sender_for_test(session, tx);
        mgr.handle_tofu_verification(session, &format!("FP-{i}"), "Peer", "");
        answers.push(rx);
    }
    assert_eq!(mgr.pending_fingerprint_count(), MAX_QUEUED_PROMPTS);
    for (i, rx) in answers.iter_mut().enumerate() {
        let got = rx.try_recv().ok();
        if i < MAX_QUEUED_PROMPTS {
            assert_eq!(got, None, "caller {i} is waiting for the user");
        } else {
            assert_eq!(got, Some(false), "caller {i} should have been refused");
        }
    }
    assert!(
        mgr.toasts
            .iter()
            .any(|t| t.message.contains("waiting to be verified")),
        "{:?}",
        mgr.toasts.iter().map(|t| &t.message).collect::<Vec<_>>()
    );
}

/// NET-05: a caller that hung up without being verified left a persisted
/// "Peer ab12…" conversation behind — one per throwaway key, forever. An
/// incoming conversation that was never verified and holds nothing goes with
/// its session; one the user accepted, or one with messages, stays.
#[test]
fn a_caller_that_leaves_unverified_leaves_no_conversation() {
    for accept in [false, true] {
        let mut mgr = ChatManager::default();
        let listener = Uuid::new_v4();
        mgr.create_local_chat_for_test(listener, "Host on :12345".into());
        mgr.chats.get_mut(&listener).unwrap().is_host_placeholder = true;
        let (tx, _rx) = mpsc::unbounded_channel();
        mgr.add_fingerprint_confirm_sender_for_test(listener, tx);
        let offered = Uuid::new_v4();
        mgr.handle_session_event(
            listener,
            SessionEvent::NewConnection {
                peer_addr: "198.51.100.9:50000".into(),
                fingerprint: "ab".repeat(32),
                sas: "123456".into(),
                chat_id: offered,
            },
        );
        assert!(
            mgr.chats.contains_key(&offered),
            "an incoming conversation was opened"
        );
        if accept {
            mgr.confirm_fingerprint(listener, true).unwrap();
        }
        mgr.handle_session_event(listener, SessionEvent::Disconnected);
        assert_eq!(
            mgr.chats.contains_key(&offered),
            accept,
            "accepted: {accept} — a verified conversation must survive its session, an unverified empty one must not"
        );
    }
}

/// A dead session's prompt can never be answered and would block the queue.
#[test]
fn a_dead_session_drops_its_queued_tofu_prompt() {
    let (mut mgr, session_id) = manager_with_chat("Peer");
    let _rx = confirm_channel(&mut mgr, session_id);
    mgr.handle_tofu_verification(session_id, "FP", "Peer", "");
    assert_eq!(mgr.pending_fingerprint_count(), 1);

    mgr.handle_session_event(session_id, SessionEvent::Disconnected);

    assert_eq!(mgr.pending_fingerprint_count(), 0);
}

fn connected(mgr: &mut ChatManager, chat_id: Uuid, peer: &str) {
    mgr.handle_session_event(chat_id, SessionEvent::Connected { peer: peer.into() });
}

/// Reconnecting keeps a user-chosen title but still labels an unnamed chat.
#[test]
fn reconnect_labels_only_unnamed_chats() {
    let (mut mgr, named) = manager_with_chat("192.168.1.20:12345");
    mgr.rename_chat(named, "Mum".into()).unwrap();
    assert!(mgr.get_chat(named).unwrap().title_is_custom);
    connected(&mut mgr, named, "192.168.1.20:12345");
    assert_eq!(mgr.get_chat(named).unwrap().title, "Mum");

    let unnamed = Uuid::new_v4();
    mgr.create_local_chat_for_test(unnamed, "connecting".into());
    connected(&mut mgr, unnamed, "10.0.0.5:9000");
    assert_eq!(mgr.get_chat(unnamed).unwrap().title, "10.0.0.5:9000");
}

/// The wire sequence restarts per session but `recv_seq` lives on the Chat,
/// so a reconnect must reset it or the peer's messages look like replays.
#[test]
fn reconnect_resets_the_replay_high_water_mark() {
    let (mut mgr, chat_id) = manager_with_chat("Peer");
    mgr.get_chat_mut(chat_id).unwrap().recv_seq = 42;

    connected(&mut mgr, chat_id, "10.0.0.5:9000");
    assert_eq!(mgr.get_chat(chat_id).unwrap().recv_seq, 0);

    frame(&mut mgr, chat_id, text(1, "first message after reconnect"));
    assert_eq!(mgr.get_chat(chat_id).unwrap().messages.len(), 1);
}

/// Same, host side: a returning peer reuses its chat id.
#[test]
fn host_side_reconnect_resets_the_replay_high_water_mark() {
    let (mut mgr, incoming) = manager_with_chat("Peer");
    mgr.get_chat_mut(incoming).unwrap().recv_seq = 99;
    let session_id = Uuid::new_v4();
    let _rx = confirm_channel(&mut mgr, session_id);

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

/// APP-04: a chunk that cannot be written — a full disk, or a peer sending
/// past the size it declared — failed the transfer here and told the sender
/// nothing. It kept streaming (up to 10 GiB) into nothing, then reported
/// "File sent". The sender is told to stop, and the stream is closed so the
/// rest of its chunks are not routed anywhere.
#[test]
fn a_chunk_that_cannot_be_written_tells_the_sender_to_stop() {
    let dir = tempfile::tempdir().unwrap();
    let mut mgr = ChatManager::default();
    mgr.config.download_dir = dir.path().to_path_buf();
    mgr.config.auto_accept_files = true;
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Peer".into());
    let mut app_rx = wire_session_for_test(&mut mgr, chat_id);

    mgr.handle_session_event(
        chat_id,
        SessionEvent::MessageReceived(ProtocolMessage::FileMeta {
            filename: "four.bin".into(),
            size: 4,
            seq: 1,
        }),
    );
    // Past the declared size: the receiver refuses to write it.
    mgr.handle_session_event(
        chat_id,
        SessionEvent::MessageReceived(ProtocolMessage::FileChunk {
            chunk: vec![0u8; 64],
            seq: 2,
        }),
    );

    let frames: Vec<ProtocolMessage> = std::iter::from_fn(|| app_rx.try_recv().ok()).collect();
    assert!(
        frames
            .iter()
            .any(|f| matches!(f, ProtocolMessage::FileCancel { .. })),
        "the sender was not told: {frames:?}"
    );
    assert!(
        !mgr.incoming_streams.contains_key(&chat_id),
        "the dead transfer still owns the stream"
    );
}

/// APP-05: a held offer whose last byte had arrived is not "stuck" — the file
/// is complete on disk and waits only for the user. A disconnect failed it
/// anyway and deleted the file, so a peer that sent and quit (or a network
/// blip) cost the recipient the whole download. It stays acceptable, and
/// accepting it after the session is gone still saves it.
#[test]
fn a_complete_offer_survives_the_sender_hanging_up() {
    let dir = tempfile::tempdir().unwrap();
    let mut mgr = ChatManager::default();
    mgr.config.download_dir = dir.path().to_path_buf();
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Peer".into());
    let _app_rx = wire_session_for_test(&mut mgr, chat_id);
    for frame in [
        ProtocolMessage::FileMeta {
            filename: "done.txt".into(),
            size: 5,
            seq: 1,
        },
        ProtocolMessage::FileChunk {
            chunk: b"hello".to_vec(),
            seq: 2,
        },
        ProtocolMessage::FileEnd { seq: 3 },
    ] {
        mgr.handle_session_event(chat_id, SessionEvent::MessageReceived(frame));
    }
    let transfer_id = *mgr
        .active_transfers
        .iter()
        .find(|(_, t)| t.status == TransferStatus::AwaitingAcceptance)
        .expect("held for the user")
        .0;

    mgr.handle_session_event(chat_id, SessionEvent::Disconnected);
    assert_eq!(
        mgr.active_transfers
            .get(&transfer_id)
            .map(|t| t.status.clone()),
        Some(TransferStatus::AwaitingAcceptance),
        "a complete offer was failed by the disconnect"
    );

    mgr.accept_incoming_file(transfer_id)
        .expect("accept offline");
    assert_eq!(
        std::fs::read(dir.path().join("done.txt")).unwrap(),
        b"hello"
    );
}

/// The other half of the rule: an offer still *streaming* when the session
/// dies is incomplete, and is failed and cleaned up as before.
#[test]
fn an_incomplete_offer_is_still_failed_by_a_disconnect() {
    let dir = tempfile::tempdir().unwrap();
    let mut mgr = ChatManager::default();
    mgr.config.download_dir = dir.path().to_path_buf();
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Peer".into());
    let _app_rx = wire_session_for_test(&mut mgr, chat_id);
    for frame in [
        ProtocolMessage::FileMeta {
            filename: "half.txt".into(),
            size: 10,
            seq: 1,
        },
        ProtocolMessage::FileChunk {
            chunk: b"hello".to_vec(),
            seq: 2,
        },
    ] {
        mgr.handle_session_event(chat_id, SessionEvent::MessageReceived(frame));
    }
    mgr.handle_session_event(chat_id, SessionEvent::Disconnected);
    assert!(mgr
        .active_transfers
        .values()
        .all(|t| matches!(t.status, TransferStatus::Failed(_))));
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "spool left behind"
    );
}

/// APP-02: `send_file` awaited room on the bounded file lane while the caller
/// held the manager lock. A lane still full from a cancelled send, behind a
/// stalled peer, parked that await — and with it every desktop command and the
/// poll loop, which all need the lock: the app froze. A full lane is now an
/// immediate "busy, try again".
#[tokio::test]
async fn a_full_file_lane_refuses_a_send_instead_of_freezing_the_app() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f.txt");
    std::fs::write(&file, b"data").unwrap();
    let mut mgr = ChatManager::default();
    let chat_id = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat_id, "Peer".into());
    let (app_tx, _app_rx) = mpsc::unbounded_channel();
    let (file_tx, _file_rx) = mpsc::channel(1);
    // Left over from an earlier send the peer never drained.
    file_tx
        .try_send(ProtocolMessage::FileChunk {
            chunk: vec![0],
            seq: 0,
        })
        .unwrap();
    mgr.add_session_for_test(
        chat_id,
        SessionHandle {
            from_app_tx: app_tx,
            file_tx,
        },
    );

    let res = tokio::time::timeout(Duration::from_secs(2), mgr.send_file(chat_id, file))
        .await
        .expect("send_file blocked on a full lane while holding the manager");
    let err = res.unwrap_err().to_string();
    assert!(err.contains("try again"), "{err}");
    assert!(
        mgr.active_outgoing_transfer_id_for_chat(chat_id).is_none(),
        "a refused send must not leave a transfer behind"
    );
}

/// APP-07: spools live in the download folder (`tmp_<uuid>_<name>`, because
/// finalizing is a rename), and after a crash or an exit with an offer still
/// held nothing ever removed them — peer-chosen bytes under a peer-chosen
/// extension, in ~/Downloads, forever. Loading the history sweeps them; it
/// touches nothing else, and nothing while a receive is in flight.
#[test]
fn leftover_spools_are_swept_and_nothing_else_is() {
    let dir = tempfile::tempdir().unwrap();
    let orphan = dir
        .path()
        .join(format!("tmp_{}_invoice.pdf", Uuid::new_v4()));
    let mine = dir.path().join("tmp_notes.txt");
    let lookalike = dir
        .path()
        .join("tmp_not-a-uuid-at-all-but-36-chars-x_y.txt");
    for p in [&orphan, &mine, &lookalike] {
        std::fs::write(p, b"x").unwrap();
    }
    let mut mgr = ChatManager::default();
    mgr.config.download_dir = dir.path().to_path_buf();

    assert_eq!(mgr.sweep_orphaned_spools(), 1);
    assert!(!orphan.exists(), "the orphaned spool survived");
    assert!(
        mine.exists() && lookalike.exists(),
        "a file the app did not write was removed"
    );

    // With a receive in flight its own spool must not be touched.
    let chat = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat, "Peer".into());
    let _rx = wire_session_for_test(&mut mgr, chat);
    mgr.handle_session_event(
        chat,
        SessionEvent::MessageReceived(ProtocolMessage::FileMeta {
            filename: "live.bin".into(),
            size: 10,
            seq: 1,
        }),
    );
    assert_eq!(mgr.sweep_orphaned_spools(), 0);
    assert_eq!(
        std::fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| e
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("tmp_"))
            .count(),
        3,
        "the live spool was removed"
    );
}

/// TUI-15: closing a connection must not cost the conversation. The TUI's
/// `:disconnect` deleted it — history and all, saved at once. This closes the
/// session through the same path as a dropped connection and keeps the rest.
#[test]
fn disconnecting_keeps_the_conversation_and_its_history() {
    let mut mgr = ChatManager::default();
    let chat = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat, "Peer".into());
    mgr.get_chat_mut(chat).unwrap().peer_fingerprint = Some("ab".repeat(32));
    mgr.get_chat_mut(chat)
        .unwrap()
        .messages
        .push(messenger_core::types::Message {
            id: Uuid::new_v4(),
            from_me: false,
            content: messenger_core::types::MessageContent::Text {
                text: "kept".into(),
            },
            timestamp: chrono::Utc::now(),
            delivered: false,
        });
    let _rx = wire_session_for_test(&mut mgr, chat);
    assert!(mgr.is_connected(&chat));

    assert!(mgr.disconnect_chat(chat));
    assert!(!mgr.is_connected(&chat));
    assert!(!mgr.sessions.contains_key(&chat));
    assert_eq!(
        mgr.get_chat(chat).unwrap().messages.len(),
        1,
        "the history went with it"
    );
    assert!(!mgr.disconnect_chat(chat), "nothing left to close");
}

/// PRIV-06 / JOURNEY-07: a notification put the message text into the OS's
/// notification store and on the lock screen, outside the app's encryption,
/// with no way to say less. By default it names the sender only; "hidden"
/// names nobody; "full" is the old behaviour, on request.
#[test]
fn a_notification_says_only_what_the_setting_allows() {
    use crate::types::NotificationContent;
    let mut mgr = ChatManager::default();
    let chat = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat, "Alice".into());
    assert_eq!(
        mgr.config.notification_content,
        NotificationContent::SenderOnly
    );

    let shown = |mgr: &ChatManager| mgr.notification_text(chat, "the secret plan");
    assert_eq!(shown(&mgr), ("Alice".into(), "New message".into()));
    mgr.config.notification_content = NotificationContent::Hidden;
    assert_eq!(shown(&mgr), ("P2PEM".into(), "New message".into()));
    mgr.config.notification_content = NotificationContent::Full;
    assert_eq!(shown(&mgr), ("Alice".into(), "the secret plan".into()));
}

/// APP-17: deleting a conversation left the contact pointing at it, so the
/// next "connect" to that contact aimed at a chat id that no longer existed.
/// The association goes with the chat; the contact itself stays.
#[test]
fn deleting_a_chat_unlinks_its_contact() {
    let mut mgr = ChatManager::new(Config::default());
    let chat_id = Uuid::new_v4();
    let kept = Uuid::new_v4();
    let contact = mgr.add_contact(
        "Contact".to_string(),
        Some("127.0.0.1:12345".to_string()),
        None,
        None,
    );
    let other = mgr.add_contact(
        "Other".to_string(),
        Some("127.0.0.1:12346".to_string()),
        None,
        None,
    );
    mgr.create_local_chat_for_test(chat_id, "Chat".to_string());
    mgr.create_local_chat_for_test(kept, "Kept".to_string());
    mgr.contact_to_chat.insert(contact, chat_id);
    mgr.contact_to_chat.insert(other, kept);

    mgr.delete_chat(chat_id);

    assert!(
        mgr.contacts.contains_key(&contact),
        "the contact was deleted with the chat"
    );
    assert_eq!(
        mgr.contact_to_chat.get(&contact),
        None,
        "still linked to a deleted chat"
    );
    assert_eq!(
        mgr.contact_to_chat.get(&other),
        Some(&kept),
        "an unrelated link was dropped"
    );
}

/// APP-16: `clear_history` aborted every session task — the listener
/// included — but left `is_hosting` and `hosting_port` set, so auto-rehost
/// promptly opened the port again: "erase everything" quietly resumed
/// listening. Clearing stops hosting outright.
#[tokio::test]
async fn clearing_history_stops_hosting_for_good() {
    let dir = tempdir().unwrap();
    let mut mgr = ChatManager::new(Config::default());
    let key = crate::identity::Identity::new_with_plaintext("H".into())
        .unwrap()
        .private_key()
        .unwrap();
    mgr.start_host(0, key).await.unwrap();
    assert!(mgr.is_hosting && mgr.hosting_port.is_some());

    mgr.clear_history(&dir.path().join("history.json.enc"));

    assert!(!mgr.is_hosting, "still marked as hosting");
    assert_eq!(mgr.hosting_port, None);
    assert!(
        !mgr.check_rehost_needed(),
        "auto-rehost would reopen the port"
    );
}

/// APP-15: an unsigned (v1) link can be written by anyone: it only has to
/// name a fingerprint with its matching public key, both of which are public.
/// Importing one for a contact the user already had replaced that contact's
/// name and address — a verified contact's next dial went wherever the forger
/// said. An unsigned link may add a contact, never change an existing one; a
/// signed link, which only the key's owner can make, still refreshes it.
#[test]
fn an_unsigned_invite_cannot_change_an_existing_contact() {
    use crate::identity::Identity;
    let peer = Identity::new_with_plaintext("Real Bob".into()).unwrap();
    let mut mgr = ChatManager::new(Config::default());
    let signed = peer
        .generate_signed_invite_link(Some("192.0.2.10:12345".into()))
        .unwrap();
    let contact = mgr.parse_invite_link(&signed).unwrap();
    let id = mgr.import_contact_from_link(contact, true).unwrap();
    mgr.contacts.get_mut(&id).unwrap().trust_state = TrustState::Verified;

    // A forger's v1 link: Bob's (public) key and fingerprint, their address.
    let forged = mgr
        .generate_invite_link(
            "Bob (new number)",
            Some("203.0.113.66:12345".into()),
            &peer.fingerprint,
            &peer.public_key_pem,
        )
        .unwrap();
    let contact = mgr.parse_invite_link(&forged).unwrap();
    let err = mgr.import_contact_from_link(contact, false).unwrap_err();
    assert!(err.to_string().contains("unsigned"), "{err}");
    let kept = &mgr.contacts[&id];
    assert_eq!(kept.name, "Real Bob");
    assert_eq!(kept.address.as_deref(), Some("192.0.2.10:12345"));
    assert_eq!(kept.trust_state, TrustState::Verified);

    // Bob's own signed link with a new address is a genuine refresh.
    let moved = peer
        .generate_signed_invite_link(Some("192.0.2.20:12345".into()))
        .unwrap();
    let contact = mgr.parse_invite_link(&moved).unwrap();
    assert_eq!(mgr.import_contact_from_link(contact, true).unwrap(), id);
    assert_eq!(
        mgr.contacts[&id].address.as_deref(),
        Some("192.0.2.20:12345")
    );
    assert_eq!(mgr.contacts[&id].trust_state, TrustState::Verified);
}

/// One listener at a time: a second `start_host` is refused while the first
/// is live (and so is relay hosting), but a placeholder whose session has
/// died — its task gone, nothing listening — is cleared and replaced rather
/// than blocking hosting for the rest of the process.
#[tokio::test]
async fn hosting_twice_is_refused_but_a_dead_listener_is_replaced() {
    let mut mgr = ChatManager::default();
    let first = mgr.start_host(0, throwaway_key()).await.unwrap();

    let again = mgr.start_host(0, throwaway_key()).await;
    assert!(
        again.unwrap_err().to_string().contains("Already listening"),
        "a second listener was started"
    );
    let relay = mgr
        .start_host_via_relay("127.0.0.1:1", None, throwaway_key())
        .await;
    assert!(relay.unwrap_err().to_string().contains("Already hosting"));
    assert_eq!(
        mgr.chats.values().filter(|c| c.is_host_placeholder).count(),
        1
    );

    // The session behind the placeholder is gone (as after a crash in its
    // task): the next start clears it and listens again.
    mgr.teardown_session(first);
    let second = mgr.start_host(0, throwaway_key()).await.unwrap();
    assert_ne!(second, first);
    assert!(
        !mgr.chats.contains_key(&first),
        "the dead placeholder stayed"
    );
    assert_eq!(
        mgr.chats.values().filter(|c| c.is_host_placeholder).count(),
        1
    );
    mgr.stop_hosting();
}

/// Auto-rehost is a timer, so a port that cannot be bound must not be retried
/// for ever while the app claims to be hosting: after
/// `MAX_CONSECUTIVE_REHOST_FAILURES` failures in a row it stops and says why.
/// A success in between resets the count — a transient failure is not one of
/// a run.
#[tokio::test]
async fn repeated_bind_failures_stop_hosting_but_a_success_resets_the_count() {
    use super::MAX_CONSECUTIVE_REHOST_FAILURES as MAX;
    let taken = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let mut mgr = ChatManager::default();
    mgr.is_hosting = true;
    mgr.hosting_port = Some(port);
    for _ in 0..MAX - 1 {
        assert!(mgr.start_host(port, throwaway_key()).await.is_err());
    }
    assert!(mgr.is_hosting, "stopped before the limit");
    // A success resets the run.
    let ok = mgr.start_host(0, throwaway_key()).await.unwrap();
    mgr.remove_host_placeholder(ok);
    for _ in 0..MAX - 1 {
        assert!(mgr.start_host(port, throwaway_key()).await.is_err());
    }
    assert!(mgr.is_hosting, "a success did not reset the failure count");
    assert!(mgr.start_host(port, throwaway_key()).await.is_err());
    assert!(
        !mgr.is_hosting,
        "hosting continued after {MAX} failures in a row"
    );
    assert_eq!(mgr.hosting_port, None);
    assert!(
        mgr.toasts
            .iter()
            .any(|t| t.message.starts_with("Stopped hosting —")),
        "the user was not told why"
    );
    drop(taken);
}

/// `auto_reconnect_contacts` runs when the desktop app unlocks. With
/// `auto_connect` off it dials nobody. On, it dials each mapped contact into
/// *its own* conversation (not a new one), skips a blocked contact, and drops a
/// mapping whose contact has been deleted rather than tripping on it each
/// start-up. (The blocked skip is two layers deep — `connect_to_contact`
/// refuses a blocked contact too — so this fails only when both go.)
#[tokio::test]
async fn auto_reconnect_dials_mapped_contacts_into_their_own_conversations() {
    use std::time::{Duration, Instant};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap().to_string();

    let mut mgr = ChatManager::default();
    let (alice_chat, blocked_chat, orphan_chat) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    for (id, title) in [
        (alice_chat, "Alice"),
        (blocked_chat, "Mallory"),
        (orphan_chat, "Gone"),
    ] {
        mgr.create_local_chat_for_test(id, title.into());
    }
    let alice = mgr.add_contact("Alice".into(), Some(addr.clone()), None, None);
    let mallory = mgr.add_contact("Mallory".into(), Some(addr.clone()), None, None);
    mgr.block_contact(mallory).unwrap();
    let deleted = Uuid::new_v4();
    mgr.contact_to_chat.insert(alice, alice_chat);
    mgr.contact_to_chat.insert(mallory, blocked_chat);
    mgr.contact_to_chat.insert(deleted, orphan_chat);
    let chats_before = mgr.chats.len();

    mgr.config.auto_connect = false;
    mgr.auto_reconnect_contacts(&throwaway_key()).await;
    assert!(mgr.sessions.is_empty(), "dialled with auto_connect off");
    assert!(
        mgr.contact_to_chat.contains_key(&deleted),
        "touched state while off"
    );

    mgr.config.auto_connect = true;
    mgr.auto_reconnect_contacts(&throwaway_key()).await;
    assert!(
        mgr.sessions.contains_key(&alice_chat),
        "Alice was not dialled into her chat"
    );
    assert!(
        !mgr.sessions.contains_key(&blocked_chat),
        "a blocked contact was dialled"
    );
    assert!(
        !mgr.contact_to_chat.contains_key(&deleted),
        "the stale mapping stayed"
    );
    assert_eq!(mgr.contact_to_chat.get(&alice), Some(&alice_chat));
    assert_eq!(
        mgr.chats.len(),
        chats_before,
        "a reconnect opened a new conversation"
    );

    // And the dial really reached the address: exactly one connection.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut accepted = 0;
    while Instant::now() < deadline && accepted == 0 {
        match listener.accept() {
            Ok(_) => accepted += 1,
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    assert_eq!(accepted, 1, "nothing dialled the contact's address");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        listener.accept().is_err(),
        "the blocked contact was dialled too"
    );
}

/// Three ways a peer's file offer ends before it starts, each of which must
/// leave nothing behind and tell the sender to stop: a new offer arriving
/// while the previous file is still streaming (the old spool is discarded and
/// that transfer failed, not left at 10% for ever), an offer over
/// `MAX_FILE_SIZE`, and a download folder that cannot be written.
#[tokio::test]
async fn a_file_offer_that_cannot_proceed_cleans_up_and_tells_the_sender() {
    let dir = tempfile::tempdir().unwrap();
    let mut mgr = manager_in(dir.path(), false);
    let chat = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat, "Peer".into());
    let mut control = wire_session_for_test(&mut mgr, chat);
    let cancels = |rx: &mut mpsc::UnboundedReceiver<ProtocolMessage>| {
        std::iter::from_fn(|| rx.try_recv().ok())
            .filter(|m| matches!(m, ProtocolMessage::FileCancel { .. }))
            .count()
    };

    // Superseded mid-stream.
    frame(&mut mgr, chat, meta("first.bin", 100, 1));
    frame(&mut mgr, chat, chunk(&[7; 10], 2));
    let first = mgr.active_transfers_snapshot()[0].id;
    frame(&mut mgr, chat, meta("second.bin", 50, 3));
    // The new offer clears finished rows, so the user learns of the lost file
    // from the toast, and the row does not linger at 10%.
    assert_eq!(
        transfer_status(&mgr, first),
        None,
        "the abandoned row lingered"
    );
    assert!(
        mgr.toasts.iter().any(|t| t.message
            == "File transfer interrupted (the sender started another file): first.bin"),
        "the half-received file vanished without a word"
    );
    let spools: Vec<_> = downloads_left(dir.path())
        .into_iter()
        .filter(|n| n.to_string_lossy().contains("first.bin"))
        .collect();
    assert!(spools.is_empty(), "the abandoned spool stayed: {spools:?}");
    let second = mgr
        .active_transfers_snapshot()
        .into_iter()
        .find(|t| t.filename == "second.bin")
        .expect("the new offer was not taken");
    assert_eq!(second.status, TransferStatus::AwaitingAcceptance);
    frame(&mut mgr, chat, ProtocolMessage::FileCancel { seq: 4 });
    let _ = cancels(&mut control);

    // Too big to accept at all.
    let before = mgr.active_transfers_snapshot().len();
    frame(
        &mut mgr,
        chat,
        meta("huge.iso", crate::MAX_FILE_SIZE + 1, 5),
    );
    assert_eq!(cancels(&mut control), 1, "the sender was not told to stop");
    assert_eq!(mgr.active_transfers_snapshot().len(), before);
    assert!(mgr
        .toasts
        .iter()
        .any(|t| t.message.starts_with("Failed to receive file")));

    // Nowhere to write it: the download "folder" is a file.
    let blocker = dir.path().join("not-a-folder");
    std::fs::write(&blocker, b"x").unwrap();
    mgr.config.download_dir = blocker;
    frame(&mut mgr, chat, meta("third.bin", 10, 6));
    assert_eq!(cancels(&mut control), 1, "the sender was not told to stop");
    let third = mgr
        .active_transfers_snapshot()
        .into_iter()
        .find(|t| t.filename == "third.bin")
        .expect("the failed offer is not shown");
    assert!(
        matches!(third.status, TransferStatus::Failed(_)),
        "{:?}",
        third.status
    );
    // A failed offer does not block the next one.
    mgr.config.download_dir = dir.path().join("downloads");
    frame(&mut mgr, chat, meta("fourth.bin", 10, 7));
    assert!(mgr
        .active_transfers_snapshot()
        .iter()
        .any(|t| t.filename == "fourth.bin" && t.status.is_active()));
}

/// A background port mapping resolves on a later poll: success publishes the
/// external address (which the invite link then carries) and names the
/// protocol, failure says LAN and relay still work, and an attempt whose task
/// went away is forgotten quietly. Still pending, nothing changes.
#[tokio::test]
async fn a_port_mapping_result_is_applied_on_the_next_poll() {
    use crate::network::nat::{MappedAddress, Protocol};
    let mut mgr = ChatManager::default();

    let (tx, rx) = tokio::sync::oneshot::channel();
    mgr.pending_upnp = Some(rx);
    mgr.poll_session_events();
    assert!(mgr.pending_upnp.is_some(), "a pending attempt was dropped");
    tx.send(Ok(MappedAddress {
        external_ip: "203.0.113.5".parse().unwrap(),
        external_port: 40000,
        protocol: Protocol::NatPmp,
    }))
    .unwrap();
    mgr.poll_session_events();
    assert_eq!(mgr.external_address.as_deref(), Some("203.0.113.5:40000"));
    assert!(mgr.pending_upnp.is_none());
    assert!(mgr
        .toasts
        .iter()
        .any(|t| t.message == "NAT-PMP: reachable from the internet at 203.0.113.5:40000"));

    let mut mgr = ChatManager::default();
    let (tx, rx) = tokio::sync::oneshot::channel();
    mgr.pending_upnp = Some(rx);
    tx.send(Err(anyhow!("no gateway"))).unwrap();
    mgr.poll_session_events();
    assert_eq!(mgr.external_address, None);
    assert!(mgr.pending_upnp.is_none());
    assert!(mgr
        .toasts
        .iter()
        .any(|t| t.message == "UPnP port mapping failed: no gateway (LAN/relay still work)"));

    let mut mgr = ChatManager::default();
    let (tx, rx) = tokio::sync::oneshot::channel::<anyhow::Result<MappedAddress>>();
    mgr.pending_upnp = Some(rx);
    drop(tx);
    mgr.poll_session_events();
    assert!(mgr.pending_upnp.is_none());
    assert!(
        mgr.toasts.is_empty(),
        "a vanished attempt raised {:?}",
        mgr.toasts
    );
}
