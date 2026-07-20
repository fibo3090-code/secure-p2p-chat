//! TUI rendering.
//!
//! Composes the frame: chat list (with status markers + unread dots), the
//! message view (auto-scroll with clamping), the input / command box, a toast
//! stack, and the status bar — then draws any active overlay on top.

use crate::tui::app::{TuiApp, TuiFocus, TuiMode};
use crate::tui::command::COMMANDS;
use crate::tui::overlays::{self, toast_color, BRAND_ACCENT};
use crate::types::MessageContent;
use ratatui::{prelude::*, widgets::*};
use unicode_width::UnicodeWidthStr;

/// Layout regions used by both the renderer and the cursor placement in `mod.rs`.
pub struct Regions {
    pub chat_list: Rect,
    pub messages: Rect,
    pub input: Rect,
    pub status: Rect,
}

/// Compute the standard layout for a given full area. Single source of truth so
/// the cursor placement in `mod.rs` never drifts from the renderer.
pub fn regions(area: Rect) -> Regions {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(vertical[0]);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(5)])
        .split(columns[1]);

    Regions {
        chat_list: columns[0],
        messages: right[0],
        input: right[1],
        status: vertical[1],
    }
}

pub fn ui(f: &mut Frame, app: &mut TuiApp) {
    let r = regions(f.area());
    render_chat_list(f, app, r.chat_list);
    render_messages(f, app, r.messages);
    render_input(f, app, r.input);
    render_status(f, app, r.status);
    render_toasts(f, app, r.messages);
    if app.mode == TuiMode::Command && !app.overlay.is_open() && !app.command_suggestions.is_empty()
    {
        render_command_suggestions(f, app, r.input);
    }
    overlays::render_overlay(f, app, f.area());
}

/// Floating autocomplete menu shown above the command box while typing a `:`command.
fn render_command_suggestions(f: &mut Frame, app: &TuiApp, input_area: Rect) {
    let total = app.command_suggestions.len();
    if total == 0 {
        return;
    }
    let visible = total.min(8) as u16;
    let height = visible + 2; // + borders
    if input_area.y < height {
        return; // not enough room above the input box
    }
    let rect = Rect {
        x: input_area.x,
        y: input_area.y - height,
        width: input_area.width,
        height,
    };
    let inner_w = rect.width.saturating_sub(2) as usize;

    let items: Vec<ListItem> = app
        .command_suggestions
        .iter()
        .map(|&ci| {
            let (_, usage, desc) = COMMANDS[ci];
            let line = Line::from(vec![
                Span::styled(
                    format!("{:<20}", usage),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    truncate(desc, inner_w.saturating_sub(21)),
                    Style::default().fg(Color::Gray),
                ),
            ]);
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(" commands · Tab to complete "),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Yellow)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▌");

    let mut state = ListState::default();
    state.select(Some(app.suggestion_index.min(total - 1)));

    f.render_widget(Clear, rect);
    f.render_stateful_widget(list, rect, &mut state);
}

fn render_chat_list(f: &mut Frame, app: &mut TuiApp, area: Rect) {
    let items: Vec<ListItem> = app
        .chat_ids
        .iter()
        .map(|id| {
            let unread = app.is_unread(id);
            let (marker, title) = app
                .chat_manager
                .chats
                .get(id)
                .map(|chat| {
                    let marker = if chat.is_host_placeholder {
                        "H"
                    } else if app.chat_manager.is_connected(&chat.id) {
                        "●"
                    } else {
                        "○"
                    };
                    (marker, chat.title.clone())
                })
                .unwrap_or(("?", "[unavailable]".to_string()));
            let unread_dot = if unread { "*" } else { " " };
            let style = if unread {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::styled(
                format!("{}{} {}", unread_dot, marker, title),
                style,
            ))
        })
        .collect();

    let title = match app.focus {
        TuiFocus::ChatList => " Chats [focus] ",
        _ => " Chats ",
    };
    let border = if app.focus == TuiFocus::ChatList {
        BRAND_ACCENT
    } else {
        Color::DarkGray
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border))
                .title(title),
        )
        .highlight_style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .bg(Color::DarkGray),
        )
        .highlight_symbol("> ");

    f.render_stateful_widget(list, area, &mut app.chat_list_state);
}

