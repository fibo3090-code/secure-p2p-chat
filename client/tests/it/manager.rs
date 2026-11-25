//! `ChatManager` behaviour reached through its public API with a captured
//! session lane instead of a socket: what reaches the wire, what is refused,
//! and what the user is told. Anything that needs a real peer is in
//! `file_transfer` / `reconnect`; anything that needs crate internals is in the
//! `chat_manager` unit tests.

use messenger_core::core::ProtocolMessage;
use messenger_core::types::{Config, Contact, ToastLevel, TransferStatus};
use p2pem_classic::app::chat_manager::{ChatManager, SessionHandle};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use uuid::Uuid;

/// A conversation with a session whose outbound control lane the test holds.
fn chat_with_lane(mgr: &mut ChatManager) -> (Uuid, mpsc::UnboundedReceiver<ProtocolMessage>) {
    let chat = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat, "Peer".into());
    let (tx, rx) = mpsc::unbounded_channel();
    mgr.add_session_for_test(chat, SessionHandle::for_test_control(tx));
    (chat, rx)
}

fn drain(rx: &mut mpsc::UnboundedReceiver<ProtocolMessage>) -> Vec<ProtocolMessage> {
    std::iter::from_fn(|| rx.try_recv().ok()).collect()
}

/// A send that cannot be delivered is an `Err` that says why — never `Ok` plus
/// a toast, because a front-end reads `Ok` as "sent", clears the composer and
/// destroys the text. "Not connected yet" and "the peer went away" are
/// different situations and must read differently.
#[test]
fn an_undeliverable_send_is_an_error_that_says_why() {
    let mut mgr = ChatManager::new(Config::default());
    let chat = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat, "Test".into());

    let err = mgr.send_message(chat, "Hello".into()).unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("not connected"),
        "got: {err}"
    );

    // Established once (a fingerprint was verified), then dropped.
    mgr.chats.get_mut(&chat).unwrap().peer_fingerprint = Some("a".repeat(64));
    let err = mgr.send_message(chat, "Hello".into()).unwrap_err();
    assert!(err.to_string().contains("disconnected"), "got: {err}");

    assert!(
        mgr.chats[&chat].messages.is_empty(),
        "an undelivered message was recorded as sent"
    );
}

/// A message over one frame's worth is split into `TextChunk`s that
/// reassemble to exactly the original, each within the per-chunk cap — and
/// the sender's own history still shows one message.
#[test]
fn a_long_message_is_chunked_on_the_wire_and_reassembles_exactly() {
    let mut mgr = ChatManager::new(Config::default());
    let (chat, mut rx) = chat_with_lane(&mut mgr);
    // Multi-byte characters, so a split on a byte boundary would corrupt it.
    let text = "abc123é😀".repeat(20_000);
    assert!(text.len() > messenger_core::MAX_TEXT_MESSAGE_BYTES);
    mgr.send_message(chat, text.clone()).unwrap();

    let frames = drain(&mut rx);
    assert!(frames.len() > 1, "a long message must be chunked");
    let mut parts = Vec::new();
    let mut ids = std::collections::HashSet::new();
    for (i, frame) in frames.into_iter().enumerate() {
        let ProtocolMessage::TextChunk {
            message_id,
            chunk_index,
            total_chunks,
            text_part,
            ..
        } = frame
        else {
            panic!("expected only TextChunk frames, got {frame:?}");
        };
        assert_eq!(chunk_index as usize, i, "chunks go out in order");
        assert!(text_part.len() <= messenger_core::TEXT_CHUNK_BYTES);
        ids.insert((message_id, total_chunks));
        parts.push(text_part);
    }
    assert_eq!(ids.len(), 1, "every chunk names the same message and total");
    let (_, total) = ids.into_iter().next().unwrap();
    assert_eq!(total as usize, parts.len());
    assert_eq!(
        parts.concat(),
        text,
        "the chunks reassemble to the original"
    );
    assert_eq!(mgr.chats[&chat].messages.len(), 1);
}

/// Typing indicators: off means nothing on the wire and no error; on with no
/// session is an error; on with a session sends start then stop.
#[test]
fn typing_indicators_follow_the_setting() {
    let mut mgr = ChatManager::new(Config {
        enable_typing_indicators: false,
        ..Config::default()
    });
    let (chat, mut rx) = chat_with_lane(&mut mgr);
    mgr.send_typing_start(chat).unwrap();
    mgr.send_typing_stop(chat).unwrap();
    assert!(
        drain(&mut rx).is_empty(),
        "disabled indicators reached the wire"
    );

    mgr.config.enable_typing_indicators = true;
    let offline = Uuid::new_v4();
    mgr.create_local_chat_for_test(offline, "Offline".into());
    assert!(mgr.send_typing_start(offline).is_err());

    mgr.send_typing_start(chat).unwrap();
    mgr.send_typing_stop(chat).unwrap();
    let frames = drain(&mut rx);
    assert!(
        matches!(
            frames.as_slice(),
            [
                ProtocolMessage::TypingStart { .. },
                ProtocolMessage::TypingStop { .. }
            ]
        ),
        "got {frames:?}"
    );
}

