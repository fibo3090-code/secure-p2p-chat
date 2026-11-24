//! Modal overlays drawn over the chat layout. While one is open, keys route to
//! it first (`TuiApp::handle_key_event`); each has an equivalent `:`-command.

use crate::app::FingerprintWarning;
use crate::tui::app::TuiApp;
use crate::tui::command::{command_help, COMMANDS};
use crate::types::{ToastLevel, TransferStatus};
use ratatui::{prelude::*, widgets::*};
use uuid::Uuid;

/// What a password overlay is for: unlocking an encrypted identity, or
/// setting a password on an unencrypted one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordMode {
    Unlock,
    Set,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TuiOverlay {
    #[default]
    None,
    Help,
    FingerprintVerify {
        fingerprint: String,
        peer_name: String,
        sas: String,
        chat_id: Uuid,
        /// Set when this is not an ordinary first use: the key changed, or it
        /// is not the one in the dialled contact's invite (CRYPTO-04).
        warning: Option<FingerprintWarning>,
    },
    Contacts,
    Settings,
    Invite {
        link: String,
    },
    Identity,
    Transfers,
    /// The Communities pane.
    Party,
    Password {
        mode: PasswordMode,
    },
    ConfirmQuit,
    ConfirmClearHistory,
    /// `:delete` on a conversation: deleting its history cannot be undone.
    ConfirmDeleteChat {
        chat_id: uuid::Uuid,
    },
    /// `:wipe` after the password checked out; only Ctrl+Y proceeds.
    ConfirmWipe,
}

impl TuiOverlay {
    pub fn is_open(&self) -> bool {
        !matches!(self, TuiOverlay::None)
    }
}

/// Compute a centered rectangle `pct_x` × `pct_y` percent of `area`.
pub fn centered_rect(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
    let split = |pct: u16| {
        let margin = Constraint::Percentage((100 - pct) / 2);
        [margin, Constraint::Percentage(pct), margin]
    };
    let vertical = Layout::vertical(split(pct_y)).split(area);
    Layout::horizontal(split(pct_x)).split(vertical[1])[1]
}

/// Chrome accent (borders, key hints) from design/tokens.json. Semantic colors
/// (success/warning/error/info) stay separate: they carry meaning, not brand.
pub(crate) const BRAND_ACCENT: Color = Color::Rgb(0x4a, 0x68, 0xec);

/// A bordered block in `color`.
pub(crate) fn block<'a>(title: impl Into<Line<'a>>, color: Color) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color))
        .title(title)
}

/// The accent for the focused pane, gray otherwise.
pub(crate) fn focus_color(focused: bool) -> Color {
    if focused {
        BRAND_ACCENT
    } else {
        Color::DarkGray
    }
}

fn selected_style(selected: bool) -> Style {
    if selected {
        Style::default().fg(BRAND_ACCENT).bold()
    } else {
        Style::default()
    }
}

fn dim<'a>(text: impl Into<std::borrow::Cow<'a, str>>) -> Span<'a> {
    Span::styled(text, Style::default().fg(Color::DarkGray))
}

fn heading(text: &str) -> Line<'_> {
    Line::from(Span::styled(
        text,
        Style::default().fg(Color::Yellow).bold(),
    ))
}

/// A gray, wrapped explanatory paragraph.
fn note(f: &mut Frame, text: &str, area: Rect) {
    f.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(Color::Gray)),
        area,
    );
}

/// A one-line key hint.
fn hint(f: &mut Frame, text: &str, area: Rect) {
    f.render_widget(Paragraph::new(dim(text)), area);
}

fn highlight_list<'a>(items: impl IntoIterator<Item = ListItem<'a>>) -> List<'a> {
    List::new(items)
        .highlight_style(Style::default().bg(Color::DarkGray).bold())
        .highlight_symbol("> ")
}

