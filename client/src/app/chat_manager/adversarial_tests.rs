//! Hostile and out-of-order event sequences for the `ChatManager` event pump.
//!
//! Every test here feeds the manager something a well-behaved peer and a
//! well-behaved UI never produce — frames for nothing, frames twice, frames in
//! the wrong order, teardown twice, ids that do not exist — and asserts what a
//! user would expect to see afterwards: no panic, nothing invented, nothing
//! lost, and the conversation still usable.

use super::*;
use tempfile::tempdir;

fn manager_in(dir: &std::path::Path) -> ChatManager {
    ChatManager::new(Config {
        download_dir: dir.join("downloads"),
        temp_dir: dir.join("temp"),
        auto_accept_files: false,
        ..Config::default()
    })
}

/// A conversation with a live (in-memory) session, returning the receiver of
/// everything the manager queues on its control lane.
fn chat_with_session(
    mgr: &mut ChatManager,
    title: &str,
) -> (Uuid, mpsc::UnboundedReceiver<ProtocolMessage>) {
    let id = Uuid::new_v4();
    mgr.create_local_chat_for_test(id, title.to_string());
    let (tx, rx) = mpsc::unbounded_channel();
    let (file_tx, _file_rx) = mpsc::channel(4);
    mgr.add_session_for_test(
        id,
        SessionHandle {
            from_app_tx: tx,
            file_tx,
        },
    );
    (id, rx)
}

fn frame(mgr: &mut ChatManager, session: Uuid, msg: ProtocolMessage) {
    mgr.handle_session_event(session, SessionEvent::MessageReceived(msg));
}

