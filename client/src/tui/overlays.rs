//! Modal overlays for the TUI.
//!
//! Overlays sit on top of the normal chat layout. While one is open, key events
//! route to it first (see `TuiApp::handle_key_event`). Every overlay has an
//! equivalent `:`-command so the app stays fully keyboard/command driven.

use crate::tui::app::TuiApp;
use crate::tui::command::{command_help, settings_keys, COMMANDS};
use crate::types::{ToastLevel, TransferStatus};
use ratatui::{prelude::*, widgets::*};
use uuid::Uuid;

/// What a password overlay is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordMode {
    /// Unlock an existing encrypted identity.
    Unlock,
    /// Set a password on a new/unencrypted identity.
    Set,
}

/// The currently active overlay (if any).
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
    },
    Contacts,
    Settings,
    Invite {
        link: String,
    },
    Identity,
    Transfers,
    /// The Communities pane: servers, channels, members with their roles, and
    /// the files shared where you can see them.
    Party,
    Password {
        mode: PasswordMode,
    },
    ConfirmQuit,
    ConfirmClearHistory,
}

impl TuiOverlay {
    pub fn is_open(&self) -> bool {
        !matches!(self, TuiOverlay::None)
    }
}

/// Compute a centered rectangle `pct_x` × `pct_y` percent of `area`.
pub fn centered_rect(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - pct_y) / 2),
            Constraint::Percentage(pct_y),
            Constraint::Percentage((100 - pct_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - pct_x) / 2),
            Constraint::Percentage(pct_x),
            Constraint::Percentage((100 - pct_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn ratatui_color(c: crate::colorgrid::Rgb) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

/// Brand-neutral chrome accent ("control teal-indigo", see design/tokens.json).
/// Used only for UI chrome (borders, key-hint labels) that previously used an
/// arbitrary `Color::Cyan` — semantic colors (success/warning/error/info) are
/// untouched, since they carry meaning rather than brand.
pub(crate) const BRAND_ACCENT: Color = Color::Rgb(0x3e, 0x8d, 0xd2);

/// Render the active overlay over `area`. No-op when none is open.
pub fn render_overlay(f: &mut Frame, app: &mut TuiApp, area: Rect) {
    match app.overlay.clone() {
        TuiOverlay::None => {}
        TuiOverlay::Help => render_help(f, app, area),
        TuiOverlay::FingerprintVerify {
            fingerprint,
            peer_name,
            sas,
            ..
        } => render_fingerprint(f, &fingerprint, &peer_name, &sas, area),
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
        TuiOverlay::ConfirmClearHistory => render_confirm(
            f,
            "Clear all history?",
            "This erases ALL chats and contacts. (y = erase, n = cancel)",
            area,
        ),
    }
}

fn clear_and_block(f: &mut Frame, area: Rect, title: &str) -> Rect {
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(BRAND_ACCENT))
        .title(Span::styled(
            format!(" {} ", title),
            Style::default().add_modifier(Modifier::BOLD),
        ));
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
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(desc));
            lines.push(Line::from(""));
            lines.push(Line::from(format!("Command name: {}", name)));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Use :help for the full command list · Esc to close",
                Style::default().fg(Color::DarkGray),
            )));
        }
    } else {
        lines.push(Line::from(Span::styled(
            "Keys",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
        for (k, v) in [
            ("Tab", "cycle focus (chats / messages / input)"),
            (":", "enter command mode"),
            ("Enter", "send message (input) / submit command"),
            ("Ctrl+J", "newline in the message input"),
            ("↑/↓", "select chat / scroll messages / command history"),
            ("PgUp/PgDn", "scroll messages"),
            ("Ctrl+L", "copy logs to clipboard"),
            ("y / n", "accept / reject in confirm & verify overlays"),
            ("Esc", "close overlay / leave command mode"),
        ] {
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<10}", k), Style::default().fg(BRAND_ACCENT)),
                Span::raw(v),
            ]));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Chat markers",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(
            "  H hosting   ● connected   ○ offline   * unread",
        ));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Commands",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
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
        lines.push(Line::from(Span::styled(
            "Esc or :help to close · ↑/↓ to scroll",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let para = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((app.overlay_scroll, 0));
    f.render_widget(para, inner);
}

fn render_fingerprint(f: &mut Frame, fingerprint: &str, peer_name: &str, sas: &str, full: Rect) {
    let area = centered_rect(70, 70, full);
    let inner = clear_and_block(f, area, "🛡 Verify peer identity");

    let has_sas = !sas.is_empty();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),                           // heading
            Constraint::Length(2),                           // instruction
            Constraint::Length(if has_sas { 3 } else { 0 }), // SAS
            Constraint::Min(6),                              // color grid
            Constraint::Length(2),                           // fingerprint hex
            Constraint::Length(1),                           // actions
        ])
        .split(inner);

    f.render_widget(
        Paragraph::new(format!("Connecting to {}", peer_name))
            .style(Style::default().add_modifier(Modifier::BOLD)),
        rows[0],
    );

    // The short authentication string leads: both peers see the same code,
    // and an interposed MITM makes the two ends differ. Reading it aloud is
    // the low-friction check; the safety grid / hex stays as the backstop.
    if has_sas {
        f.render_widget(
            Paragraph::new("Read this code aloud with your peer — it must match on both ends:")
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(Color::Gray)),
            rows[1],
        );
        f.render_widget(
            Paragraph::new(sas.to_string())
                .alignment(ratatui::layout::Alignment::Center)
                .style(
                    Style::default()
                        .fg(BRAND_ACCENT)
                        .add_modifier(Modifier::BOLD),
                ),
            rows[2],
        );
    } else {
        f.render_widget(
            Paragraph::new(
                "Confirm this safety grid / fingerprint matches your peer's, via a separate channel.",
            )
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(Color::Gray)),
            rows[1],
        );
    }

    // Safety grid — the same frozen palette every front-end renders, so the
    // shape a user compares is identical in the terminal and the desktop app.
    let grid = crate::colorgrid::generate_color_grid(fingerprint);
    let grid_lines: Vec<Line> = grid
        .iter()
        .map(|row| {
            Line::from(
                row.iter()
                    .map(|c| Span::styled("███ ", Style::default().fg(ratatui_color(*c))))
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    f.render_widget(Paragraph::new(grid_lines), rows[3]);

    f.render_widget(
        Paragraph::new(fingerprint.to_string())
            .wrap(Wrap { trim: true })
            .style(Style::default().fg(Color::White)),
        rows[4],
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" y ", Style::default().bg(Color::Green).fg(Color::Black)),
            Span::raw(" accept   "),
            Span::styled(" n ", Style::default().bg(Color::Red).fg(Color::White)),
            Span::raw(" reject   (or :verify accept|reject)"),
        ])),
        rows[5],
    );
}

