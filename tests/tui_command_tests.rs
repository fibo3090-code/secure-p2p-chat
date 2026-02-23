use ratatui_crossterm::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use egui_tracing::tracing::EventCollector;
use encodeur_rsa_rust::tui::app::{TuiApp, TuiCommand, TuiFocus, TuiMode};
use uuid::Uuid;

#[test]
fn parse_commands_cover_core_paths() {
    assert_eq!(TuiApp::parse_command(":help").unwrap(), TuiCommand::Help);
    assert_eq!(TuiApp::parse_command(":quit").unwrap(), TuiCommand::Quit);
    assert_eq!(
        TuiApp::parse_command(":host 9090").unwrap(),
        TuiCommand::Host(Some(9090))
    );
    assert_eq!(
        TuiApp::parse_command(":connect 127.0.0.1:9001").unwrap(),
        TuiCommand::Connect {
            host: "127.0.0.1".to_string(),
            port: 9001,
        }
    );
}

#[test]
fn parse_command_errors_are_clear() {
    assert!(TuiApp::parse_command(":").is_err());
    assert!(TuiApp::parse_command(":rename").is_err());
    assert!(TuiApp::parse_command(":connect").is_err());
    assert!(TuiApp::parse_command(":connect :abc").is_err());
}

#[test]
fn key_flow_enters_and_submits_command_mode() {
    let mut app = TuiApp::new(EventCollector::new()).unwrap();
    app.focus = TuiFocus::ChatList;

    app.handle_key_event(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));
    assert_eq!(app.mode, TuiMode::Command);

    app.handle_key_event(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(app.mode, TuiMode::Normal);
    assert_eq!(app.take_pending_command(), Some(TuiCommand::Help));
}

#[test]
fn focus_cycles_with_tab() {
    let mut app = TuiApp::new(EventCollector::new()).unwrap();
    app.focus = TuiFocus::ChatList;

    app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.focus, TuiFocus::MessageView);
    app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.focus, TuiFocus::Input);
    app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.focus, TuiFocus::ChatList);
}

#[test]
fn sync_chat_ids_preserves_selection_when_possible() {
    let mut app = TuiApp::new(EventCollector::new()).unwrap();
    let id1 = Uuid::new_v4();
    let id2 = Uuid::new_v4();
    app.chat_manager
        .create_local_chat_for_test(id1, "A".to_string());
    app.chat_manager
        .create_local_chat_for_test(id2, "B".to_string());

    app.sync_chat_ids();
    let selected = app.chat_ids.first().copied().unwrap();
    app.chat_list_state.select(Some(0));

    app.sync_chat_ids();
    assert_eq!(app.selected_chat_id(), Some(selected));
}