/// The safety grid (the frozen palette every front-end renders, so users
/// compare the same shape everywhere) and the fingerprint hex.
fn render_grid_and_hex(f: &mut Frame, fingerprint: &str, grid: Rect, hex: Rect) {
    let lines: Vec<Line> = crate::colorgrid::generate_color_grid(fingerprint)
        .iter()
        .map(|row| {
            Line::from(
                row.iter()
                    .map(|c| Span::styled("███ ", Style::default().fg(Color::Rgb(c.0, c.1, c.2))))
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    f.render_widget(Paragraph::new(lines), grid);
    f.render_widget(
        // The terminal's own foreground: white was white-on-white on a light
        // theme, and this is the backstop for the safety code (TUI-21).
        Paragraph::new(fingerprint.to_string()).wrap(Wrap { trim: true }),
        hex,
    );
}

/// Trim to `max` display columns, marking the cut so a truncated name does not
/// read as the whole name.
pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// Render the active overlay over `area`. No-op when none is open.
pub fn render_overlay(f: &mut Frame, app: &mut TuiApp, area: Rect) {
    match app.overlay.clone() {
        TuiOverlay::None => {}
        TuiOverlay::Help => render_help(f, app, area),
        TuiOverlay::FingerprintVerify {
            fingerprint,
            peer_name,
            sas,
            warning,
            ..
        } => render_fingerprint(f, &fingerprint, &peer_name, &sas, warning.as_ref(), area),
        TuiOverlay::Contacts => render_contacts(f, app, area),
        TuiOverlay::Settings => render_settings(f, app, area),
        TuiOverlay::Invite { link } => render_invite(f, &link, area),
        TuiOverlay::Identity => render_identity(f, app, area),
        TuiOverlay::Transfers => render_transfers(f, app, area),
        TuiOverlay::Party => render_party(f, app, area),
        TuiOverlay::Password { mode } => render_password(f, app, mode, area),
        TuiOverlay::ConfirmQuit => render_confirm(
            f,
            "Quit?",
            "Save history and exit? (y = quit, n = cancel)",
            area,
        ),
        TuiOverlay::ConfirmDeleteChat { .. } => render_confirm(
            f,
            "Delete this conversation?",
            "Its messages are deleted from this device and cannot be recovered. \
             (y = delete, n = cancel)",
            area,
        ),
        TuiOverlay::ConfirmClearHistory => render_confirm(
            f,
            "Clear all history?",
            "This erases ALL chats and contacts. (y = erase, n = cancel)",
            area,
        ),
        TuiOverlay::ConfirmWipe => render_confirm(
            f,
            "Delete ALL data?",
            "This permanently deletes your identity, every chat and contact, saved \
             communities and diagnostics from this device, then quits. It cannot be \
             undone.\n\nCtrl+Y = delete everything · any other key = cancel",
            area,
        ),
    }
}

fn clear_and_block(f: &mut Frame, area: Rect, title: &str) -> Rect {
    f.render_widget(Clear, area);
    let title = Span::styled(format!(" {} ", title), Style::default().bold());
    let block = block(title, BRAND_ACCENT);
    let inner = block.inner(area);
    f.render_widget(block, area);
    inner
}

fn render_help(f: &mut Frame, app: &TuiApp, full: Rect) {
    let area = centered_rect(80, 80, full);
    let title = if app.help_topic.is_some() {
        "Help — command"
    } else {
        "Help — commands & keys"
    };
    let inner = clear_and_block(f, area, title);

    let mut lines: Vec<Line> = Vec::new();
    if let Some(topic) = app.help_topic.as_deref() {
        if let Some((name, usage, desc)) = command_help(topic) {
            lines.push(Line::from(Span::styled(
                usage,
                Style::default().fg(Color::Green).bold(),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(desc));
            lines.push(Line::from(""));
            lines.push(Line::from(format!("Command name: {}", name)));
            lines.push(Line::from(""));
            lines.push(Line::from(dim(
                "Use :help for the full command list · Esc to close",
            )));
        }
    } else {
        lines.push(heading("Keys"));
        for (k, v) in [
            ("Tab", "cycle focus (chats / messages / input)"),
            (":", "enter command mode"),
            ("Enter", "send message (input) / submit command"),
            ("Ctrl+J", "newline in the message input"),
            ("↑/↓", "select chat / scroll messages / command history"),
            ("PgUp/PgDn", "scroll messages"),
            ("Ctrl+L", "copy logs to clipboard"),
            ("Ctrl+C", "quit (asks; Ctrl+C again to confirm)"),
            ("y / n", "confirm / cancel in confirm overlays"),
            (
                "Ctrl+Y / Ctrl+N",
                "accept / reject a peer's identity (Esc = decide later)",
            ),
            ("Esc", "close overlay / leave command mode"),
        ] {
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<10}", k), Style::default().fg(BRAND_ACCENT)),
                Span::raw(v),
            ]));
        }
        lines.push(Line::from(""));
        lines.push(heading("Chat markers"));
        lines.push(Line::from(
            "  H hosting   ● connected   ○ offline   * unread",
        ));
        lines.push(Line::from(""));
        lines.push(heading("Commands"));
        for (_, usage, desc) in COMMANDS {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {:<41}", usage),
                    Style::default().fg(Color::Green),
                ),
                Span::raw(*desc),
            ]));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(dim("Esc or :help to close · ↑/↓ to scroll")));
    }

    let para = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((app.overlay_scroll, 0));
    f.render_widget(para, inner);
}

