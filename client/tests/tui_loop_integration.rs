use egui_tracing::tracing::EventCollector;
use p2pem_classic::app::chat_manager::SessionHandle;
use p2pem_classic::core::ProtocolMessage;
use p2pem_classic::tui::app::{TuiApp, TuiCommand, TuiFocus};
use ratatui_crossterm::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;
use uuid::Uuid;

#[test]
fn input_flow_supports_multiline_and_send() {
    let mut app = TuiApp::new(EventCollector::new()).unwrap();
    let chat_id = Uuid::new_v4();
    app.chat_manager
        .create_local_chat_for_test(chat_id, "Integration".to_string());
    let (tx, mut rx) = mpsc::unbounded_channel();
    app.chat_manager
        .add_session_for_test(chat_id, SessionHandle::for_test_control(tx));

    app.sync_chat_ids();
    app.chat_list_state.select(Some(0));
    app.focus = TuiFocus::Input;
    // Isolate input/send behavior from the typing-indicator traffic.
    app.chat_manager.config.enable_typing_indicators = false;

    app.handle_key_event(KeyEvent::new(KeyCode::Char('H'), KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
    app.handle_key_event(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE));

    assert_eq!(app.input_field.text(), "Hi\n!");

    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(app.input_field.text(), "");
    let msg = rx.try_recv().expect("message sent to session");
    match msg {
        ProtocolMessage::Text { text, .. } => assert_eq!(text, "Hi\n!"),
        _ => panic!("expected text message"),
    }
}

#[tokio::test]
async fn quit_command_terminates_loop_flag() {
    let mut app = TuiApp::new(EventCollector::new()).unwrap();
    assert!(!app.should_quit);

    // :quit now asks for confirmation; the loop only exits after 'y'.
    app.execute_command(TuiCommand::Quit).await;
    assert!(!app.should_quit);
    app.handle_key_event(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
    assert!(app.should_quit);
}

#[tokio::test]
async fn rename_command_updates_selected_chat() {
    let mut app = TuiApp::new(EventCollector::new()).unwrap();
    let chat_id = Uuid::new_v4();
    app.chat_manager
        .create_local_chat_for_test(chat_id, "Old".to_string());

    app.sync_chat_ids();
    app.chat_list_state.select(Some(0));

    app.execute_command(TuiCommand::Rename("New Title".to_string()))
        .await;

    let renamed = app.chat_manager.get_chat(chat_id).unwrap();
    assert_eq!(renamed.title, "New Title");
}
