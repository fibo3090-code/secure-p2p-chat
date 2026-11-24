//! TUI rendering: chat list, message view (auto-scroll with clamping), input /
//! command box, toast stack and status bar, then any active overlay on top.

use crate::tui::app::{TuiApp, TuiFocus, TuiMode};
use crate::tui::command::COMMANDS;
use crate::tui::overlays::{self, block, focus_color, toast_color, truncate, BRAND_ACCENT};
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

/// The standard layout: one source of truth so the cursor placement in
/// `mod.rs` never drifts from the renderer.
pub fn regions(area: Rect) -> Regions {
    let vertical = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);
    let columns = Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(vertical[0]);
    let right = Layout::vertical([Constraint::Min(1), Constraint::Length(5)]).split(columns[1]);
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
    if app.monochrome {
        strip_colour(f.buffer_mut());
    }
}

/// `NO_COLOR`: drop every cell's colour after drawing, keeping its emphasis.
/// Done once over the finished frame rather than at each of the many styling
/// sites, so a new widget cannot forget it (TUI-21).
fn strip_colour(buf: &mut ratatui::buffer::Buffer) {
    for cell in buf.content.iter_mut() {
        cell.set_fg(Color::Reset).set_bg(Color::Reset);
    }
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
        .block(block(" commands · Tab to complete ", Color::Yellow))
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

    let focused = app.focus == TuiFocus::ChatList;
    let title = if focused {
        " Chats [focus] "
    } else {
        " Chats "
    };
    let list = List::new(items)
        .block(block(title, focus_color(focused)))
        .highlight_style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .bg(Color::DarkGray),
        )
        .highlight_symbol("> ");

    f.render_stateful_widget(list, area, &mut app.chat_list_state);
}

/// Bytes of one message's text drawn in the messages pane: a ~24 MiB text
/// re-wrapped every frame froze the terminal (PERF-02). History keeps it all.
const MAX_RENDER_TEXT_BYTES: usize = 8 * 1024;

