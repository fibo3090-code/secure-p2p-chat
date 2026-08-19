use p2pem_classic::app::chat_manager::{ChatManager, SessionHandle};
use p2pem_classic::core::ProtocolMessage;
use p2pem_classic::types::Config;
use tokio::sync::mpsc;
use uuid::Uuid;

/// Sending with no session must FAIL, not succeed-and-toast.
///
/// A front-end reads `Ok(())` as "sent": it clears the composer and adds no
/// message row, so the text is destroyed and a four-second toast is the only
/// trace. Returning an error is what lets every caller keep the draft.
#[test]
fn test_send_message_without_session_errors_instead_of_dropping_text() {
    let mut manager = ChatManager::new(Config::default());
    let chat_id = Uuid::new_v4();
    manager.create_local_chat_for_test(chat_id, "Test".to_string());

    let result = manager.send_message(chat_id, "Hello".to_string());

    let err = result.expect_err("a send with no session must not report success");
    assert!(
        err.to_string().to_lowercase().contains("not connected"),
        "the error must say why nothing was sent, got: {err}"
    );

    // Nothing was queued, so nothing may be recorded as sent either.
    let chat = manager.chats.get(&chat_id).unwrap();
    assert!(chat.messages.is_empty());
}

/// The same, for a conversation that was established and then dropped: the
/// error must say the peer is gone rather than "still connecting".
#[test]
fn test_send_message_to_disconnected_peer_reports_the_disconnect() {
    let mut manager = ChatManager::new(Config::default());
    let chat_id = Uuid::new_v4();
    manager.create_local_chat_for_test(chat_id, "Test".to_string());
    manager.chats.get_mut(&chat_id).unwrap().peer_fingerprint = Some("a".repeat(64));

    let err = manager
        .send_message(chat_id, "Hello".to_string())
        .expect_err("a send to a dropped peer must not report success");
    assert!(err.to_string().contains("disconnected"), "got: {err}");
    assert!(manager.chats.get(&chat_id).unwrap().messages.is_empty());
}

#[test]
fn test_message_sequence_ordering() {
    let mut manager = ChatManager::new(Config::default());
    let chat_id = Uuid::new_v4();
    manager.create_local_chat_for_test(chat_id, "Test".to_string());

    // Add a mock session and KEEP RECEIVER ALIVE
    let (tx, _rx) = mpsc::unbounded_channel();
    manager.add_session_for_test(chat_id, SessionHandle::for_test_control(tx));

    manager.send_message(chat_id, "One".to_string()).unwrap();
    manager.send_message(chat_id, "Two".to_string()).unwrap();

    let chat = manager.chats.get(&chat_id).unwrap();
    assert_eq!(chat.send_seq, 2);
    assert_eq!(chat.messages.len(), 2);
}

#[test]
fn test_large_message_is_chunked_for_transport() {
    let mut manager = ChatManager::new(Config::default());
    let chat_id = Uuid::new_v4();
    manager.create_local_chat_for_test(chat_id, "Chunk Test".to_string());

    let (tx, mut rx) = mpsc::unbounded_channel();
    manager.add_session_for_test(chat_id, SessionHandle::for_test_control(tx));

    let large_text = "abc123".repeat(20_000);
    manager.send_message(chat_id, large_text.clone()).unwrap();

    let mut message_count = 0usize;
    let mut chunk_count = 0usize;
    while let Ok(msg) = rx.try_recv() {
        message_count += 1;
        match msg {
            ProtocolMessage::TextChunk {
                chunk_index,
                total_chunks,
                text_part,
                ..
            } => {
                assert!((chunk_index as usize) < total_chunks as usize);
                assert!(text_part.len() < large_text.len());
                chunk_count += 1;
            }
            other => panic!("expected TextChunk, got {:?}", other),
        }
    }

    assert!(
        message_count > 1,
        "large text should be split into multiple transport messages"
    );
    assert_eq!(message_count, chunk_count);
    let chat = manager.chats.get(&chat_id).unwrap();
    assert_eq!(
        chat.messages.len(),
        1,
        "large text should stay one local message"
    );
}
