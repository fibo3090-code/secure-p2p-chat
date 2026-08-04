//! Breadth coverage for ChatManager features that the other suites don't already
//! exercise: conversation rename/delete, history clearing, toast lifecycle,
//! incoming file-transfer state, typing indicators without a session,
//! contact import/association, and invite QR generation.

use p2pem_classic::app::chat_manager::ChatManager;
use p2pem_classic::types::{Config, Contact, ToastLevel, TransferStatus, TrustState};
use std::time::{Duration, Instant};
use uuid::Uuid;

fn sample_contact(name: &str) -> Contact {
    Contact {
        id: Uuid::new_v4(),
        name: name.to_string(),
        address: Some("127.0.0.1:12345".to_string()),
        addresses: Vec::new(),
        relay_server: None,
        relay_token: None,
        fingerprint: Some("AA".repeat(32)),
        public_key: None,
        created_at: chrono::Utc::now(),
        trust_state: TrustState::Unverified,
        notes: String::new(),
        tags: Vec::new(),
        last_seen: None,
    }
}

#[test]
fn rename_missing_chat_is_an_error() {
    let mut mgr = ChatManager::default();
    assert!(mgr.rename_chat(Uuid::new_v4(), "Nope".to_string()).is_err());
}

#[test]
fn delete_chat_removes_it() {
    let mut mgr = ChatManager::default();
    let id = Uuid::new_v4();
    mgr.create_local_chat_for_test(id, "Temp".to_string());
    assert!(mgr.get_chat(id).is_some());
    mgr.delete_chat(id);
    assert!(mgr.get_chat(id).is_none());
}

#[test]
fn clear_history_in_memory_wipes_all_state() {
    let mut mgr = ChatManager::default();
    mgr.create_local_chat_for_test(Uuid::new_v4(), "A".to_string());
    mgr.import_contact(sample_contact("Carol")).unwrap();
    mgr.add_toast(ToastLevel::Info, "hi".to_string());

    // Empty path + no key => in-memory clear only (no file written).
    mgr.clear_history(std::path::Path::new(""));
    assert!(mgr.chats.is_empty());
    assert!(mgr.contacts.is_empty());
    assert!(mgr.toasts.is_empty());
}

#[test]
fn toast_lifecycle_add_and_expire() {
    let mut mgr = ChatManager::default();
    mgr.add_toast(ToastLevel::Success, "fresh".to_string());
    assert_eq!(mgr.toasts.len(), 1);

    // A fresh toast survives cleanup.
    mgr.cleanup_expired_toasts();
    assert_eq!(mgr.toasts.len(), 1);

    // Age the toast past its display duration; cleanup should drop it.
    if let Some(old) = Instant::now().checked_sub(Duration::from_secs(60)) {
        mgr.toasts[0].created_at = old;
        mgr.cleanup_expired_toasts();
        assert!(mgr.toasts.is_empty(), "expired toast should be removed");
    }
}

#[test]
fn incoming_file_transfer_state_tracks_progress() {
    let mut mgr = ChatManager::default();
    // Auto-accept keeps the transfer on the frictionless Pending → InProgress
    // path; the acceptance gate (default) is covered by the chat_manager tests.
    mgr.config.auto_accept_files = true;
    let chat = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat, "Files".to_string());

    let transfer = mgr
        .start_receiving_file(chat, "doc.pdf", 1000)
        .expect("start transfer");

    let snap = mgr.active_transfers_snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].status, TransferStatus::Pending);
    assert_eq!(snap[0].received, 0);

    mgr.update_transfer_progress(transfer, 500);
    let snap = mgr.active_transfers_snapshot();
    assert_eq!(snap[0].received, 500);
    assert_eq!(snap[0].status, TransferStatus::InProgress);
}

#[test]
fn start_receiving_oversized_file_is_rejected() {
    let mut mgr = ChatManager::default();
    let chat = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat, "Files".to_string());
    let too_big = p2pem_classic::MAX_FILE_SIZE + 1;
    assert!(mgr.start_receiving_file(chat, "huge.bin", too_big).is_err());
}

