use p2pem_classic::app::chat_manager::{ChatManager, SessionHandle};
use p2pem_classic::core::ProtocolMessage;
use p2pem_classic::types::Config;
use tokio::sync::mpsc;
use uuid::Uuid;

#[test]
fn test_send_message_without_session_returns_ok_and_toasts() {
    let mut manager = ChatManager::new(Config::default());
    let chat_id = Uuid::new_v4();
    manager.create_local_chat_for_test(chat_id, "Test".to_string());

    let initial_toasts = manager.toasts.len();

    // No session added, so it should warn and return Ok() (silently skipping send)
    let result = manager.send_message(chat_id, "Hello".to_string());

    assert!(result.is_ok());
    assert_eq!(manager.toasts.len(), initial_toasts + 1);
    assert!(manager
        .toasts
        .last()
        .unwrap()
        .message
        .contains("Connecting"));

    // Message should NOT be in history because session was missing
    let chat = manager.chats.get(&chat_id).unwrap();
    assert!(chat.messages.is_empty());
}

#[test]
fn test_message_sequence_ordering() {
    let mut manager = ChatManager::new(Config::default());
    let chat_id = Uuid::new_v4();
    manager.create_local_chat_for_test(chat_id, "Test".to_string());

    // Add a mock session and KEEP RECEIVER ALIVE
    let (tx, _rx) = mpsc::unbounded_channel();
    manager.add_session_for_test(chat_id, SessionHandle { from_app_tx: tx });

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
    manager.add_session_for_test(chat_id, SessionHandle { from_app_tx: tx });

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