/// `send_file` refuses, with an error that names the problem, anything it
/// cannot actually stream: no session, a directory, a path that is not there.
#[tokio::test]
async fn send_file_refuses_what_it_cannot_stream() {
    let mut mgr = ChatManager::new(Config::default());
    let offline = Uuid::new_v4();
    mgr.create_local_chat_for_test(offline, "Offline".into());
    let file = tempfile::NamedTempFile::new().unwrap();
    let err = mgr
        .send_file(offline, file.path().to_path_buf())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Session not found"), "got: {err}");

    let (chat, mut rx) = chat_with_lane(&mut mgr);
    let dir = tempfile::tempdir().unwrap();
    let err = mgr
        .send_file(chat, dir.path().to_path_buf())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not a regular file"), "got: {err}");

    let missing = dir.path().join("missing.txt");
    let err = mgr.send_file(chat, missing).await.unwrap_err();
    assert!(err.to_string().contains("file not found"), "got: {err}");

    assert!(drain(&mut rx).is_empty(), "a refused send reached the wire");
    assert!(mgr.active_transfers_snapshot().is_empty());
}

/// With auto-accept on, an incoming file moves Pending → InProgress as bytes
/// arrive; an offer over `MAX_FILE_SIZE` is refused before anything is held.
#[test]
fn incoming_transfers_track_progress_and_refuse_oversized_offers() {
    let mut mgr = ChatManager::default();
    mgr.config.auto_accept_files = true;
    let chat = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat, "Files".into());

    assert!(mgr
        .start_receiving_file(chat, "huge.bin", p2pem_classic::MAX_FILE_SIZE + 1)
        .is_err());
    assert!(mgr.active_transfers_snapshot().is_empty());

    let transfer = mgr.start_receiving_file(chat, "doc.pdf", 1000).unwrap();
    let snap = mgr.active_transfers_snapshot();
    assert_eq!(
        (snap[0].status.clone(), snap[0].received),
        (TransferStatus::Pending, 0)
    );
    mgr.update_transfer_progress(transfer, 500);
    let snap = mgr.active_transfers_snapshot();
    assert_eq!(
        (snap[0].status.clone(), snap[0].received),
        (TransferStatus::InProgress, 500)
    );
}

/// Success toasts expire; clearing history wipes chats, contacts and toasts.
#[test]
fn toasts_expire_and_clearing_history_wipes_the_in_memory_state() {
    let mut mgr = ChatManager::default();
    mgr.add_toast(ToastLevel::Success, "fresh".into());
    mgr.cleanup_expired_toasts();
    assert_eq!(mgr.toasts.len(), 1, "a fresh toast survives cleanup");
    let aged = Instant::now()
        .checked_sub(Duration::from_secs(60))
        .expect("the clock is past a minute");
    mgr.toasts[0].created_at = aged;
    mgr.cleanup_expired_toasts();
    assert!(mgr.toasts.is_empty(), "an expired toast was kept");

    mgr.create_local_chat_for_test(Uuid::new_v4(), "A".into());
    mgr.add_contact("Carol".into(), None, Some("ab".repeat(32)), None);
    mgr.add_toast(ToastLevel::Info, "hi".into());
    // An empty path with no key: an in-memory clear, nothing written.
    mgr.clear_history(std::path::Path::new(""));
    assert!(mgr.chats.is_empty() && mgr.contacts.is_empty() && mgr.toasts.is_empty());
}

/// An empty connection password means none, not "the empty string".
#[test]
fn an_empty_connection_password_is_no_password() {
    let mut mgr = ChatManager::default();
    assert!(!mgr.has_connection_password());
    mgr.set_connection_password(Some(String::new()));
    assert!(!mgr.has_connection_password());
    mgr.set_connection_password(Some("hunter2".into()));
    assert!(mgr.has_connection_password());
    mgr.set_connection_password(None);
    assert!(!mgr.has_connection_password());
}

/// Auto-rehost rebuilds a *direct* listener only: a locked conversation
/// pauses it, and a relay rendezvous (no `hosting_port`) is never silently
/// replaced by a TCP listener the user never chose.
#[test]
fn auto_rehost_is_for_direct_listeners_and_pauses_while_locked() {
    let mut mgr = ChatManager::default();
    mgr.is_hosting = true;
    mgr.hosting_port = Some(12345);
    assert!(mgr.check_rehost_needed());
    mgr.set_conversation_locked(true);
    assert!(mgr.is_conversation_locked());
    assert!(!mgr.check_rehost_needed(), "a locked conversation rehosted");
    mgr.set_conversation_locked(false);
    assert!(mgr.check_rehost_needed());

    mgr.hosting_port = None;
    assert!(
        !mgr.check_rehost_needed(),
        "a relay rendezvous was replaced by a TCP listener"
    );
}

/// The invite QR is a decodable PNG at a scannable size, and a link too long
/// to fit in a QR code is an error rather than a truncated code.
#[test]
fn the_invite_qr_is_a_scannable_png_or_an_error() {
    let mgr = ChatManager::default();
    let png = mgr
        .generate_invite_qr("chat-p2p://invite/v2/abcdef")
        .expect("qr");
    let img = image::load_from_memory_with_format(&png, image::ImageFormat::Png).expect("png");
    assert!(
        img.width() >= 200 && img.height() >= 200,
        "too small to scan"
    );
    assert!(mgr.generate_invite_qr(&"A".repeat(8000)).is_err());
}

/// Whatever is pasted into the invite box is refused with an error — not
/// accepted as a contact, and not a panic.
#[test]
fn malformed_invite_links_are_refused() {
    let mgr = ChatManager::default();
    for link in [
        "",
        "chat-p2p://invite/",
        "chat-p2p://invite/!!!not-base64!!!",
        "chat-p2p://invite/e30",    // {}
        "chat-p2p://invite/bnVsbA", // null
        "chat-p2p://invite/W10",    // []
        "http://example.com/",
        "javascript:alert(1)",
        &format!("chat-p2p://invite/{}", "A".repeat(100_000)),
    ] {
        let parsed: Result<Contact, _> = mgr.parse_invite_link(link);
        assert!(parsed.is_err(), "{:.40}… was accepted", link);
    }
}
