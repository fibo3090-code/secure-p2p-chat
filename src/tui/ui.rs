use crate::tui::app::TuiApp;
use crate::types::MessageContent;
use ratatui::{prelude::*, widgets::*};

pub fn ui(f: &mut Frame, app: &mut TuiApp) {
    let size = f.size();

    // MAIN LAYOUT: Header (1), Content (Min 1), Footer (1)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Length(1), // Header/Status
                Constraint::Min(1),    // Main content
                Constraint::Length(1), // Footer/Help
            ]
            .as_ref(),
        )
        .split(size);

    // 1. HEADER / STATUS BAR
    let port_str = if app.chat_manager.is_hosting {
        format!("🟢 Hosting :{}", app.chat_manager.config.listen_port)
    } else {
        "🔴 Not Hosting".to_string()
    };

    let identity_str = format!("👤 {}", app.identity_name);

    let header = Line::from(vec![
        Span::styled(
            " P2P CHAT ",
            Style::default()
                .bg(Color::Cyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" | "),
        Span::styled(identity_str, Style::default().fg(Color::Yellow)),
        Span::raw(" | "),
        Span::styled(port_str, Style::default().fg(Color::Green)),
    ]);
    f.render_widget(Paragraph::new(header).alignment(Alignment::Left), chunks[0]);

    // 2. MAIN CONTENT: Split into sidebar and chat view
    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(25), Constraint::Percentage(75)].as_ref())
        .split(chunks[1]);

    // Sidebar - Chat List
    let chat_items: Vec<ListItem> = app
        .chat_ids
        .iter()
        .map(|id| {
            let chat = app.chat_manager.chats.get(id).unwrap();
            let title = if chat.is_host_placeholder {
                format!("{} (Host)", chat.title)
            } else {
                chat.title.clone()
            };
            ListItem::new(vec![Line::from(vec![
                Span::styled("• ", Style::default().fg(Color::Cyan)),
                Span::raw(title),
            ])])
        })
        .collect();

    let chat_list = List::new(chat_items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Conversations "),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Indexed(236)) // Dark gray background for selection
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    f.render_stateful_widget(chat_list, body_chunks[0], &mut app.chat_list_state);

    // Chat View: Messages (Min 1) + Input (3)
    let chat_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(3)].as_ref())
        .split(body_chunks[1]);

    // Messages View
    if let Some(selected_index) = app.chat_list_state.selected() {
        if let Some(chat_id) = app.chat_ids.get(selected_index) {
            if let Some(chat) = app.chat_manager.chats.get(chat_id) {
                let messages: Vec<Line> = chat
                    .messages
                    .iter()
                    .map(|msg| {
                        let timestamp = msg.timestamp.format("%H:%M ").to_string();
                        let (name, name_color, content_color) = if msg.from_me {
                            ("Me", Color::Cyan, Color::White)
                        } else {
                            ("Peer", Color::Green, Color::Indexed(250)) // Light gray
                        };

                        let content = match &msg.content {
                            MessageContent::Text { text } => text.clone(),
                            MessageContent::File { filename, .. } => {
                                format!("📁 [File: {}]", filename)
                            }
                            MessageContent::Edited { .. } => "✎ [Edited]".to_string(),
                        };

                        Line::from(vec![
                            Span::styled(timestamp, Style::default().fg(Color::Indexed(240))), // Gray
                            Span::styled(
                                format!("{}: ", name),
                                Style::default().fg(name_color).add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(content, Style::default().fg(content_color)),
                        ])
                    })
                    .collect();

                let chat_block = Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan))
                    .title(format!(" {} ", chat.title));

                let paragraph = Paragraph::new(messages)
                    .block(chat_block)
                    .scroll((app.message_scroll, 0))
                    .wrap(Wrap { trim: true });

                f.render_widget(paragraph, chat_chunks[0]);
            }
        }
    } else {
        f.render_widget(
            Paragraph::new("Select a conversation from the sidebar to start chatting.")
                .block(Block::default().borders(Borders::ALL).title(" Messages "))
                .alignment(Alignment::Center),
            chat_chunks[0],
        );
    }

    // Input Box
    let input_block = Block::default()
        .borders(Borders::ALL)
        .title(" Message (Enter to send) ");

    let input_paragraph = Paragraph::new(app.input_text.as_str())
        .block(input_block)
        .style(Style::default().fg(Color::White));

    f.render_widget(input_paragraph, chat_chunks[1]);

    // 3. FOOTER / HELP BAR
    let help_text = vec![
        Span::styled(" Esc ", Style::default().bg(Color::Indexed(238))),
        Span::raw(" Quit | "),
        Span::styled(" ↑/↓ ", Style::default().bg(Color::Indexed(238))),
        Span::raw(" Select Chat | "),
        Span::styled(" PgUp/PgDn ", Style::default().bg(Color::Indexed(238))),
        Span::raw(" Scroll | "),
        Span::styled(" Enter ", Style::default().bg(Color::Indexed(238))),
        Span::raw(" Send "),
    ];
    let footer = Paragraph::new(Line::from(help_text)).alignment(Alignment::Center);
    f.render_widget(footer, chunks[2]);
}
