//! The terminal client driven by keystrokes, the way a user drives it, against
//! a captured session lane. Command *parsing* is pinned beside the parser in
//! `tui/command.rs`; this is what the app does with the keys.

use messenger_core::core::ProtocolMessage;
use p2pem_classic::app::chat_manager::SessionHandle;
use p2pem_classic::logbuf::LogBuffer;
use p2pem_classic::tui::app::{TuiApp, TuiCommand, TuiFocus, TuiMode};
use ratatui_crossterm::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;
use uuid::Uuid;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn type_str(app: &mut TuiApp, text: &str) {
    for c in text.chars() {
        app.handle_key_event(key(KeyCode::Char(c)));
    }
}

/// An app with `n` conversations, each with a session lane the test holds.
fn app_with_chats(
    n: usize,
) -> (
    TuiApp,
    Vec<(Uuid, mpsc::UnboundedReceiver<ProtocolMessage>)>,
) {
    let mut app = TuiApp::new(LogBuffer::new()).unwrap();
    // Isolate send behaviour from the typing-indicator frames keys produce.
    app.chat_manager.config.enable_typing_indicators = false;
    let mut chats = Vec::new();
    for i in 0..n {
        let id = Uuid::new_v4();
        app.chat_manager
            .create_local_chat_for_test(id, format!("Chat {i}"));
        let (tx, rx) = mpsc::unbounded_channel();
        app.chat_manager
            .add_session_for_test(id, SessionHandle::for_test_control(tx));
        chats.push((id, rx));
    }
    app.sync_chat_ids();
    // Order the receivers the way the list shows them.
    let order = app.chat_ids.clone();
    chats.sort_by_key(|(id, _)| order.iter().position(|o| o == id));
    (app, chats)
}

fn sent_texts(rx: &mut mpsc::UnboundedReceiver<ProtocolMessage>) -> Vec<String> {
    std::iter::from_fn(|| rx.try_recv().ok())
        .map(|m| match m {
            ProtocolMessage::Text { text, .. } => text,
            other => panic!("expected only text frames, got {other:?}"),
        })
        .collect()
}

/// Ctrl+J inserts a newline, Enter sends: the multi-line text reaches the
/// selected conversation's wire, its history, and the input is cleared.
#[test]
fn typing_a_multiline_message_and_pressing_enter_sends_it() {
    let (mut app, mut chats) = app_with_chats(1);
    app.chat_list_state.select(Some(0));
    app.focus = TuiFocus::Input;

    type_str(&mut app, "Hi");
    app.handle_key_event(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
    type_str(&mut app, "!");
    assert_eq!(app.input_field.text(), "Hi\n!");
    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(app.input_field.text(), "");
    let (id, rx) = &mut chats[0];
    assert_eq!(sent_texts(rx), ["Hi\n!"]);
    assert_eq!(app.chat_manager.chats[id].messages.len(), 1);
}

/// A message goes to the conversation that is selected, and only to it.
#[test]
fn a_message_goes_only_to_the_selected_conversation() {
    let (mut app, mut chats) = app_with_chats(2);
    app.chat_list_state.select(Some(0));
    app.input_field.set_text("To the first");
    app.send_message();
    app.next_chat();
    app.input_field.set_text("To the second");
    app.send_message();

    assert_eq!(sent_texts(&mut chats[0].1), ["To the first"]);
    assert_eq!(sent_texts(&mut chats[1].1), ["To the second"]);
}

/// `:` opens command mode from the chat list; typing and Enter submits the
/// parsed command and returns to normal mode.
#[test]
fn a_typed_command_is_parsed_and_submitted() {
    let mut app = TuiApp::new(LogBuffer::new()).unwrap();
    app.focus = TuiFocus::ChatList;
    app.handle_key_event(key(KeyCode::Char(':')));
    assert_eq!(app.mode, TuiMode::Command);
    type_str(&mut app, "help");
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(app.mode, TuiMode::Normal);
    assert_eq!(app.take_pending_command(), Some(TuiCommand::Help(None)));
}

/// Tab cycles focus list → messages → input → list.
#[test]
fn tab_cycles_focus() {
    let mut app = TuiApp::new(LogBuffer::new()).unwrap();
    app.focus = TuiFocus::ChatList;
    for expected in [TuiFocus::MessageView, TuiFocus::Input, TuiFocus::ChatList] {
        app.handle_key_event(key(KeyCode::Tab));
        assert_eq!(app.focus, expected);
    }
}

/// Re-syncing the conversation list keeps the selection on the same
/// conversation, and `:rename` renames the selected one.
#[tokio::test]
async fn the_selection_survives_a_resync_and_rename_acts_on_it() {
    let (mut app, chats) = app_with_chats(2);
    app.chat_list_state.select(Some(1));
    let selected = chats[1].0;
    app.sync_chat_ids();
    assert_eq!(app.selected_chat_id(), Some(selected));

    app.execute_command(TuiCommand::Rename("New Title".into()))
        .await;
    assert_eq!(
        app.chat_manager.get_chat(selected).unwrap().title,
        "New Title"
    );
    assert_ne!(
        app.chat_manager.get_chat(chats[0].0).unwrap().title,
        "New Title"
    );
}