/// The text the fingerprint prompt leads with when it is not a first use.
pub(crate) fn fingerprint_warning_text(warning: &FingerprintWarning) -> String {
    match warning {
        FingerprintWarning::KeyChanged { previous } => format!(
            "⚠ WARNING: this conversation's key has CHANGED (was {}…). It may be a new device — or someone intercepting the connection. Reject unless the codes match.",
            short_fingerprint(previous)
        ),
        FingerprintWarning::InviteMismatch {
            contact_name,
            expected,
            ..
        } => format!(
            "⚠ WARNING: this is NOT the key in {}'s invite (expected {}…). Someone may be intercepting the connection. Reject unless the codes match.",
            contact_name,
            short_fingerprint(expected)
        ),
    }
}

fn short_fingerprint(fp: &str) -> String {
    fp.chars().take(16).collect()
}

fn render_fingerprint(
    f: &mut Frame,
    fingerprint: &str,
    peer_name: &str,
    sas: &str,
    warning: Option<&FingerprintWarning>,
    full: Rect,
) {
    let area = centered_rect(70, 70, full);
    let inner = clear_and_block(f, area, "🛡 Verify peer identity");

    let has_sas = !sas.is_empty();
    let rows = Layout::vertical([
        Constraint::Length(if warning.is_some() { 3 } else { 0 }), // warning
        Constraint::Length(2),                                     // heading
        Constraint::Length(2),                                     // instruction
        Constraint::Length(if has_sas { 3 } else { 0 }),           // SAS
        Constraint::Min(6),                                        // color grid
        Constraint::Length(2),                                     // fingerprint hex
        Constraint::Length(1),                                     // actions
    ])
    .split(inner);

    // A warning leads: the SAS still decides, but the user must know this
    // is not an ordinary first contact before reading the code.
    if let Some(warning) = warning {
        f.render_widget(
            Paragraph::new(fingerprint_warning_text(warning))
                .wrap(Wrap { trim: true })
                .style(
                    Style::default()
                        .fg(Color::White)
                        .bg(Color::Red)
                        .add_modifier(Modifier::BOLD),
                ),
            rows[0],
        );
    }

    f.render_widget(
        Paragraph::new(format!("Connecting to {}", peer_name)).style(Style::default().bold()),
        rows[1],
    );

    // The SAS leads: both peers see the same code and a MITM makes them
    // differ. The safety grid / hex stays as the backstop.
    if has_sas {
        note(
            f,
            "Read this code aloud with your peer — it must match on both ends:",
            rows[2],
        );
        f.render_widget(
            Paragraph::new(sas.to_string())
                .alignment(ratatui::layout::Alignment::Center)
                .style(Style::default().fg(BRAND_ACCENT).bold()),
            rows[3],
        );
    } else {
        note(
            f,
            "Confirm this safety grid / fingerprint matches your peer's, via a separate channel.",
            rows[2],
        );
    }
    render_grid_and_hex(f, fingerprint, rows[4], rows[5]);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " Ctrl+Y ",
                Style::default().bg(Color::Green).fg(Color::Black),
            ),
            Span::raw(" accept   "),
            Span::styled(" Ctrl+N ", Style::default().bg(Color::Red).fg(Color::White)),
            Span::raw(" reject   "),
            Span::styled(
                " Esc ",
                Style::default().bg(Color::DarkGray).fg(Color::White),
            ),
            Span::raw(" later (:verify accept|reject)"),
        ])),
        rows[6],
    );
}