/// `text`, cut to [`MAX_RENDER_TEXT_BYTES`] on a character boundary with a
/// note saying how much was left out.
fn render_text(text: &str) -> String {
    if text.len() <= MAX_RENDER_TEXT_BYTES {
        return text.to_string();
    }
    let mut cut = MAX_RENDER_TEXT_BYTES;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let rest_kib = (text.len() - cut).div_ceil(1024);
    format!("{} [… {rest_kib} KiB more not shown]", &text[..cut])
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
            MessageContent::Text { text } => render_text(text),
            MessageContent::File { filename, size, .. } => {
                format!("📎 {} ({})", filename, crate::util::format_size(*size))
            }
        };
        // Delivery receipt: the peer acknowledged this sent message.
        if msg.from_me && msg.delivered {
            content.push_str(" ✓");
        }
        // One screen line per line of the message: ratatui drops a `\n`
        // inside a span, which glued "a\nb" into "ab" (TUI-09). The first
        // carries the time and sender; the rest are indented under it — and
        // the text the scroll height is measured on is indented the same way.
        let indent = " ".repeat(ts.chars().count() + prefix.chars().count());
        plain_texts.push(format!(
            "{}{}{}",
            ts,
            prefix,
            content.replace('\n', &format!("\n{indent}"))
        ));
        let mut segments = content.split('\n');
        let first = segments.next().unwrap_or_default().to_string();
        let rest: Vec<String> = segments.map(str::to_string).collect();
        lines.push(Line::from(vec![
            Span::styled(ts, Style::default().fg(Color::DarkGray)),
            Span::styled(
                prefix,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(first),
        ]));
        for segment in rest {
            lines.push(Line::from(vec![
                Span::raw(indent.clone()),
                Span::raw(segment),
            ]));
        }
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
    let para = Paragraph::new(lines)
        .block(block(
            title,
            focus_color(app.focus == TuiFocus::MessageView),
        ))
        .wrap(Wrap { trim: false });

    // Auto-scroll: ratatui 0.30 keeps `Paragraph::line_count` private, so
    // compute the wrapped height ourselves to clamp / stick to the bottom.
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

/// Lay the input box's text out in rows of at most `width` columns, and find
/// the cursor — given as a char index into `text` — on them (TUI-20).
///
/// Rows are cut by **display width**, character by character, so this is
/// exactly what is drawn: the box renders these rows itself rather than
/// handing the text to a word-wrapper and guessing where the cursor went.
/// Returns the rows and the cursor's (row, column).
pub(crate) fn layout_input(text: &str, cursor: usize, width: u16) -> (Vec<String>, u16, u16) {
    use unicode_width::UnicodeWidthChar;
    let width = width.max(1) as usize;
    let mut rows = vec![String::new()];
    let mut col = 0usize;
    let mut at: Option<(usize, usize)> = None;
    for (i, c) in text.chars().enumerate() {
        if i == cursor {
            at = Some((rows.len() - 1, col));
        }
        if c == '\n' {
            rows.push(String::new());
            col = 0;
            continue;
        }
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if col + w > width && col > 0 {
            rows.push(String::new());
            col = 0;
            // A cursor placed just before this char belongs on the new row.
            if i == cursor {
                at = Some((rows.len() - 1, 0));
            }
        }
        rows.last_mut().expect("never empty").push(c);
        col += w;
    }
    let (mut row, mut ccol) = at.unwrap_or((rows.len() - 1, col));
    // An end cursor on a full row would sit on the border: start the next row.
    if ccol >= width {
        rows.push(String::new());
        row += 1;
        ccol = 0;
    }
    (rows, row as u16, ccol as u16)
}

/// The input box's text and cursor (char index), command prefix included —
/// shared by the renderer and the hardware-cursor placement so they agree.
pub(crate) fn input_text_and_cursor(app: &TuiApp) -> (String, usize) {
    if app.mode == TuiMode::Command {
        (
            format!(":{}", app.command_field.text()),
            app.command_field.cursor_index() + 1,
        )
    } else {
        (
            app.input_field.display_text(),
            app.input_field.cursor_index(),
        )
    }
}

/// The input box's rows, the first one shown, and the cursor relative to it.
pub(crate) fn input_view(app: &TuiApp, area: Rect) -> (Vec<String>, u16, (u16, u16)) {
    let inner_w = area.width.saturating_sub(2);
    let inner_h = area.height.saturating_sub(2).max(1);
    let (text, cursor) = input_text_and_cursor(app);
    let (rows, row, col) = layout_input(&text, cursor, inner_w);
    // Keep the cursor's row in view: scroll just far enough down.
    let offset = row.saturating_sub(inner_h - 1);
    (rows, offset, (row - offset, col))
}

fn render_input(f: &mut Frame, app: &TuiApp, area: Rect) {
    let (title, style, border) = if app.mode == TuiMode::Command {
        (
            " Command ".to_string(),
            Style::default().fg(Color::Yellow),
            Color::Yellow,
        )
    } else if app.focus == TuiFocus::Input {
        (
            " Message [focus] Enter=send Ctrl+J=newline ".to_string(),
            Style::default(),
            BRAND_ACCENT,
        )
    } else {
        (" Message ".to_string(), Style::default(), Color::DarkGray)
    };

    // Rows are laid out here, not by a word-wrapper, so the scroll offset
    // and the cursor are computed on exactly what is drawn (TUI-20).
    let (rows, offset, _) = input_view(app, area);
    let lines: Vec<Line> = rows.into_iter().map(Line::from).collect();
    let p = Paragraph::new(lines)
        .style(style)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border))
                .title(title),
        )
        .scroll((offset, 0));
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
    let rect = Rect {
        x: area.x + area.width.saturating_sub(width + 1),
        y: area.y + area.height.saturating_sub(height + 1),
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
        Paragraph::new(lines).block(block(" notices ", Color::DarkGray)),
        rect,
    );
}

