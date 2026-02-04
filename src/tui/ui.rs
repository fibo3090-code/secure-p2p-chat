use crate::tui::app::TuiApp;
use crate::types::MessageContent;
use ratatui::{prelude::*, widgets::*};

pub fn ui(f: &mut Frame, app: &mut TuiApp) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)].as_ref())
        .split(f.size());

    // Chat list
    let chat_items: Vec<ListItem> = app
        .chat_ids
        .iter()
        .map(|id| {
            let chat = app.chat_manager.chats.get(id).unwrap();
            ListItem::new(chat.title.as_str())
        })
        .collect();

    let chat_list = List::new(chat_items)
        .block(Block::default().borders(Borders::ALL).title("Chats"))
        .highlight_style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .bg(Color::DarkGray),
        )
        .highlight_symbol("> ");

    f.render_stateful_widget(chat_list, chunks[0], &mut app.chat_list_state);

    // Main content area
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(3)].as_ref())
        .split(chunks[1]);

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
                Paragraph::new(messages)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(chat.title.clone()),
                    )
                    .scroll((app.message_scroll, 0))
                    .wrap(Wrap { trim: true })
            } else {
                Paragraph::new("No chat selected")
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

    f.render_widget(messages_view, main_chunks[0]);

    // Input box
    let input_box = Paragraph::new(app.input_text.as_str())
        .style(Style::default())
        .block(Block::default().borders(Borders::ALL).title("Input"));
    f.render_widget(input_box, main_chunks[1]);
}