fn render_contacts(f: &mut Frame, app: &mut TuiApp, full: Rect) {
    let area = centered_rect(70, 70, full);
    let inner = clear_and_block(f, area, "Contacts");

    if app.contact_ids.is_empty() {
        note(
            f,
            "No contacts yet.\n\nAdd one with :contact-add <name> <host:port> [fingerprint]\nor import an invite with :import <link>.",
            inner,
        );
        return;
    }

    let items: Vec<ListItem> = app
        .contact_ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let c = app.chat_manager.contacts.get(id);
            let (name, detail) = c
                .map(|c| {
                    let route = c
                        .address
                        .clone()
                        .or_else(|| c.relay_server.clone())
                        .unwrap_or_else(|| "no address".to_string());
                    (c.name.clone(), route)
                })
                .unwrap_or_else(|| ("?".into(), String::new()));
            ListItem::new(format!("{:>2}. {:<20} {}", i + 1, name, detail))
        })
        .collect();

    f.render_stateful_widget(highlight_list(items), inner, &mut app.contacts_list_state);
}

fn render_settings(f: &mut Frame, app: &mut TuiApp, full: Rect) {
    let area = centered_rect(70, 70, full);
    let inner = clear_and_block(f, area, "Settings (Enter toggles · :set <key> <value>)");

    let cfg = &app.chat_manager.config;
    let rows = [
        format!("download-dir   {}", cfg.download_dir.display()),
        format!("listen-port    {}", cfg.listen_port),
        format!("notifications  {}", on_off(cfg.enable_notifications)),
        format!("typing         {}", on_off(cfg.enable_typing_indicators)),
        format!("auto-accept    {}", on_off(cfg.auto_accept_files)),
        format!("auto-host      {}", on_off(cfg.auto_host_on_startup)),
        format!("upnp           {}", on_off(cfg.enable_upnp)),
    ];
    let list = highlight_list(rows.map(ListItem::new));
    f.render_stateful_widget(list, inner, &mut app.settings_list_state);
}

fn on_off(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

fn render_invite(f: &mut Frame, link: &str, full: Rect) {
    let area = centered_rect(80, 50, full);
    let inner = clear_and_block(f, area, "Invite link (copied to clipboard)");
    f.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "Share this with your peer; they run :import <link>.",
                Style::default().fg(Color::Gray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                link.to_string(),
                Style::default().fg(Color::Green),
            )),
            Line::from(""),
            Line::from(dim("Esc to close")),
        ])
        .wrap(Wrap { trim: false }),
        inner,
    );
}

fn render_identity(f: &mut Frame, app: &TuiApp, full: Rect) {
    let area = centered_rect(70, 70, full);
    let inner = clear_and_block(f, area, "Your identity (fingerprint copied)");

    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(6),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .split(inner);

    f.render_widget(
        Paragraph::new(format!("Name: {}", app.identity_name)).style(Style::default().bold()),
        rows[0],
    );
    render_grid_and_hex(f, &app.identity_fingerprint(), rows[1], rows[2]);
    hint(f, "Esc to close", rows[3]);
}