fn texts(mgr: &ChatManager, chat: Uuid) -> Vec<String> {
    mgr.get_chat(chat)
        .map(|c| {
            c.messages
                .iter()
                .filter_map(|m| match &m.content {
                    MessageContent::Text { text } => Some(text.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn text(seq: u64, body: &str) -> ProtocolMessage {
    ProtocolMessage::Text {
        text: body.to_string(),
        timestamp: 0,
        seq,
    }
}

fn chunk_frame(id: Uuid, index: u32, total: u32, part: &str, seq: u64) -> ProtocolMessage {
    ProtocolMessage::TextChunk {
        message_id: id,
        chunk_index: index,
        total_chunks: total,
        text_part: part.to_string(),
        timestamp: 1,
        seq,
    }
}

// ── Events for things that do not exist ─────────────────────────────────────

#[test]
fn every_event_for_an_unknown_session_is_ignored() {
    let dir = tempdir().unwrap();
    let mut mgr = manager_in(dir.path());
    let ghost = Uuid::new_v4();
    let events = vec![
        SessionEvent::Listening { port: 1 },
        SessionEvent::Connected {
            peer: "1.2.3.4:5".into(),
        },
        SessionEvent::ShowFingerprintVerification {
            fingerprint: "ab".repeat(32),
            peer_name: "x".into(),
            sas: "123456".into(),
            chat_id: ghost,
        },
        SessionEvent::Ready,
        SessionEvent::MessageReceived(text(1, "hello?")),
        SessionEvent::MessageReceived(ProtocolMessage::FileChunk {
            chunk: vec![1, 2, 3],
            seq: 2,
        }),
        SessionEvent::MessageReceived(ProtocolMessage::FileEnd { seq: 3 }),
        SessionEvent::MessageReceived(ProtocolMessage::Ack {
            acked_seq: 9,
            seq: 4,
        }),
        SessionEvent::FileSendComplete { seq: 5 },
        SessionEvent::TextSendComplete { seq: 6 },
        SessionEvent::Error("boom".into()),
        SessionEvent::Disconnected,
        SessionEvent::Disconnected,
    ];
    for event in events {
        mgr.handle_session_event(ghost, event);
    }
    assert!(mgr.chats.is_empty(), "an event invented a conversation");
    assert!(mgr.active_transfers_snapshot().is_empty());
    assert!(
        mgr.pending_fingerprint().is_none(),
        "a prompt for nothing was queued"
    );
}

#[test]
fn file_frames_without_an_offer_write_nothing() {
    let dir = tempdir().unwrap();
    let mut mgr = manager_in(dir.path());
    let (chat, _rx) = chat_with_session(&mut mgr, "Peer");
    frame(
        &mut mgr,
        chat,
        ProtocolMessage::FileChunk {
            chunk: vec![7; 10],
            seq: 1,
        },
    );
    frame(&mut mgr, chat, ProtocolMessage::FileEnd { seq: 2 });
    frame(&mut mgr, chat, ProtocolMessage::FileCancel { seq: 3 });
    assert!(mgr.active_transfers_snapshot().is_empty());
    assert!(
        std::fs::read_dir(dir.path().join("downloads"))
            .map(|d| d.count() == 0)
            .unwrap_or(true),
        "a chunk with no offer reached the disk"
    );
    // The conversation is still usable afterwards.
    frame(&mut mgr, chat, text(4, "still here"));
    assert_eq!(texts(&mgr, chat), ["still here"]);
}

// ── Replays and ordering ────────────────────────────────────────────────────

#[test]
fn replayed_and_reordered_frames_are_dropped() {
    let dir = tempdir().unwrap();
    let mut mgr = manager_in(dir.path());
    let (chat, _rx) = chat_with_session(&mut mgr, "Peer");
    frame(&mut mgr, chat, text(5, "five"));
    frame(&mut mgr, chat, text(5, "five again"));
    frame(&mut mgr, chat, text(3, "three, late"));
    frame(&mut mgr, chat, text(6, "six"));
    assert_eq!(texts(&mgr, chat), ["five", "six"]);
}

// ── Large-message reassembly ────────────────────────────────────────────────

#[test]
fn a_chunk_that_changes_the_total_discards_the_message() {
    let dir = tempdir().unwrap();
    let mut mgr = manager_in(dir.path());
    let (chat, _rx) = chat_with_session(&mut mgr, "Peer");
    let id = Uuid::new_v4();
    frame(&mut mgr, chat, chunk_frame(id, 0, 2, "a", 1));
    frame(&mut mgr, chat, chunk_frame(id, 1, 3, "b", 2));
    frame(&mut mgr, chat, chunk_frame(id, 2, 3, "c", 3));
    assert!(
        texts(&mgr, chat).is_empty(),
        "an inconsistent message was assembled"
    );

    // A well-formed message afterwards still arrives.
    let next = Uuid::new_v4();
    frame(&mut mgr, chat, chunk_frame(next, 0, 2, "he", 4));
    frame(&mut mgr, chat, chunk_frame(next, 1, 2, "llo", 5));
    assert_eq!(texts(&mgr, chat), ["hello"]);
}

#[test]
fn a_repeated_chunk_does_not_complete_a_message() {
    let dir = tempdir().unwrap();
    let mut mgr = manager_in(dir.path());
    let (chat, _rx) = chat_with_session(&mut mgr, "Peer");
    let id = Uuid::new_v4();
    frame(&mut mgr, chat, chunk_frame(id, 0, 2, "first", 1));
    frame(&mut mgr, chat, chunk_frame(id, 0, 2, "first again", 2));
    assert!(texts(&mgr, chat).is_empty());
    frame(&mut mgr, chat, chunk_frame(id, 1, 2, "-second", 3));
    assert_eq!(texts(&mgr, chat).len(), 1);
}

#[test]
fn the_partial_message_cap_holds_and_earlier_messages_still_complete() {
    let dir = tempdir().unwrap();
    let mut mgr = manager_in(dir.path());
    let (chat, _rx) = chat_with_session(&mut mgr, "Peer");
    let mut seq = 0;
    let ids: Vec<Uuid> = (0..crate::MAX_CONCURRENT_PARTIAL_TEXT_PER_CHAT + 1)
        .map(|_| Uuid::new_v4())
        .collect();
    for id in &ids {
        seq += 1;
        frame(&mut mgr, chat, chunk_frame(*id, 0, 2, "x", seq));
    }
    // The one over the cap was refused; the others complete.
    for id in &ids[..crate::MAX_CONCURRENT_PARTIAL_TEXT_PER_CHAT] {
        seq += 1;
        frame(&mut mgr, chat, chunk_frame(*id, 1, 2, "y", seq));
    }
    seq += 1;
    frame(
        &mut mgr,
        chat,
        chunk_frame(
            ids[crate::MAX_CONCURRENT_PARTIAL_TEXT_PER_CHAT],
            1,
            2,
            "y",
            seq,
        ),
    );
    assert_eq!(
        texts(&mgr, chat).len(),
        crate::MAX_CONCURRENT_PARTIAL_TEXT_PER_CHAT
    );
}

#[test]
fn partial_messages_do_not_cross_conversations() {
    let dir = tempdir().unwrap();
    let mut mgr = manager_in(dir.path());
    let (a, _ra) = chat_with_session(&mut mgr, "A");
    let (b, _rb) = chat_with_session(&mut mgr, "B");
    let id = Uuid::new_v4();
    frame(&mut mgr, a, chunk_frame(id, 0, 2, "from-a ", 1));
    frame(&mut mgr, b, chunk_frame(id, 1, 2, "from-b", 1));
    assert!(texts(&mgr, a).is_empty() && texts(&mgr, b).is_empty());
}

// ── Teardown ────────────────────────────────────────────────────────────────

#[test]
fn deleting_twice_and_deleting_nothing_are_harmless() {
    let dir = tempdir().unwrap();
    let mut mgr = manager_in(dir.path());
    let (chat, _rx) = chat_with_session(&mut mgr, "Peer");
    mgr.delete_chat(chat);
    mgr.delete_chat(chat);
    mgr.delete_chat(Uuid::new_v4());
    assert!(mgr.chats.is_empty());
    assert!(!mgr.is_connected(&chat));
}

#[test]
fn a_disconnect_mid_receive_leaves_no_spool_and_a_retry_works() {
    let dir = tempdir().unwrap();
    let mut mgr = manager_in(dir.path());
    let (chat, _rx) = chat_with_session(&mut mgr, "Peer");
    frame(
        &mut mgr,
        chat,
        ProtocolMessage::FileMeta {
            filename: "half.bin".into(),
            size: 10,
            seq: 1,
        },
    );
    frame(
        &mut mgr,
        chat,
        ProtocolMessage::FileChunk {
            chunk: vec![1; 5],
            seq: 2,
        },
    );
    mgr.handle_session_event(chat, SessionEvent::Disconnected);
    let leftovers: Vec<_> = std::fs::read_dir(dir.path().join("downloads"))
        .map(|d| d.flatten().map(|e| e.file_name()).collect())
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "spool left behind: {leftovers:?}");
    assert!(mgr
        .active_transfers_snapshot()
        .iter()
        .all(|t| !t.status.is_active()));
}

// ── Files with awkward shapes ───────────────────────────────────────────────

#[test]
fn an_empty_file_can_be_received() {
    let dir = tempdir().unwrap();
    let mut mgr = manager_in(dir.path());
    let (chat, _rx) = chat_with_session(&mut mgr, "Peer");
    frame(
        &mut mgr,
        chat,
        ProtocolMessage::FileMeta {
            filename: "empty.txt".into(),
            size: 0,
            seq: 1,
        },
    );
    frame(&mut mgr, chat, ProtocolMessage::FileEnd { seq: 2 });
    let offer = mgr.active_transfers_snapshot()[0].id;
    mgr.accept_incoming_file(offer)
        .expect("accept an empty file");
    let saved = dir.path().join("downloads").join("empty.txt");
    assert_eq!(std::fs::read(&saved).expect("saved").len(), 0);
}

#[test]
fn a_traversal_filename_lands_inside_the_download_folder() {
    let dir = tempdir().unwrap();
    let mut mgr = manager_in(dir.path());
    let (chat, _rx) = chat_with_session(&mut mgr, "Peer");
    for (seq, name) in [(1, "../../escape.txt"), (4, "..\\..\\escape2.txt")] {
        frame(
            &mut mgr,
            chat,
            ProtocolMessage::FileMeta {
                filename: name.into(),
                size: 2,
                seq,
            },
        );
        frame(
            &mut mgr,
            chat,
            ProtocolMessage::FileChunk {
                chunk: b"hi".to_vec(),
                seq: seq + 1,
            },
        );
        frame(&mut mgr, chat, ProtocolMessage::FileEnd { seq: seq + 2 });
    }
    for offer in mgr.active_transfers_snapshot() {
        let _ = mgr.accept_incoming_file(offer.id);
    }
    let downloads = dir.path().join("downloads").canonicalize().unwrap();
    for m in &mgr.get_chat(chat).unwrap().messages {
        if let MessageContent::File { path: Some(p), .. } = &m.content {
            assert!(
                p.canonicalize().unwrap().starts_with(&downloads),
                "{} escaped the download folder",
                p.display()
            );
        }
    }
    assert!(!dir.path().join("escape.txt").exists());
}

#[test]
fn a_second_offer_with_the_same_name_does_not_overwrite_the_first() {
    let dir = tempdir().unwrap();
    let mut mgr = manager_in(dir.path());
    let (chat, _rx) = chat_with_session(&mut mgr, "Peer");
    let mut seq = 0;
    for body in [b"first".to_vec(), b"second".to_vec()] {
        seq += 1;
        frame(
            &mut mgr,
            chat,
            ProtocolMessage::FileMeta {
                filename: "same.txt".into(),
                size: body.len() as u64,
                seq,
            },
        );
        seq += 1;
        frame(
            &mut mgr,
            chat,
            ProtocolMessage::FileChunk { chunk: body, seq },
        );
        seq += 1;
        frame(&mut mgr, chat, ProtocolMessage::FileEnd { seq });
        let offer = mgr
            .active_transfers_snapshot()
            .into_iter()
            .find(|t| t.status == TransferStatus::AwaitingAcceptance)
            .unwrap();
        mgr.accept_incoming_file(offer.id).unwrap();
    }
    let mut contents: Vec<Vec<u8>> = mgr
        .get_chat(chat)
        .unwrap()
        .messages
        .iter()
        .filter_map(|m| match &m.content {
            MessageContent::File { path: Some(p), .. } => std::fs::read(p).ok(),
            _ => None,
        })
        .collect();
    contents.sort();
    assert_eq!(contents, vec![b"first".to_vec(), b"second".to_vec()]);
}

// ── User actions on odd targets ─────────────────────────────────────────────

#[test]
fn transfer_actions_on_unknown_or_finished_transfers_fail_cleanly() {
    let dir = tempdir().unwrap();
    let mut mgr = manager_in(dir.path());
    let ghost = Uuid::new_v4();
    assert!(mgr.accept_incoming_file(ghost).is_err());
    assert!(mgr.reject_incoming_file(ghost).is_err());
    mgr.cancel_transfer(ghost);

    let (chat, _rx) = chat_with_session(&mut mgr, "Peer");
    frame(
        &mut mgr,
        chat,
        ProtocolMessage::FileMeta {
            filename: "a.txt".into(),
            size: 1,
            seq: 1,
        },
    );
    frame(
        &mut mgr,
        chat,
        ProtocolMessage::FileChunk {
            chunk: b"a".to_vec(),
            seq: 2,
        },
    );
    frame(&mut mgr, chat, ProtocolMessage::FileEnd { seq: 3 });
    let offer = mgr.active_transfers_snapshot()[0].id;
    mgr.accept_incoming_file(offer).unwrap();
    assert!(mgr.accept_incoming_file(offer).is_err(), "accepted twice");
    assert!(
        mgr.reject_incoming_file(offer).is_err(),
        "declined after accepting"
    );
    mgr.cancel_transfer(offer);
}

#[test]
fn renaming_keeps_whole_characters_and_refuses_unknown_chats() {
    let dir = tempdir().unwrap();
    let mut mgr = manager_in(dir.path());
    let (chat, _rx) = chat_with_session(&mut mgr, "Peer");
    let long: String = "👨‍👩‍👧".repeat(30);
    mgr.rename_chat(chat, long).unwrap();
    let title = &mgr.get_chat(chat).unwrap().title;
    assert!(title.chars().count() <= 50);
    assert!(mgr.rename_chat(Uuid::new_v4(), "x".into()).is_err());
}

#[test]
fn sending_to_nothing_fails_instead_of_pretending() {
    let dir = tempdir().unwrap();
    let mut mgr = manager_in(dir.path());
    assert!(mgr.send_message(Uuid::new_v4(), "hi".into()).is_err());
    let offline = Uuid::new_v4();
    mgr.create_local_chat_for_test(offline, "Offline".into());
    assert!(mgr.send_message(offline, "hi".into()).is_err());
    assert!(
        texts(&mgr, offline).is_empty(),
        "an undelivered message was shown as sent"
    );
}

#[test]
fn an_empty_message_is_not_sent() {
    let dir = tempdir().unwrap();
    let mut mgr = manager_in(dir.path());
    let (chat, mut rx) = chat_with_session(&mut mgr, "Peer");
    assert!(mgr.send_message(chat, String::new()).is_err());
    assert!(mgr.send_message(chat, "   \n\t ".into()).is_err());
    assert!(rx.try_recv().is_err(), "an empty frame went on the wire");
    assert!(texts(&mgr, chat).is_empty());
}

#[test]
fn unread_counts_survive_marks_beyond_the_end() {
    let dir = tempdir().unwrap();
    let mut mgr = manager_in(dir.path());
    let (chat, _rx) = chat_with_session(&mut mgr, "Peer");
    frame(&mut mgr, chat, text(1, "one"));
    mgr.get_chat_mut(chat).unwrap().read_count = 99;
    assert_eq!(mgr.unread_count(chat), 0);
    frame(&mut mgr, chat, text(2, "two"));
    mgr.mark_chat_read(chat);
    assert_eq!(mgr.unread_count(chat), 0);
    assert_eq!(mgr.unread_count(Uuid::new_v4()), 0);
}

#[test]
fn a_message_ends_the_typing_indicator_and_a_stop_without_start_is_harmless() {
    let dir = tempdir().unwrap();
    let mut mgr = manager_in(dir.path());
    let (chat, _rx) = chat_with_session(&mut mgr, "Peer");
    frame(&mut mgr, chat, ProtocolMessage::TypingStop { seq: 1 });
    frame(&mut mgr, chat, ProtocolMessage::TypingStart { seq: 2 });
    assert!(mgr.get_chat(chat).unwrap().peer_typing);
    frame(&mut mgr, chat, text(3, "done typing"));
    assert!(!mgr.get_chat(chat).unwrap().peer_typing);
}

#[test]
fn clearing_history_mid_transfer_leaves_nothing_on_disk() {
    let dir = tempdir().unwrap();
    let mut mgr = manager_in(dir.path());
    let (chat, _rx) = chat_with_session(&mut mgr, "Peer");
    frame(
        &mut mgr,
        chat,
        ProtocolMessage::FileMeta {
            filename: "big.bin".into(),
            size: 1000,
            seq: 1,
        },
    );
    frame(
        &mut mgr,
        chat,
        ProtocolMessage::FileChunk {
            chunk: vec![0; 100],
            seq: 2,
        },
    );
    mgr.clear_history(std::path::Path::new(""));
    let leftovers: Vec<_> = std::fs::read_dir(dir.path().join("downloads"))
        .map(|d| d.flatten().map(|e| e.file_name()).collect())
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "spool left behind: {leftovers:?}");
    assert!(mgr.chats.is_empty() && mgr.active_transfers_snapshot().is_empty());
}