#[test]
fn typing_indicators_without_session_error() {
    let config = Config {
        enable_typing_indicators: true,
        ..Config::default()
    };
    let mut mgr = ChatManager::new(config);
    let chat = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat, "Solo".to_string());
    // No session attached -> Session not found.
    assert!(mgr.send_typing_start(chat).is_err());
    assert!(mgr.send_typing_stop(chat).is_err());
}

#[test]
fn typing_indicators_disabled_is_a_noop_ok() {
    let config = Config {
        enable_typing_indicators: false,
        ..Config::default()
    };
    let mut mgr = ChatManager::new(config);
    let chat = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat, "Solo".to_string());
    // Disabled => returns Ok without needing a session.
    assert!(mgr.send_typing_start(chat).is_ok());
    assert!(mgr.send_typing_stop(chat).is_ok());
}

#[test]
fn contact_import_get_associate_remove() {
    let mut mgr = ChatManager::default();
    let id = mgr.import_contact(sample_contact("Dave")).unwrap();
    assert_eq!(mgr.get_contact(id).unwrap().name, "Dave");

    let chat = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat, "Dave".to_string());
    mgr.associate_contact_with_chat(id, chat);
    assert_eq!(mgr.contact_to_chat.get(&id).copied(), Some(chat));
    assert!(mgr.get_chat(chat).unwrap().participants.contains(&id));

    mgr.remove_contact(id);
    assert!(mgr.get_contact(id).is_none());
    assert!(!mgr.contact_to_chat.contains_key(&id));
}

#[test]
fn connection_state_helpers() {
    let mut mgr = ChatManager::default();
    let chat = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat, "X".to_string());
    assert_eq!(mgr.sessions_len(), 0);
    assert!(!mgr.is_connected(&chat));
    assert!(mgr.chat_ids().contains(&chat));
}

#[test]
fn generate_invite_qr_produces_png() {
    let mgr = ChatManager::default();
    let png = mgr
        .generate_invite_qr("chat-p2p://invite/v2/abcdef")
        .expect("qr generation");
    // PNG magic number.
    assert_eq!(
        &png[..8],
        &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n']
    );
}

#[test]
fn connection_password_setter_ignores_empty() {
    let mut mgr = ChatManager::default();
    assert!(!mgr.has_connection_password());
    mgr.set_connection_password(Some(String::new()));
    assert!(
        !mgr.has_connection_password(),
        "empty password must be treated as none"
    );
    mgr.set_connection_password(Some("hunter2".to_string()));
    assert!(mgr.has_connection_password());
    mgr.set_connection_password(None);
    assert!(!mgr.has_connection_password());
}

#[test]
fn conversation_lock_pauses_rehost() {
    let mut mgr = ChatManager::default();
    mgr.is_hosting = true;
    // A *direct* listener: `hosting_port` records the port `start_host` bound,
    // and is what tells auto-rehost there is a TCP listener to rebuild.
    mgr.hosting_port = Some(12345);
    // Hosting with no live placeholder => a rehost would normally be needed.
    assert!(mgr.check_rehost_needed());
    mgr.set_conversation_locked(true);
    assert!(mgr.is_conversation_locked());
    assert!(
        !mgr.check_rehost_needed(),
        "a locked conversation must not auto-rehost"
    );
    mgr.set_conversation_locked(false);
    assert!(mgr.check_rehost_needed());
}

/// Relay hosting must never be "resumed" as a direct TCP listener: the token
/// the user shared would be dead while they became reachable by a route they
/// never chose. `start_host_via_relay` leaves `hosting_port` unset, and that is
/// what marks the difference.
#[test]
fn relay_hosting_is_not_auto_rehosted_as_a_direct_listener() {
    let mut mgr = ChatManager::default();
    mgr.is_hosting = true;
    mgr.hosting_port = None;
    assert!(
        !mgr.check_rehost_needed(),
        "a relay rendezvous must not be replaced by a TCP listener"
    );
}