fn render_contacts(f: &mut Frame, app: &mut TuiApp, full: Rect) {
    let area = centered_rect(70, 70, full);
    let inner = clear_and_block(f, area, "Contacts");

    if app.contact_ids.is_empty() {
        f.render_widget(
            Paragraph::new(
                "No contacts yet.\n\nAdd one with :contact-add <name> <host:port> [fingerprint]\nor import an invite with :import <link>.",
            )
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(Color::Gray)),
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

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    f.render_stateful_widget(list, inner, &mut app.contacts_list_state);
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
        format!("mdns           {}", on_off(cfg.enable_mdns)),
        format!("upnp           {}", on_off(cfg.enable_upnp)),
        format!("theme          {:?}", cfg.theme),
    ];
    let items: Vec<ListItem> = rows.iter().map(|r| ListItem::new(r.clone())).collect();
    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    f.render_stateful_widget(list, inner, &mut app.settings_list_state);

    let _ = settings_keys();
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
            Line::from(Span::styled(
                "Esc to close",
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .wrap(Wrap { trim: false }),
        inner,
    );
}

fn render_identity(f: &mut Frame, app: &TuiApp, full: Rect) {
    let area = centered_rect(70, 70, full);
    let inner = clear_and_block(f, area, "Your identity (fingerprint copied)");

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(6),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(inner);

    f.render_widget(
        Paragraph::new(format!("Name: {}", app.identity_name))
            .style(Style::default().add_modifier(Modifier::BOLD)),
        rows[0],
    );
    let grid = crate::colorgrid::generate_color_grid(&app.identity_fingerprint());
    let grid_lines: Vec<Line> = grid
        .iter()
        .map(|row| {
            Line::from(
                row.iter()
                    .map(|c| Span::styled("███ ", Style::default().fg(ratatui_color(*c))))
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    f.render_widget(Paragraph::new(grid_lines), rows[1]);
    f.render_widget(
        Paragraph::new(app.identity_fingerprint())
            .wrap(Wrap { trim: true })
            .style(Style::default().fg(Color::White)),
        rows[2],
    );
    f.render_widget(
        Paragraph::new(Span::styled(
            "Esc to close",
            Style::default().fg(Color::DarkGray),
        )),
        rows[3],
    );
}

fn render_transfers(f: &mut Frame, app: &TuiApp, full: Rect) {
    let area = centered_rect(70, 60, full);
    let inner = clear_and_block(f, area, "File transfers");

    let transfers = app.chat_manager.active_transfers_sorted();
    if transfers.is_empty() {
        f.render_widget(
            Paragraph::new("No active transfers.\n\nSend a file with :send <path>.")
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(Color::Gray)),
            inner,
        );
        return;
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

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
            let style = if i == sel {
                Style::default()
                    .fg(BRAND_ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Line::styled(text, style)
        })
        .collect();
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), rows[0]);
    f.render_widget(
        Paragraph::new(Span::styled(
            "↑/↓ select · c cancel · Esc close",
            Style::default().fg(Color::DarkGray),
        )),
        rows[1],
    );
}

/// The Communities pane: the servers you have joined, and for the selected one
/// its channels (with their access rule), members (with role and presence), and
/// the files shared where you can see them.
///
/// The commands remain the way to *act*; this is the surface that shows the
/// state they act on, which command output could only ever print one slice of.
fn render_party(f: &mut Frame, app: &TuiApp, full: Rect) {
    let area = centered_rect(84, 76, full);
    let inner = clear_and_block(f, area, "Communities");

    let servers = app.party_manager.server_ids();
    if servers.is_empty() {
        f.render_widget(
            Paragraph::new(
                "You have not joined any communities.\n\n\
                 Join one with:\n  \
                 :party-connect <host[:port]> <username> [password]\n\n\
                 The first time you connect to an address you will be shown the \
                 server's code to check with its operator; run :party-trust to \
                 accept it. Nothing — not your username, not the password — is \
                 sent before you do.",
            )
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(Color::Gray)),
            inner,
        );
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let sel = app.party_sel.min(servers.len().saturating_sub(1));
    // Server tabs across the top, so it is obvious which one the rest describes.
    let tabs: Vec<Span> = servers
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let conn = app.party_manager.server(*id);
            let name = conn
                .map(|c| {
                    if c.server_name.is_empty() {
                        c.address.clone()
                    } else {
                        c.server_name.clone()
                    }
                })
                .unwrap_or_else(|| "?".to_string());
            let style = if i == sel {
                Style::default()
                    .fg(BRAND_ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            Span::styled(format!(" {} ", name), style)
        })
        .collect();
    f.render_widget(Paragraph::new(Line::from(tabs)), rows[0]);

    let Some(conn) = servers
        .get(sel)
        .and_then(|id| app.party_manager.server(*id))
    else {
        return;
    };

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(34), Constraint::Percentage(66)])
        .split(rows[1]);

    // Left: channels and members.
    let mut left: Vec<Line> = Vec::new();
    left.push(Line::styled(
        format!("{}  ({})", conn.address, party_status_label(&conn.status)),
        Style::default().fg(Color::DarkGray),
    ));
    left.push(Line::raw(""));
    left.push(Line::styled(
        "CHANNELS",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    ));
    for c in &conn.channels {
        // The kind is the access rule, so it belongs next to the name rather
        // than being something you have to remember per channel.
        let suffix = match c.kind {
            messenger_core::party::ChannelKind::Public => String::new(),
            other => format!("  ({})", other.label().to_lowercase()),
        };
        left.push(Line::raw(format!("  #{}{}", c.name, suffix)));
    }
    left.push(Line::raw(""));
    left.push(Line::styled(
        "MEMBERS",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    ));
    for m in &conn.members {
        let dot = if m.online { "●" } else { "○" };
        let me = if Some(m.id) == conn.member_id {
            " (you)"
        } else {
            ""
        };
        let role = if m.role == messenger_core::party::Role::Member {
            String::new()
        } else {
            format!("  [{}]", m.role.label().to_lowercase())
        };
        let style = if m.online {
            Style::default()
        } else {
            Style::default().fg(Color::DarkGray)
        };
        left.push(Line::styled(
            format!("  {} {}{}{}", dot, m.username, me, role),
            style,
        ));
    }
    f.render_widget(Paragraph::new(left).wrap(Wrap { trim: false }), cols[0]);

    // Right: shared files and storage.
    let mut right: Vec<Line> = Vec::new();
    right.push(Line::styled(
        "SHARED FILES",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    ));
    if conn.files.is_empty() {
        right.push(Line::styled(
            "  none visible — :party-files refreshes this",
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        for file in conn.files.iter().take(14) {
            right.push(Line::raw(format!(
                "  {:<24} {:>9}  {} · {}",
                truncate(&file.name, 24),
                crate::util::format_size(file.size),
                file.uploader_name,
                file.location_name
            )));
        }
        if conn.files.len() > 14 {
            right.push(Line::styled(
                format!("  … and {} more", conn.files.len() - 14),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }
    if let Some(q) = conn.quota {
        right.push(Line::raw(""));
        let used = crate::util::format_size(q.used);
        let line = match q.limit {
            Some(limit) => format!(
                "  You: {} of {}   Server: {} of {}",
                used,
                crate::util::format_size(limit),
                crate::util::format_size(q.server_used),
                crate::util::format_size(q.server_limit)
            ),
            None => format!(
                "  You: {} (no personal limit)   Server: {} of {}",
                used,
                crate::util::format_size(q.server_used),
                crate::util::format_size(q.server_limit)
            ),
        };
        right.push(Line::styled(line, Style::default().fg(Color::DarkGray)));
    }
    if let Some(err) = &conn.last_error {
        right.push(Line::raw(""));
        right.push(Line::styled(
            format!("  {}", err),
            Style::default().fg(Color::Red),
        ));
    }
    f.render_widget(Paragraph::new(right).wrap(Wrap { trim: false }), cols[1]);

    f.render_widget(
        Paragraph::new(Span::styled(
            "←/→ switch community · :party-files refresh · :party-role · :party-channel-access · Esc close",
            Style::default().fg(Color::DarkGray),
        )),
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

/// Trim to `max` display columns, marking the cut so a truncated name does not
/// read as the whole name.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
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
        PasswordMode::Set => {
            "Protect your private key with a password. You'll need it on next launch."
        }
    };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(2),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(inner);

    f.render_widget(
        Paragraph::new(prompt)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(Color::Gray)),
        rows[0],
    );
    f.render_widget(
        Paragraph::new(app.password_field.display_text())
            .block(Block::default().borders(Borders::ALL).title("password"))
            .style(Style::default().fg(Color::Yellow)),
        rows[1],
    );
    f.render_widget(
        Paragraph::new(Span::styled(
            "Enter to confirm · Esc to cancel",
            Style::default().fg(Color::DarkGray),
        )),
        rows[2],
    );
}

fn render_confirm(f: &mut Frame, title: &str, message: &str, full: Rect) {
    let area = centered_rect(50, 25, full);
    let inner = clear_and_block(f, area, title);
    f.render_widget(
        Paragraph::new(message)
            .wrap(Wrap { trim: false })
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::White)),
        inner,
    );
}

/// Severity → ratatui color for toasts.
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
    //! `design/tokens.json` is the source of record for brand colour, but
    //! nothing generates code from it. This is the Rust half of the drift
    //! guard (the desktop CSS half is `desktop/src/lib/tokens.test.js`); it
    //! replaces the equivalent check that lived in the egui theming module
    //! before that was retired.
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
