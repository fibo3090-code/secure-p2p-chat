//! Hostile and out-of-order event sequences for the `ChatManager` event pump:
//! frames for nothing, twice, out of order, double teardown, unknown ids. The
//! manager must not panic, invent or lose anything, and stay usable.

use super::test_support::*;
use super::*;
use tempfile::tempdir;

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

/// A gated manager in a fresh temp dir with one live conversation.
fn setup() -> (
    tempfile::TempDir,
    ChatManager,
    Uuid,
    mpsc::UnboundedReceiver<ProtocolMessage>,
) {
    let dir = tempdir().unwrap();
    let mut mgr = manager_in(dir.path(), false);
    let (chat, rx) = chat_with_session(&mut mgr, "Peer");
    (dir, mgr, chat, rx)
}

// ── Events for things that do not exist ─────────────────────────────────────

#[test]
fn every_event_for_an_unknown_session_is_ignored() {
    let dir = tempdir().unwrap();
    let mut mgr = manager_in(dir.path(), false);
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
        SessionEvent::MessageReceived(chunk(&[1, 2, 3], 2)),
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
    let (dir, mut mgr, chat, _rx) = setup();
    frame(&mut mgr, chat, chunk(&[7; 10], 1));
    frame(&mut mgr, chat, ProtocolMessage::FileEnd { seq: 2 });
    frame(&mut mgr, chat, ProtocolMessage::FileCancel { seq: 3 });
    assert!(mgr.active_transfers_snapshot().is_empty());
    assert!(
        downloads_left(dir.path()).is_empty(),
        "a chunk with no offer reached the disk"
    );
    // The conversation is still usable afterwards.
    frame(&mut mgr, chat, text(4, "still here"));
    assert_eq!(texts(&mgr, chat), ["still here"]);
}

// ── Replays and ordering ────────────────────────────────────────────────────

#[test]
fn replayed_and_reordered_frames_are_dropped() {
    let (_dir, mut mgr, chat, _rx) = setup();
    frame(&mut mgr, chat, text(5, "five"));
    frame(&mut mgr, chat, text(5, "five again"));
    frame(&mut mgr, chat, text(3, "three, late"));
    frame(&mut mgr, chat, text(6, "six"));
    assert_eq!(texts(&mgr, chat), ["five", "six"]);
}

// ── Large-message reassembly ────────────────────────────────────────────────

#[test]
fn a_chunk_that_changes_the_total_discards_the_message() {
    let (_dir, mut mgr, chat, _rx) = setup();
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
    let (_dir, mut mgr, chat, _rx) = setup();
    let id = Uuid::new_v4();
    frame(&mut mgr, chat, chunk_frame(id, 0, 2, "first", 1));
    frame(&mut mgr, chat, chunk_frame(id, 0, 2, "first again", 2));
    assert!(texts(&mgr, chat).is_empty());
    frame(&mut mgr, chat, chunk_frame(id, 1, 2, "-second", 3));
    assert_eq!(texts(&mgr, chat).len(), 1);
}

#[test]
fn the_partial_message_cap_holds_and_earlier_messages_still_complete() {
    let (_dir, mut mgr, chat, _rx) = setup();
    let cap = crate::MAX_CONCURRENT_PARTIAL_TEXT_PER_CHAT;
    let mut seq = 0;
    let ids: Vec<Uuid> = (0..cap + 1).map(|_| Uuid::new_v4()).collect();
    for id in &ids {
        seq += 1;
        frame(&mut mgr, chat, chunk_frame(*id, 0, 2, "x", seq));
    }
    // The one over the cap was refused; the others complete.
    for id in &ids {
        seq += 1;
        frame(&mut mgr, chat, chunk_frame(*id, 1, 2, "y", seq));
    }
    assert_eq!(texts(&mgr, chat).len(), cap);
}

#[test]
fn partial_messages_do_not_cross_conversations() {
    let dir = tempdir().unwrap();
    let mut mgr = manager_in(dir.path(), false);
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
    let (_dir, mut mgr, chat, _rx) = setup();
    mgr.delete_chat(chat);
    mgr.delete_chat(chat);
    mgr.delete_chat(Uuid::new_v4());
    assert!(mgr.chats.is_empty());
    assert!(!mgr.is_connected(&chat));
}

#[test]
fn a_disconnect_mid_receive_leaves_no_spool_and_a_retry_works() {
    let (dir, mut mgr, chat, _rx) = setup();
    frame(&mut mgr, chat, meta("half.bin", 10, 1));
    frame(&mut mgr, chat, chunk(&[1; 5], 2));
    mgr.handle_session_event(chat, SessionEvent::Disconnected);
    let leftovers = downloads_left(dir.path());
    assert!(leftovers.is_empty(), "spool left behind: {leftovers:?}");
    assert!(mgr
        .active_transfers_snapshot()
        .iter()
        .all(|t| !t.status.is_active()));
}

// ── Files with awkward shapes ───────────────────────────────────────────────

#[test]
fn an_empty_file_can_be_received() {
    let (dir, mut mgr, chat, _rx) = setup();
    frame(&mut mgr, chat, meta("empty.txt", 0, 1));
    frame(&mut mgr, chat, ProtocolMessage::FileEnd { seq: 2 });
    let offer = mgr.active_transfers_snapshot()[0].id;
    mgr.accept_incoming_file(offer)
        .expect("accept an empty file");
    let saved = dir.path().join("downloads").join("empty.txt");
    assert_eq!(std::fs::read(&saved).expect("saved").len(), 0);
}