/// Greedy word-wrap row count, close enough to ratatui's `Wrap { trim: false }`
/// to clamp the message view. Honors newlines; hard-splits overlong words.
fn wrapped_rows(text: &str, width: u16) -> u16 {
    let width = width.max(1) as usize;
    text.split('\n')
        .fold(0u16, |rows, seg| {
            rows.saturating_add(segment_rows(seg, width))
        })
        .max(1)
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
        }
        if col == 0 {
            // The word starts a line; hard-split it if longer than the width.
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

#[cfg(test)]
mod input_layout_tests {
    use super::layout_input;

    /// TUI-20: rows are cut at the box width — by display width, so a wide
    /// character never straddles the border — and the cursor is found on
    /// them, not on the logical line.
    #[test]
    fn input_rows_wrap_at_the_box_width_and_carry_the_cursor() {
        let (rows, row, col) = layout_input("abcdefgh", 8, 3);
        assert_eq!(rows, ["abc", "def", "gh"]);
        assert_eq!((row, col), (2, 2), "cursor after the last char");

        let (_, row, col) = layout_input("abcdefgh", 4, 3);
        assert_eq!((row, col), (1, 1), "cursor before 'e'");

        // A full last row puts an end cursor at the start of the next one,
        // never on the border.
        let (rows, row, col) = layout_input("abcdef", 6, 3);
        assert_eq!((row, col), (2, 0));
        assert_eq!(rows.len(), 3);

        // Hard newlines start rows; a wide char that would cross the edge
        // moves down whole.
        let (rows, _, _) = layout_input("ab\nc", 0, 10);
        assert_eq!(rows, ["ab", "c"]);
        let (rows, row, col) = layout_input("ab界", 3, 3);
        assert_eq!(rows, ["ab", "界"]);
        assert_eq!((row, col), (1, 2));
    }
}

#[cfg(test)]
mod tests {
    use super::{render_text, ui, wrapped_rows, MAX_RENDER_TEXT_BYTES};
    use crate::logbuf::LogBuffer;
    use crate::tui::app::{TuiApp, TuiFocus};
    use crate::tui::overlays::{PasswordMode, TuiOverlay};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use uuid::Uuid;

    fn app() -> TuiApp {
        TuiApp::ephemeral(LogBuffer::new()).expect("app")
    }

    fn draw(app: &mut TuiApp, width: u16, height: u16) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal.draw(|f| ui(f, app)).expect("draw");
    }

    /// TUI-09: a message was one `Line` with the text in a single `Span`, and
    /// ratatui drops control characters inside a span — so "line one\nline
    /// two" rendered as "line oneline two". Each line of a message is its own
    /// line on screen.
    #[test]
    fn a_multi_line_message_keeps_its_lines() {
        let mut app = app();
        let chat = Uuid::new_v4();
        app.chat_manager
            .create_local_chat_for_test(chat, "Peer".into());
        app.chat_manager.get_chat_mut(chat).unwrap().messages.push(
            messenger_core::types::Message {
                id: Uuid::new_v4(),
                from_me: false,
                content: messenger_core::types::MessageContent::Text {
                    text: "first line\nsecond line".into(),
                },
                timestamp: chrono::Utc::now(),
                delivered: false,
            },
        );
        app.tick();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
        terminal.draw(|f| ui(f, &mut app)).expect("draw");
        let buf = terminal.backend().buffer();
        let rows: Vec<String> = (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect();
        let first = rows
            .iter()
            .position(|r| r.contains("first line"))
            .expect("first line shown");
        assert!(
            !rows[first].contains("second line"),
            "joined: {}",
            rows[first]
        );
        assert!(
            rows.get(first + 1)
                .is_some_and(|r| r.contains("second line")),
            "the second line is not below the first: {rows:#?}"
        );
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
                sas: "12-34-56 🐶 🐱 🦊".into(),
                chat_id: Uuid::new_v4(),
                warning: None,
            },
            TuiOverlay::FingerprintVerify {
                fingerprint: "ab".repeat(32),
                peer_name: "Peer".into(),
                sas: "12-34-56 🐶 🐱 🦊".into(),
                chat_id: Uuid::new_v4(),
                warning: Some(crate::app::FingerprintWarning::InviteMismatch {
                    contact_id: Uuid::new_v4(),
                    contact_name: "Alice".into(),
                    expected: "cd".repeat(32),
                }),
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
            TuiOverlay::ConfirmWipe,
        ];
        for ov in overlays {
            let mut app = app();
            app.overlay = ov;
            draw(&mut app, 100, 30);
        }
    }

    fn screen(app: &mut TuiApp, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal.draw(|f| ui(f, app)).expect("draw");
        let buf = terminal.backend().buffer();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    /// The transfers overlay is the only place the TUI shows a transfer's
    /// state, and the only one where an offer waiting for :accept says so.
    /// Each status reads as itself, progress is a percentage of the size, the
    /// direction is drawn, and the selection marker sits on the chosen row.
    #[test]
    fn the_transfers_overlay_says_what_each_transfer_is_doing() {
        use messenger_core::types::{FileTransferState, TransferDirection, TransferStatus};
        let mut app = app();
        let chat = Uuid::new_v4();
        let rows = [
            (
                "offer.pdf",
                TransferStatus::AwaitingAcceptance,
                TransferDirection::Incoming,
                0,
            ),
            (
                "half.bin",
                TransferStatus::InProgress,
                TransferDirection::Outgoing,
                512,
            ),
            (
                "broke.iso",
                TransferStatus::Failed("disk full".into()),
                TransferDirection::Incoming,
                10,
            ),
        ];
        for (i, (name, status, direction, received)) in rows.into_iter().enumerate() {
            app.chat_manager
                .insert_transfer_for_test(FileTransferState {
                    id: Uuid::from_u128(i as u128 + 1),
                    chat_id: chat,
                    filename: name.into(),
                    size: 1024,
                    received,
                    status,
                    seq: 0,
                    direction,
                });
        }
        app.overlay = TuiOverlay::Transfers;
        let shown = screen(&mut app, 160, 40);
        let row = |name: &str| {
            shown
                .iter()
                .find(|r| r.contains(name))
                .unwrap_or_else(|| panic!("{name} not shown:\n{}", shown.join("\n")))
                .clone()
        };
        assert!(row("offer.pdf").contains("[awaiting approval — :accept / :decline]"));
        assert!(row("half.bin").contains("[50%]"), "{}", row("half.bin"));
        assert!(row("half.bin").contains('⬆') && row("offer.pdf").contains('⬇'));
        assert!(row("broke.iso").contains("failed: disk full"));

        let marked: Vec<_> = shown.iter().filter(|r| r.contains('▶')).collect();
        assert_eq!(marked.len(), 1, "one selected row");
        app.transfer_sel = 2;
        let shown = screen(&mut app, 160, 40);
        assert!(shown.iter().any(|r| r.contains("▶ ⬇ broke.iso")));
        // A selection past the end (the list shrank) still marks a real row.
        app.transfer_sel = 99;
        let shown = screen(&mut app, 160, 40);
        assert_eq!(shown.iter().filter(|r| r.contains('▶')).count(), 1);
    }

    /// What each of these overlay states *says*, not only that it draws:
    /// `renders_every_overlay_without_panic` passes for an overlay that
    /// renders nothing at all.
    #[test]
    fn overlays_say_what_the_state_they_show_is() {
        use crate::app::party_manager::PartyStatus;
        use crate::app::FingerprintWarning;
        let make = app;
        let joined = |rows: &[String]| rows.join("\n");
        let prompt = |sas: &str, warning| TuiOverlay::FingerprintVerify {
            fingerprint: "ab".repeat(32),
            peer_name: "Peer".into(),
            sas: sas.into(),
            chat_id: Uuid::new_v4(),
            warning,
        };

        // :help <command> shows that command, not the list.
        let mut app = make();
        app.help_topic = Some("send".into());
        app.overlay = TuiOverlay::Help;
        let shown = joined(&screen(&mut app, 120, 40));
        assert!(
            shown.contains(":send <path>") && shown.contains("Command name: send"),
            "{shown}"
        );

        // A changed key and a key that is not the invite's each lead with a
        // warning naming what was expected.
        for (warning, says) in [
            (
                FingerprintWarning::KeyChanged {
                    previous: "cd".repeat(32),
                },
                "key has CHANGED (was cdcdcdcdcdcdcdcd…)",
            ),
            (
                FingerprintWarning::InviteMismatch {
                    contact_id: Uuid::new_v4(),
                    contact_name: "Alice".into(),
                    expected: "ef".repeat(32),
                },
                "NOT the key in Alice's invite (expected efefefefefefefef…)",
            ),
        ] {
            let mut app = make();
            app.overlay = prompt("12-34-56 🐶 🐱 🦊", Some(warning));
            let shown = joined(&screen(&mut app, 160, 50));
            assert!(shown.contains(says), "{says}:\n{shown}");
            assert!(shown.contains("Read this code aloud"));
        }

        // A legacy peer has no safety code: the prompt falls back to the
        // grid and says so, rather than showing an empty code to compare.
        let mut app = make();
        app.overlay = prompt("", None);
        let shown = joined(&screen(&mut app, 160, 50));
        assert!(
            shown.contains("safety grid / fingerprint matches"),
            "{shown}"
        );
        assert!(!shown.contains("Read this code aloud") && !shown.contains("WARNING"));

        // A refused join says why in the Communities pane.
        let mut app = make();
        let (server, _, _out, _keep) = app.party_manager.add_joined_server_for_test("srv:1");
        app.party_manager
            .server_mut_for_test(server)
            .unwrap()
            .status = PartyStatus::Rejected("wrong password".into());
        app.overlay = TuiOverlay::Party;
        let shown = joined(&screen(&mut app, 160, 50));
        assert!(shown.contains("rejected: wrong password"), "{shown}");

        // Setting a password is two steps, and the second says it is the second.
        let mut app = make();
        app.overlay = TuiOverlay::Password {
            mode: PasswordMode::Set,
        };
        assert!(joined(&screen(&mut app, 120, 40)).contains("at least 12 characters"));
        app.new_password_first = Some(zeroize::Zeroizing::new("first attempt".into()));
        assert!(joined(&screen(&mut app, 120, 40)).contains("Type the same password again"));
    }

    #[test]
    fn render_does_not_panic_with_stale_chat_ids() {
        let mut app = app();
        app.chat_ids = vec![Uuid::new_v4()];
        app.chat_list_state.select(Some(0));
        draw(&mut app, 100, 30);
    }

    #[test]
    fn a_huge_message_is_cut_for_drawing() {
        let text = "é".repeat(20_000); // 40 KB, two bytes per char
        let shown = render_text(&text);
        assert!(shown.len() < MAX_RENDER_TEXT_BYTES + 64);
        assert!(shown.ends_with("KiB more not shown]"));
        assert_eq!(render_text("short"), "short");
    }

    #[test]
    fn render_does_not_panic_on_small_terminal_size() {
        let mut app = app();
        app.focus = TuiFocus::Input;
        draw(&mut app, 40, 10);
    }
}
