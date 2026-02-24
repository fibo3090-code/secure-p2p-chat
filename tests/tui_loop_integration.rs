use egui_tracing::tracing::EventCollector;
use encodeur_rsa_rust::app::chat_manager::SessionHandle;
use encodeur_rsa_rust::core::ProtocolMessage;
use encodeur_rsa_rust::tui::app::{TuiApp, TuiCommand, TuiFocus};
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
        .add_session_for_test(chat_id, SessionHandle { from_app_tx: tx });

    app.sync_chat_ids();
    app.chat_list_state.select(Some(0));
    app.focus = TuiFocus::Input;

    app.handle_key_event(KeyEvent::new(KeyCode::Char('H'), KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
    app.handle_key_event(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE));

    assert_eq!(app.input_text, "Hi\n!");

    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(app.input_text, "");
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

    app.execute_command(TuiCommand::Quit).await;

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
