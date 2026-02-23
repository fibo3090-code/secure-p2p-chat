use crate::tui::app::{TuiApp, TuiFocus, TuiMode};
use crate::types::MessageContent;
use ratatui::{prelude::*, widgets::*};

pub fn ui(f: &mut Frame, app: &mut TuiApp) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)].as_ref())
        .split(f.area());

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)].as_ref())
        .split(vertical[0]);

    // Chat list
    let chat_items: Vec<ListItem> = app
        .chat_ids
        .iter()
        .map(|id| {
            let (prefix, title) = app
                .chat_manager
                .chats
                .get(id)
                .map(|chat| {
                    let connected = app.chat_manager.is_connected(&chat.id);
                    let marker = if chat.is_host_placeholder {
                        "H"
                    } else if connected {
                        "●"
                    } else {
                        "○"
                    };
                    (marker.to_string(), chat.title.as_str().to_string())
                })
                .unwrap_or_else(|| ("?".to_string(), "[Unavailable chat]".to_string()));
            ListItem::new(format!("{} {}", prefix, title))
        })
        .collect();

    let chat_list_title = match app.focus {
        TuiFocus::ChatList => "Chats [focus]",
        _ => "Chats",
    };

    let chat_list = List::new(chat_items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(chat_list_title),
        )
        .highlight_style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .bg(Color::DarkGray),
        )
        .highlight_symbol("> ");

    f.render_stateful_widget(chat_list, columns[0], &mut app.chat_list_state);

    // Main content area (messages + input/command)
    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(5)].as_ref())
        .split(columns[1]);

    // Messages view
    let messages_view = if let Some(selected_index) = app.chat_list_state.selected() {
        if let Some(chat_id) = app.chat_ids.get(selected_index) {
            if let Some(chat) = app.chat_manager.chats.get(chat_id) {
                let messages: Vec<Line> = chat
                    .messages
                    .iter()
                    .map(|msg| {
                        let timestamp = msg.timestamp.format("%H:%M ").to_string();
                        let (prefix, color) = if msg.from_me {
                            ("You: ", Color::Cyan)
                        } else {
                            ("Peer: ", Color::Green)
                        };
                        let content = match &msg.content {
                            MessageContent::Text { text } => text.clone(),
                            MessageContent::File { filename, .. } => {
                                format!("[File: {}]", filename)
                            }
                            MessageContent::Edited { .. } => "[Message Edited]".to_string(),
                        };
                        Line::from(vec![
                            Span::styled(timestamp, Style::default().fg(Color::DarkGray)),
                            Span::styled(
                                prefix,
                                Style::default().fg(color).add_modifier(Modifier::BOLD),
                            ),
                            Span::raw(content),
                        ])
                    })
                    .collect();

                let title = if app.chat_manager.is_connected(chat_id) {
                    format!("{} [connected]", chat.title)
                } else {
                    format!("{} [offline]", chat.title)
                };

                Paragraph::new(messages)
                    .block(Block::default().borders(Borders::ALL).title(title))
                    .scroll((app.message_scroll, 0))
                    .wrap(Wrap { trim: false })
            } else {
                Paragraph::new("Selected chat is unavailable")
                    .block(Block::default().borders(Borders::ALL).title("Messages"))
            }
        } else {
            Paragraph::new("No chat available")
                .block(Block::default().borders(Borders::ALL).title("Messages"))
        }
    } else {
        Paragraph::new("Select a chat")
            .block(Block::default().borders(Borders::ALL).title("Messages"))
    };

    f.render_widget(messages_view, right_chunks[0]);

    // Input/command box
    let (input_title, input_text) = if app.mode == TuiMode::Command {
        (
            "Command (:help)",
            if app.command_buffer.is_empty() {
                ":".to_string()
            } else {
                format!(":{}", app.command_buffer)
            },
        )
    } else {
        (
            if app.focus == TuiFocus::Input {
                "Input [focus] Enter=send Ctrl+J=newline"
            } else {
                "Input Enter=send Ctrl+J=newline"
            },
            app.input_text.clone(),
        )
    };

    let input_style = if app.mode == TuiMode::Command {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };

    let input_box = Paragraph::new(input_text)
        .style(input_style)
        .block(Block::default().borders(Borders::ALL).title(input_title))
        .wrap(Wrap { trim: false });
    f.render_widget(input_box, right_chunks[1]);

    // Status line
    let status = Paragraph::new(app.status_line.clone())
        .style(Style::default().fg(Color::DarkGray))
        .block(Block::default());
    f.render_widget(status, vertical[1]);
}

#[cfg(test)]
mod tests {
    use super::ui;
    use crate::tui::app::{TuiApp, TuiFocus};
    use egui_tracing::tracing::EventCollector;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use uuid::Uuid;

    fn draw_once(app: &mut TuiApp) {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|f| ui(f, app)).expect("draw");
    }

    #[test]
    fn render_does_not_panic_with_stale_chat_ids() {
        let mut app = TuiApp::new(EventCollector::new()).expect("app");
        let stale_id = Uuid::new_v4();
        app.chat_ids = vec![stale_id];
        app.chat_list_state.select(Some(0));

        draw_once(&mut app);
    }

    #[test]
    fn render_does_not_panic_with_mixed_valid_and_stale_chats() {
        let mut app = TuiApp::new(EventCollector::new()).expect("app");
        let valid_id = Uuid::new_v4();
        let stale_id = Uuid::new_v4();
        app.chat_manager
            .create_local_chat_for_test(valid_id, "Valid chat".to_string());
        app.chat_ids = vec![stale_id, valid_id];
        app.chat_list_state.select(Some(0));

        draw_once(&mut app);
    }

    #[test]
    fn render_does_not_panic_on_small_terminal_size() {
        let mut app = TuiApp::new(EventCollector::new()).expect("app");
        app.focus = TuiFocus::Input;

        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|f| ui(f, &mut app)).expect("draw");
    }
}
