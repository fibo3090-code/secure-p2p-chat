use encodeur_rsa_rust::app::chat_manager::{ChatManager, SessionHandle};
use encodeur_rsa_rust::types::Config;
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
fn test_group_message_offline_participants_toasts() {
    let mut manager = ChatManager::new(Config::default());

    let cid1 = manager.add_contact("Alice".to_string(), None, None, None);
    let cid2 = manager.add_contact("Bob".to_string(), None, None, None);

    let chat_id = manager.create_group_chat(vec![cid1, cid2], Some("Group".to_string()));

    let initial_toasts = manager.toasts.len();

    // participants are offline (no sessions)
    let result = manager.send_group_message(chat_id, "Hi team".to_string());

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 0); // Sent to 0 participants
    assert_eq!(manager.toasts.len(), initial_toasts + 1);
    assert!(manager
        .toasts
        .last()
        .unwrap()
        .message
        .contains("all recipients are offline"));

    // Message should still be in local group history
    let chat = manager.chats.get(&chat_id).unwrap();
    assert_eq!(chat.messages.len(), 1);
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