fn render_transfers(f: &mut Frame, app: &TuiApp, full: Rect) {
    let area = centered_rect(70, 60, full);
    let inner = clear_and_block(f, area, "File transfers");

    let transfers = app.chat_manager.active_transfers_sorted();
    if transfers.is_empty() {
        note(
            f,
            "No active transfers.\n\nSend a file with :send <path>.",
            inner,
        );
        return;
    }
    let rows = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);

    let sel = app.transfer_sel.min(transfers.len().saturating_sub(1));
    let lines: Vec<Line> = transfers
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let pct = if t.size > 0 {
                (t.received as f64 / t.size as f64 * 100.0) as u64
            } else {
                0
            };
            let status = match &t.status {
                TransferStatus::Pending => "pending".into(),
                TransferStatus::AwaitingAcceptance => {
                    "awaiting approval — :accept / :decline".into()
                }
                TransferStatus::InProgress => format!("{}%", pct),
                TransferStatus::Completed => "done".into(),
                TransferStatus::Failed(e) => format!("failed: {}", e),
                TransferStatus::Cancelled => "cancelled".into(),
            };
            let arrow = match t.direction {
                crate::types::TransferDirection::Incoming => "⬇",
                crate::types::TransferDirection::Outgoing => "⬆",
            };
            let marker = if i == sel { "▶ " } else { "  " };
            let text = format!(
                "{}{} {:<22} {} / {}  [{}]",
                marker,
                arrow,
                t.filename,
                crate::util::format_size(t.received),
                crate::util::format_size(t.size),
                status
            );
            Line::styled(text, selected_style(i == sel))
        })
        .collect();
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), rows[0]);
    hint(f, "↑/↓ select · c cancel · Esc close", rows[1]);
}

