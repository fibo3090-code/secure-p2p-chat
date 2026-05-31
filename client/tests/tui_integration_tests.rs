use egui_tracing::tracing::EventCollector;
use encodeur_rsa_rust::app::chat_manager::SessionHandle;
use encodeur_rsa_rust::core::ProtocolMessage;
use encodeur_rsa_rust::tui::app::TuiApp;
use tokio::sync::mpsc;
use uuid::Uuid;

#[test]
fn test_message_roundtrip_integration() {
    let mut app = TuiApp::new(EventCollector::new()).unwrap();
    let chat_id = Uuid::new_v4();
    app.chat_manager
        .create_local_chat_for_test(chat_id, "Integration Test".to_string());

    // Setup dummy session
    let (tx, mut rx) = mpsc::unbounded_channel();
    app.chat_manager
        .add_session_for_test(chat_id, SessionHandle { from_app_tx: tx });

    app.chat_ids = vec![chat_id];
    app.chat_list_state.select(Some(0));

    // Simulate UI typing and sending
    app.input_field.set_text("Integrated message");
    app.send_message();

    // 1. Verify it cleared input
    assert_eq!(app.input_field.text(), "");

    // 2. Verify it's in history
    let chat = app.chat_manager.chats.get(&chat_id).unwrap();
    assert_eq!(chat.messages.len(), 1);

    // 3. Verify it was actually sent to the 'network' (the channel)
    let msg = rx
        .try_recv()
        .expect("Message should have been sent to channel");
    match msg {
        ProtocolMessage::Text { text, .. } => assert_eq!(text, "Integrated message"),
        _ => panic!("Wrong protocol message type sent"),
    }
}

#[test]
fn test_multiple_chats_isolation() {
    let mut app = TuiApp::new(EventCollector::new()).unwrap();

    let chat_id_1 = Uuid::new_v4();
    let chat_id_2 = Uuid::new_v4();

    app.chat_manager
        .create_local_chat_for_test(chat_id_1, "Chat 1".to_string());
    app.chat_manager
        .create_local_chat_for_test(chat_id_2, "Chat 2".to_string());

    let (tx1, mut rx1) = mpsc::unbounded_channel();
    let (tx2, mut rx2) = mpsc::unbounded_channel();

    app.chat_manager
        .add_session_for_test(chat_id_1, SessionHandle { from_app_tx: tx1 });
    app.chat_manager
        .add_session_for_test(chat_id_2, SessionHandle { from_app_tx: tx2 });

    app.chat_ids = vec![chat_id_1, chat_id_2];

    // Send to Chat 1
    app.chat_list_state.select(Some(0));
    app.input_field.set_text("To Chat 1");
    app.send_message();

    // Send to Chat 2
    app.next_chat(); // Switch to Chat 2 (index 1)
    app.input_field.set_text("To Chat 2");
    app.send_message();

    // Verify isolation in history
    assert_eq!(
        app.chat_manager
            .chats
            .get(&chat_id_1)
            .unwrap()
            .messages
            .len(),
        1
    );
    assert_eq!(
        app.chat_manager
            .chats
            .get(&chat_id_2)
            .unwrap()
            .messages
            .len(),
        1
    );

    assert!(rx1.try_recv().is_ok());
    assert!(rx1.try_recv().is_err()); // Only one message

    assert!(rx2.try_recv().is_ok());
    assert!(rx2.try_recv().is_err()); // Only one message
}

#[test]
fn test_chat_switching_preserves_input_state() {
    let mut app = TuiApp::new(EventCollector::new()).unwrap();
    app.chat_ids = vec![Uuid::new_v4(), Uuid::new_v4()];
    app.chat_list_state.select(Some(0));

    app.input_field.set_text("Draft for chat 0");

    // Switch to next chat
    app.next_chat();
    assert_eq!(app.chat_list_state.selected(), Some(1));

    // The TUI shares a single input buffer across chats (input_field is a field
    // of TuiApp, not Chat), so switching chats preserves the in-progress draft.
    assert_eq!(app.input_field.text(), "Draft for chat 0");

    // If the user expects per-chat drafts, we might want to change this,
    // but the test currently reflects the actual behavior.
}

#[test]
fn test_typing_indicator_flow_integration() {
    let mut app = TuiApp::new(EventCollector::new()).unwrap();
    let chat_id = Uuid::new_v4();
    app.chat_manager
        .create_local_chat_for_test(chat_id, "Typing Test".to_string());

    let (tx, mut rx) = mpsc::unbounded_channel();
    app.chat_manager
        .add_session_for_test(chat_id, SessionHandle { from_app_tx: tx });

    app.chat_ids = vec![chat_id];
    app.chat_list_state.select(Some(0));

    // Enable typing indicators in config
    app.chat_manager.config.enable_typing_indicators = true;

    // Trigger typing start via manager (app doesn't have a helper for this yet,
    // it's usually triggered in the event loop when keys are pressed)
    app.chat_manager.send_typing_start(chat_id).unwrap();

    let msg = rx.try_recv().expect("TypingStart should be sent");
    match msg {
        ProtocolMessage::TypingStart { .. } => {}
        _ => panic!("Expected TypingStart"),
    }

    app.chat_manager.send_typing_stop(chat_id).unwrap();
    let msg2 = rx.try_recv().expect("TypingStop should be sent");
    match msg2 {
        ProtocolMessage::TypingStop { .. } => {}
        _ => panic!("Expected TypingStop"),
    }
}