fn render_messages(f: &mut Frame, app: &mut TuiApp, area: Rect) {
    let inner_w = area.width.saturating_sub(2);
    let inner_h = area.height.saturating_sub(2);
    app.msg_view_width = inner_w;
    app.msg_view_height = inner_h;

    let selected = app.selected_chat_id();
    let Some(chat_id) = selected else {
        let p = Paragraph::new("Select a chat (Tab to focus the list, ↑/↓ to choose)")
            .block(block(" Messages ", Color::DarkGray));
        f.render_widget(p, area);
        return;
    };
    let Some(chat) = app.chat_manager.chats.get(&chat_id) else {
        let p = Paragraph::new("Selected chat is unavailable")
            .block(block(" Messages ", Color::DarkGray));
        f.render_widget(p, area);
        return;
    };

    let mut lines: Vec<Line> = Vec::with_capacity(chat.messages.len());
    let mut plain_texts: Vec<String> = Vec::with_capacity(chat.messages.len());
    for msg in &chat.messages {
        let ts = msg.timestamp.format("%H:%M ").to_string();
        let (prefix, color) = if msg.from_me {
            ("You ", BRAND_ACCENT)
        } else {
            ("Peer ", Color::Green)
        };
        let mut content = match &msg.content {
            MessageContent::Text { text } => text.clone(),
            MessageContent::File { filename, size, .. } => {
                format!("📎 {} ({})", filename, crate::util::format_size(*size))
            }
        };
        // Delivery receipt: the peer acknowledged this sent message.
        if msg.from_me && msg.delivered {
            content.push_str(" ✓");
        }
        plain_texts.push(format!("{}{}{}", ts, prefix, content));
        lines.push(Line::from(vec![
            Span::styled(ts, Style::default().fg(Color::DarkGray)),
            Span::styled(
                prefix,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(content),
        ]));
    }

    let connected = app.chat_manager.is_connected(&chat_id);
    let mut title = if connected {
        format!(" {} [connected] ", chat.title)
    } else if chat.is_host_placeholder {
        format!(" {} [waiting for peer] ", chat.title)
    } else {
        format!(" {} [offline] ", chat.title)
    };
    if chat.peer_typing {
        title.push_str("· typing… ");
    }
    for t in app.chat_manager.active_transfers_snapshot() {
        if t.chat_id == chat_id && t.size > 0 {
            let pct = (t.received.saturating_mul(100) / t.size).min(100);
            title.push_str(&format!("· 📎 {} {}% ", t.filename, pct));
        }
    }
    let border = if app.focus == TuiFocus::MessageView {
        BRAND_ACCENT
    } else {
        Color::DarkGray
    };

    // Auto-scroll: compute wrapped height and clamp / stick to bottom.
    let para = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border))
                .title(title),
        )
        .wrap(Wrap { trim: false });

    // ratatui 0.30 keeps `Paragraph::line_count` private, so compute the wrapped
    // height ourselves (greedy word-wrap, unicode-width aware) to clamp scroll.
    let total: u16 = plain_texts
        .iter()
        .map(|t| wrapped_rows(t, inner_w))
        .fold(0u16, |a, b| a.saturating_add(b))
        .max(1);
    let max_scroll = total.saturating_sub(inner_h);
    if app.stick_to_bottom {
        app.message_scroll = max_scroll;
    } else {
        if app.message_scroll >= max_scroll {
            app.stick_to_bottom = true;
        }
        app.message_scroll = app.message_scroll.min(max_scroll);
    }

    f.render_widget(para.scroll((app.message_scroll, 0)), area);
}

fn render_input(f: &mut Frame, app: &TuiApp, area: Rect) {
    let (title, text, style, border) = if app.mode == TuiMode::Command {
        (
            " Command ".to_string(),
            format!(":{}", app.command_field.text()),
            Style::default().fg(Color::Yellow),
            Color::Yellow,
        )
    } else {
        let title = if app.focus == TuiFocus::Input {
            " Message [focus] Enter=send Ctrl+J=newline ".to_string()
        } else {
            " Message ".to_string()
        };
        let border = if app.focus == TuiFocus::Input {
            BRAND_ACCENT
        } else {
            Color::DarkGray
        };
        (
            title,
            app.input_field.display_text(),
            Style::default(),
            border,
        )
    };

    let p = Paragraph::new(text)
        .style(style)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border))
                .title(title),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(p, area);
}

fn render_status(f: &mut Frame, app: &TuiApp, area: Rect) {
    let p = Paragraph::new(app.status_line.clone())
        .style(Style::default().fg(Color::Gray).bg(Color::Black));
    f.render_widget(p, area);
}