/// The Communities pane: joined servers and, for the selected one, its threads
/// (channels with their access rule, DMs with role and presence), the selected
/// thread's messages, and the files you can see. Commands remain the way to act.
fn render_party(f: &mut Frame, app: &TuiApp, full: Rect) {
    let area = centered_rect(84, 76, full);
    let inner = clear_and_block(f, area, "Communities");

    let servers = app.party_manager.server_ids();
    if servers.is_empty() {
        note(
            f,
            "You have not joined any communities.\n\n\
                 Join one with:\n  \
                 :party-connect <host[:port]> <username> [password]\n\n\
                 The first time you connect to an address you will be shown the \
                 server's fingerprint to check with its operator; run :party-trust \
                 to accept it. Nothing — not your username, not the password — is \
                 sent before you do.\n\n\
                 Communities are not end-to-end encrypted: the server's operator \
                 can read every message, DM and file posted there.",
            inner,
        );
        return;
    }

    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(inner);

    let sel = app.party_sel.min(servers.len().saturating_sub(1));
    // Server tabs across the top, so it is obvious which one the rest describes.
    let mut tabs: Vec<Span> = servers
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let name = app.party_manager.server(*id).map_or("?", |c| {
                if c.server_name.is_empty() {
                    &c.address
                } else {
                    &c.server_name
                }
            });
            let text = format!(" {} ", name);
            if i == sel {
                Span::styled(text, selected_style(true))
            } else {
                dim(text)
            }
        })
        .collect();
    // Always visible: community traffic is stored readable by the operator
    // (audit JOURNEY-04 / PRIV-01), and a DM here looks like any other.
    tabs.push(Span::styled(
        "  ⚠ operator can read messages, DMs and files",
        Style::default().fg(Color::Yellow),
    ));
    f.render_widget(Paragraph::new(Line::from(tabs)), rows[0]);

    let Some(conn) = servers
        .get(sel)
        .and_then(|id| app.party_manager.server(*id))
    else {
        return;
    };

    let cols =
        Layout::horizontal([Constraint::Percentage(34), Constraint::Percentage(66)]).split(rows[1]);

    // Left: channels, then a DM with each member; ↑/↓ picks the one shown on
    // the right (and that :party-post writes to).
    let threads = app.party_threads();
    let thread_sel = app.party_thread_sel.min(threads.len().saturating_sub(1));
    let section =
        |label: &'static str| Line::styled(label, Style::default().fg(Color::DarkGray).bold());
    let mut left: Vec<Line> = Vec::new();
    left.push(Line::from(dim(format!(
        "{}  ({})",
        conn.address,
        party_status_label(&conn.status)
    ))));
    left.push(Line::raw(""));
    left.push(section("CHANNELS"));
    let mut dm_header_shown = false;
    for (i, thread) in threads.iter().enumerate() {
        let marker = if i == thread_sel { "▶ " } else { "  " };
        let style = selected_style(i == thread_sel);
        match thread {
            crate::tui::app::PartyThread::Channel { id, name } => {
                // The kind is the access rule, so it belongs next to the name.
                let suffix = conn
                    .channels
                    .iter()
                    .find(|c| c.id == *id)
                    .map(|c| match c.kind {
                        messenger_core::party::ChannelKind::Public => String::new(),
                        other => format!("  ({})", other.label().to_lowercase()),
                    })
                    .unwrap_or_default();
                left.push(Line::styled(format!("{marker}#{name}{suffix}"), style));
            }
            crate::tui::app::PartyThread::Dm { member, name } => {
                if !dm_header_shown {
                    left.push(Line::raw(""));
                    left.push(section("DIRECT MESSAGES"));
                    dm_header_shown = true;
                }
                let info = conn.members.iter().find(|m| m.id == *member);
                let dot = if info.is_some_and(|m| m.online) {
                    "●"
                } else {
                    "○"
                };
                let role = info
                    .filter(|m| m.role != messenger_core::party::Role::Member)
                    .map(|m| format!("  [{}]", m.role.label().to_lowercase()))
                    .unwrap_or_default();
                left.push(Line::styled(format!("{marker}{dot} {name}{role}"), style));
            }
        }
    }
    f.render_widget(Paragraph::new(left).wrap(Wrap { trim: false }), cols[0]);

    // Right: the thread's messages, newest at the bottom, then files/quota.
    let right_rows = Layout::vertical([Constraint::Min(3), Constraint::Length(7)]).split(cols[1]);
    let name_of = |id: uuid::Uuid| {
        if Some(id) == conn.member_id {
            "you".to_string()
        } else {
            conn.members
                .iter()
                .find(|m| m.id == id)
                .map(|m| m.username.clone())
                .unwrap_or_else(|| "?".to_string())
        }
    };
    let messages = app.selected_party_messages();
    let capacity = right_rows[0].height as usize;
    let mut msg_lines: Vec<Line> = Vec::new();
    if messages.is_empty() {
        msg_lines.push(Line::from(dim(
            "  no messages yet — :party-post <text> writes here",
        )));
    }
    for env in messages
        .iter()
        .skip(messages.len().saturating_sub(capacity))
    {
        let body = match &env.payload {
            messenger_core::party::MessagePayload::Text(t) => t.replace('\n', " ⏎ "),
            messenger_core::party::MessagePayload::File(meta) => format!(
                "📎 {} ({}) — :party-download {}",
                meta.name,
                crate::util::format_size(meta.size),
                meta.name
            ),
        };
        let pending = if env.seq == 0 { " (sending…)" } else { "" };
        msg_lines.push(Line::from(vec![
            Span::styled(
                format!("{}: ", name_of(env.sender)),
                Style::default().fg(BRAND_ACCENT),
            ),
            Span::raw(format!("{body}{pending}")),
        ]));
    }
    f.render_widget(
        Paragraph::new(msg_lines).wrap(Wrap { trim: false }),
        right_rows[0],
    );

    let mut right: Vec<Line> = Vec::new();
    right.push(section("SHARED FILES"));
    if conn.files.is_empty() {
        right.push(Line::from(dim(
            "  none visible — :party-files refreshes this",
        )));
    } else {
        for file in conn.files.iter().take(3) {
            right.push(Line::raw(format!(
                "  {:<24} {:>9}  {} · {}",
                truncate(&file.name, 24),
                crate::util::format_size(file.size),
                file.uploader_name,
                file.location_name
            )));
        }
        if conn.files.len() > 3 {
            right.push(Line::from(dim(format!(
                "  … and {} more",
                conn.files.len() - 3
            ))));
        }
    }
    if let Some(q) = conn.quota {
        let size = crate::util::format_size;
        let mine = match q.limit {
            Some(limit) => format!("{} of {}", size(q.used), size(limit)),
            None => format!("{} (no personal limit)", size(q.used)),
        };
        right.push(Line::from(dim(format!(
            "  You: {mine}   Server: {} of {}",
            size(q.server_used),
            size(q.server_limit)
        ))));
    }
    if let Some(err) = &conn.last_error {
        right.push(Line::styled(
            format!("  {}", err),
            Style::default().fg(Color::Red),
        ));
    }
    f.render_widget(
        Paragraph::new(right).wrap(Wrap { trim: false }),
        right_rows[1],
    );

    hint(
        f,
        "←/→ community · ↑/↓ thread · :party-post writes here · :party-files · Esc close",
        rows[2],
    );
}