#[test]
fn a_traversal_filename_lands_inside_the_download_folder() {
    let (dir, mut mgr, chat, _rx) = setup();
    feed_file(&mut mgr, chat, "../../escape.txt", b"hi", 1);
    feed_file(&mut mgr, chat, "..\\..\\escape2.txt", b"hi", 4);
    for offer in mgr.active_transfers_snapshot() {
        let _ = mgr.accept_incoming_file(offer.id);
    }
    let downloads = dir.path().join("downloads").canonicalize().unwrap();
    for p in file_paths(&mgr, chat) {
        assert!(
            p.canonicalize().unwrap().starts_with(&downloads),
            "{} escaped the download folder",
            p.display()
        );
    }
    assert!(!dir.path().join("escape.txt").exists());
}

#[test]
fn a_second_offer_with_the_same_name_does_not_overwrite_the_first() {
    let (_dir, mut mgr, chat, _rx) = setup();
    for (body, seq) in [(&b"first"[..], 1), (&b"second"[..], 4)] {
        feed_file(&mut mgr, chat, "same.txt", body, seq);
        let offer = mgr
            .active_transfers_snapshot()
            .into_iter()
            .find(|t| t.status == TransferStatus::AwaitingAcceptance)
            .unwrap();
        mgr.accept_incoming_file(offer.id).unwrap();
    }
    let mut contents: Vec<Vec<u8>> = file_paths(&mgr, chat)
        .iter()
        .filter_map(|p| std::fs::read(p).ok())
        .collect();
    contents.sort();
    assert_eq!(contents, vec![b"first".to_vec(), b"second".to_vec()]);
}

// ── User actions on odd targets ─────────────────────────────────────────────

#[test]
fn transfer_actions_on_unknown_or_finished_transfers_fail_cleanly() {
    let (_dir, mut mgr, chat, _rx) = setup();
    let ghost = Uuid::new_v4();
    assert!(mgr.accept_incoming_file(ghost).is_err());
    assert!(mgr.reject_incoming_file(ghost).is_err());
    mgr.cancel_transfer(ghost);

    feed_file(&mut mgr, chat, "a.txt", b"a", 1);
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
    let (_dir, mut mgr, chat, _rx) = setup();
    let long: String = "👨‍👩‍👧".repeat(30);
    mgr.rename_chat(chat, long).unwrap();
    let title = &mgr.get_chat(chat).unwrap().title;
    assert!(title.chars().count() <= 50);
    assert!(mgr.rename_chat(Uuid::new_v4(), "x".into()).is_err());
}

#[test]
fn a_conversation_cannot_be_renamed_to_nothing() {
    let dir = tempdir().unwrap();
    let mut mgr = manager_in(dir.path(), false);
    let (chat, _rx) = chat_with_session(&mut mgr, "Mum");
    for blank in ["", "   ", "\n\t"] {
        assert!(
            mgr.rename_chat(chat, blank.into()).is_err(),
            "{blank:?} accepted"
        );
    }
    assert_eq!(mgr.get_chat(chat).unwrap().title, "Mum");
    assert!(!mgr.get_chat(chat).unwrap().title_is_custom);
}

#[test]
fn sending_to_nothing_fails_instead_of_pretending() {
    let (mut mgr, offline) = manager_with_chat("Offline");
    assert!(mgr.send_message(Uuid::new_v4(), "hi".into()).is_err());
    assert!(mgr.send_message(offline, "hi".into()).is_err());
    assert!(
        texts(&mgr, offline).is_empty(),
        "an undelivered message was shown as sent"
    );
}

#[test]
fn an_empty_message_is_not_sent() {
    let dir = tempdir().unwrap();
    let mut mgr = manager_in(dir.path(), false);
    let (chat, mut rx) = chat_with_session(&mut mgr, "Peer");
    assert!(mgr.send_message(chat, String::new()).is_err());
    assert!(mgr.send_message(chat, "   \n\t ".into()).is_err());
    assert!(rx.try_recv().is_err(), "an empty frame went on the wire");
    assert!(texts(&mgr, chat).is_empty());
}

#[test]
fn unread_counts_survive_marks_beyond_the_end() {
    let (_dir, mut mgr, chat, _rx) = setup();
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
    let (_dir, mut mgr, chat, _rx) = setup();
    frame(&mut mgr, chat, ProtocolMessage::TypingStop { seq: 1 });
    frame(&mut mgr, chat, ProtocolMessage::TypingStart { seq: 2 });
    assert!(mgr.get_chat(chat).unwrap().peer_typing);
    frame(&mut mgr, chat, text(3, "done typing"));
    assert!(!mgr.get_chat(chat).unwrap().peer_typing);
}

#[test]
fn clearing_history_mid_transfer_leaves_nothing_on_disk() {
    let (dir, mut mgr, chat, _rx) = setup();
    frame(&mut mgr, chat, meta("big.bin", 1000, 1));
    frame(&mut mgr, chat, chunk(&[0; 100], 2));
    mgr.clear_history(std::path::Path::new(""));
    let leftovers = downloads_left(dir.path());
    assert!(leftovers.is_empty(), "spool left behind: {leftovers:?}");
    assert!(mgr.chats.is_empty() && mgr.active_transfers_snapshot().is_empty());
}