/// Stack the most recent toasts in the lower-right of the message area.
fn render_toasts(f: &mut Frame, app: &TuiApp, area: Rect) {
    let toasts = &app.chat_manager.toasts;
    if toasts.is_empty() {
        return;
    }
    let show = toasts.iter().rev().take(4).collect::<Vec<_>>();
    let width = area.width.saturating_sub(4).clamp(10, 50);
    let height = (show.len() as u16) + 2;
    if area.height <= height + 1 || area.width <= width + 2 {
        return;
    }
    let x = area.x + area.width.saturating_sub(width + 1);
    let y = area.y + area.height.saturating_sub(height + 1);
    let rect = Rect {
        x,
        y,
        width,
        height,
    };

    let lines: Vec<Line> = show
        .into_iter()
        .rev()
        .map(|t| {
            Line::from(Span::styled(
                truncate(&t.message, (width as usize).saturating_sub(2)),
                Style::default().fg(toast_color(t.level)),
            ))
        })
        .collect();

    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(" notices "),
        ),
        rect,
    );
}

/// Greedy word-wrap row count for a single logical line, matching ratatui's
/// `Wrap { trim: false }` closely enough to clamp/auto-scroll the message view.
/// Honors embedded newlines and hard-splits words longer than the width.
fn wrapped_rows(text: &str, width: u16) -> u16 {
    let width = width.max(1) as usize;
    let mut rows: u16 = 0;
    for segment in text.split('\n') {
        rows = rows.saturating_add(segment_rows(segment, width));
    }
    rows.max(1)
}

fn segment_rows(seg: &str, width: usize) -> u16 {
    let mut rows: u16 = 1;
    let mut col: usize = 0;
    let mut first = true;
    for word in seg.split(' ') {
        let w = UnicodeWidthStr::width(word);
        let advance = if first { w } else { w + 1 };
        if !first && col + advance > width {
            rows = rows.saturating_add(1);
            col = 0;
            // fall through to place the word at line start
        }
        if col == 0 {
            // place word at line start; hard-split if longer than width
            if w > width {
                let extra = ((w - 1) / width) as u16;
                rows = rows.saturating_add(extra);
                col = w - (extra as usize) * width;
            } else {
                col = w;
            }
        } else {
            col += advance;
        }
        first = false;
    }
    rows
}

fn block<'a>(title: &str, color: Color) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color))
        .title(title.to_string())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", t)
    }
}

#[cfg(test)]
mod tests {
    use super::{ui, wrapped_rows};
    use crate::tui::app::{TuiApp, TuiFocus};
    use crate::tui::overlays::{PasswordMode, TuiOverlay};
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
    fn wrapped_rows_handles_wrap_and_newlines() {
        assert_eq!(wrapped_rows("short", 80), 1);
        assert_eq!(wrapped_rows("a\nb\nc", 80), 3);
        // 10 chars at width 4 -> "aaaaaaaaaa" hard-splits into 3 rows.
        assert_eq!(wrapped_rows("aaaaaaaaaa", 4), 3);
        // Word wrap: two 4-char words at width 5 -> 2 rows.
        assert_eq!(wrapped_rows("aaaa bbbb", 5), 2);
    }

    #[test]
    fn renders_every_overlay_without_panic() {
        let overlays = [
            TuiOverlay::Help,
            TuiOverlay::FingerprintVerify {
                fingerprint: "ab".repeat(32),
                peer_name: "Peer".into(),
                chat_id: Uuid::new_v4(),
            },
            TuiOverlay::Contacts,
            TuiOverlay::Settings,
            TuiOverlay::Invite {
                link: "chat-p2p://invite/v2/abc".into(),
            },
            TuiOverlay::Identity,
            TuiOverlay::Transfers,
            TuiOverlay::Password {
                mode: PasswordMode::Unlock,
            },
            TuiOverlay::ConfirmQuit,
            TuiOverlay::ConfirmClearHistory,
        ];
        for ov in overlays {
            let mut app = TuiApp::new(EventCollector::new()).expect("app");
            app.overlay = ov;
            draw_once(&mut app);
        }
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
    fn render_does_not_panic_on_small_terminal_size() {
        let mut app = TuiApp::new(EventCollector::new()).expect("app");
        app.focus = TuiFocus::Input;
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|f| ui(f, &mut app)).expect("draw");
    }
}