fn party_status_label(status: &crate::app::party_manager::PartyStatus) -> String {
    use crate::app::party_manager::PartyStatus;
    match status {
        PartyStatus::Connecting => "connecting".to_string(),
        PartyStatus::Joined => "joined".to_string(),
        PartyStatus::Rejected(r) => format!("rejected: {r}"),
        PartyStatus::Disconnected => "disconnected".to_string(),
    }
}

fn render_password(f: &mut Frame, app: &TuiApp, mode: PasswordMode, full: Rect) {
    let area = centered_rect(60, 40, full);
    let title = match mode {
        PasswordMode::Unlock => "Unlock identity",
        PasswordMode::Set => "Set a password",
    };
    let inner = clear_and_block(f, area, title);

    let prompt = match mode {
        PasswordMode::Unlock => {
            "Your identity is encrypted. Enter your password to unlock messaging and history."
        }
        PasswordMode::Set if app.new_password_first.is_some() => {
            "Type the same password again to confirm it. There is no way to recover it."
        }
        PasswordMode::Set => {
            "Protect your private key with a password (at least 12 characters). \
             You'll need it on next launch, and it cannot be recovered."
        }
    };
    let rows = Layout::vertical([
        Constraint::Min(2),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .split(inner);

    note(f, prompt, rows[0]);
    f.render_widget(
        Paragraph::new(app.password_field.display_text())
            .block(Block::default().borders(Borders::ALL).title("password"))
            .style(Style::default().fg(Color::Yellow)),
        rows[1],
    );
    hint(f, "Enter to confirm · Esc to cancel", rows[2]);
}

fn render_confirm(f: &mut Frame, title: &str, message: &str, full: Rect) {
    let area = centered_rect(50, 25, full);
    let inner = clear_and_block(f, area, title);
    f.render_widget(
        Paragraph::new(message)
            .wrap(Wrap { trim: false })
            .alignment(Alignment::Center),
        inner,
    );
}

pub fn toast_color(level: ToastLevel) -> Color {
    match level {
        ToastLevel::Info => Color::Cyan,
        ToastLevel::Success => Color::Green,
        ToastLevel::Warning => Color::Yellow,
        ToastLevel::Error => Color::Red,
    }
}

#[cfg(test)]
mod token_drift_tests {
    //! `design/tokens.json` is the source of record for brand colour but
    //! generates nothing; this is the Rust half of the drift guard (the CSS
    //! half is `desktop/src/lib/tokens.test.js`).
    use super::BRAND_ACCENT;
    use ratatui::style::Color;
    use std::path::Path;

    fn tokens() -> serde_json::Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../design/tokens.json");
        let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "design/tokens.json must be readable ({}): {e}",
                path.display()
            )
        });
        serde_json::from_str(&raw).expect("design/tokens.json must be valid JSON")
    }

    fn hex_to_rgb(hex: &str) -> (u8, u8, u8) {
        let h = hex.trim_start_matches('#');
        assert_eq!(h.len(), 6, "expected #rrggbb, got {hex}");
        (
            u8::from_str_radix(&h[0..2], 16).expect("red"),
            u8::from_str_radix(&h[2..4], 16).expect("green"),
            u8::from_str_radix(&h[4..6], 16).expect("blue"),
        )
    }

    #[test]
    fn tui_brand_accent_matches_design_tokens() {
        let hex = tokens()["brand"]["flatAccent"]
            .as_str()
            .expect("brand.flatAccent")
            .to_string();
        let (r, g, b) = hex_to_rgb(&hex);
        assert_eq!(
            BRAND_ACCENT,
            Color::Rgb(r, g, b),
            "TUI chrome accent drifted from design/tokens.json brand.flatAccent ({hex})"
        );
    }

    /// The safety grid is a security signal users compare across devices, so
    /// its palette must not depend on which front-end renders it.
    #[test]
    fn safety_grid_palette_is_shared_and_frozen() {
        use crate::colorgrid::PALETTE;
        assert_eq!(PALETTE.len(), 16, "the grid indexes bytes modulo 16");
        assert_eq!(PALETTE[0], (230, 25, 75));
        assert_eq!(PALETTE[15], (128, 128, 0));
    }
}
