//! Party server window for the GUI (Phase 1 Party tab).
//!
//! A modeless window that lets the user join a Party server (address + username +
//! optional password), then browse its channels, members, and messages and post to
//! a channel. Connection state lives in [`App::party_manager`]; this view renders a
//! snapshot each frame and issues requests through it.

use eframe::egui;
use uuid::Uuid;

use crate::app::party_manager::PartyStatus;
use crate::gui::app_ui::App;
use messenger_core::party::{dm_thread_id, ChannelInfo, MemberInfo, MessagePayload};

/// Render the Party window if open.
pub fn render_party_window(app: &mut App, ctx: &egui::Context) {
    if !app.show_party {
        return;
    }
    let mut open = true;
    egui::Window::new("🎉 Party Servers")
        .open(&mut open)
        .resizable(true)
        .default_width(620.0)
        .default_height(460.0)
        .show(ctx, |ui| render_body(app, ui));
    if !open {
        app.show_party = false;
    }
}

fn render_body(app: &mut App, ui: &mut egui::Ui) {
    // --- Join form ---
    ui.heading("Join a server");
    egui::Grid::new("party_connect_form")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Address");
            ui.text_edit_singleline(&mut app.party_address);
            ui.end_row();
            ui.label("Username");
            ui.text_edit_singleline(&mut app.party_username);
            ui.end_row();
            ui.label("Password (optional)");
            ui.add(egui::TextEdit::singleline(&mut app.party_password).password(true));
            ui.end_row();
        });
    let can_connect = !app.party_address.trim().is_empty() && !app.party_username.trim().is_empty();
    if ui
        .add_enabled(can_connect, egui::Button::new("Connect"))
        .clicked()
    {
        connect_clicked(app);
    }

    // Surface the last connection error (e.g. wrong password / unreachable host).
    let last_error = app
        .party_manager
        .try_lock()
        .ok()
        .and_then(|p| p.last_error().map(String::from));
    if let Some(err) = last_error {
        ui.horizontal(|ui| {
            ui.colored_label(egui::Color32::from_rgb(220, 80, 80), format!("⚠ {err}"));
            if ui.small_button("Dismiss").clicked() {
                if let Ok(mut p) = app.party_manager.try_lock() {
                    p.clear_last_error();
                }
            }
        });
    }

    ui.separator();

    // --- Snapshot the manager state for this frame ---
    let Some(snapshot) = snapshot(app) else {
        ui.label("Connecting…");
        return;
    };

    if snapshot.servers.is_empty() {
        ui.label("No servers joined yet. Fill in the form above and Connect.");
        return;
    }

    // Default the selected server to the first one.
    if app
        .party_selected_server
        .map(|id| !snapshot.servers.iter().any(|s| s.id == id))
        .unwrap_or(true)
    {
        app.party_selected_server = snapshot.servers.first().map(|s| s.id);
    }

    // --- Server selector ---
    ui.horizontal_wrapped(|ui| {
        for srv in &snapshot.servers {
            let label = format!("{} {}", status_icon(&srv.status), srv.display_name());
            ui.selectable_value(&mut app.party_selected_server, Some(srv.id), label);
        }
    });

    let Some(server_id) = app.party_selected_server else {
        return;
    };
    let Some(srv) = snapshot.servers.iter().find(|s| s.id == server_id) else {
        return;
    };

    ui.add_space(4.0);
    ui.label(format!("Status: {}", status_text(&srv.status)));
    ui.label(format!(
        "Server fingerprint (verify out-of-band): {}",
        short_fp(&srv.fingerprint)
    ));

    ui.separator();

    egui::SidePanel::left("party_channels")
        .resizable(false)
        .default_width(170.0)
        .show_inside(ui, |ui| {
            ui.strong("Channels");
            for ch in &srv.channels {
                let selected =
                    app.party_selected_dm.is_none() && app.party_selected_channel == Some(ch.id);
                if ui
                    .selectable_label(selected, format!("# {}", ch.name))
                    .clicked()
                {
                    app.party_selected_channel = Some(ch.id);
                    app.party_selected_dm = None;
                }
            }
            // Create a new channel.
            ui.horizontal(|ui| {
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut app.party_new_channel_input)
                        .hint_text("new channel")
                        .desired_width(110.0),
                );
                let submit = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (submit || ui.button("+").clicked())
                    && !app.party_new_channel_input.trim().is_empty()
                {
                    let name = std::mem::take(&mut app.party_new_channel_input);
                    if let Ok(party) = app.party_manager.try_lock() {
                        if let Err(e) = party.create_channel(server_id, name) {
                            tracing::warn!("Create channel failed: {}", e);
                        }
                    }
                }
            });

            ui.add_space(8.0);
            ui.strong(format!("Members ({}) — click to DM", srv.members.len()));
            for m in &srv.members {
                let dot = if m.online { "🟢" } else { "⚪" };
                if srv.my_member == Some(m.id) {
                    ui.label(format!("{dot} {} (you)", m.username));
                    continue;
                }
                let selected = app.party_selected_dm == Some(m.id);
                if ui
                    .selectable_label(selected, format!("{dot} ✉ {}", m.username))
                    .clicked()
                {
                    let newly_selected = app.party_selected_dm != Some(m.id);
                    app.party_selected_dm = Some(m.id);
                    if newly_selected {
                        // Seed the DM thread from durable history on first open.
                        if let Ok(party) = app.party_manager.try_lock() {
                            let _ = party.fetch_dm_history(server_id, m.id);
                        }
                    }
                }
            }
        });

    // Default the selected channel to the first one of this server.
    if app
        .party_selected_channel
        .map(|id| !srv.channels.iter().any(|c| c.id == id))
        .unwrap_or(true)
    {
        app.party_selected_channel = srv.channels.first().map(|c| c.id);
    }

    egui::CentralPanel::default().show_inside(ui, |ui| {
        // DM view takes precedence over the channel view when a member is selected.
        if let Some(peer) = app.party_selected_dm {
            ui.strong(format!("✉ Direct messages with {}", srv.username(peer)));
            let thread = srv.my_member.map(|me| dm_thread_id(me, peer));
            render_message_list(ui, srv, thread.as_ref());
            ui.separator();
            ui.horizontal(|ui| {
                let resp = ui.text_edit_singleline(&mut app.party_post_input);
                let submit = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let clicked = ui.button("Send").clicked();
                if (submit || clicked) && !app.party_post_input.trim().is_empty() {
                    dm_send_clicked(app, server_id, peer);
                }
            });
            return;
        }

        let Some(channel) = app.party_selected_channel else {
            ui.label("No channel selected.");
            return;
        };
        render_message_list(ui, srv, Some(&channel));
        ui.separator();
        ui.horizontal(|ui| {
            let resp = ui.text_edit_singleline(&mut app.party_post_input);
            let submit = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let clicked = ui.button("Send").clicked();
            if (submit || clicked) && !app.party_post_input.trim().is_empty() {
                post_clicked(app, server_id, channel);
            }
        });
    });
}

/// Render the scrollable message list for a channel or DM thread.
fn render_message_list(ui: &mut egui::Ui, srv: &ServerSnapshot, thread: Option<&Uuid>) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .max_height(ui.available_height() - 40.0)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            let empty = Vec::new();
            let msgs = thread.and_then(|t| srv.messages.get(t)).unwrap_or(&empty);
            if msgs.is_empty() {
                ui.weak("No messages yet.");
            }
            for (sender, text) in msgs {
                let name = srv.username(*sender);
                ui.horizontal_wrapped(|ui| {
                    ui.strong(format!("{name}:"));
                    ui.label(text);
                });
            }
        });
}

fn dm_send_clicked(app: &mut App, server_id: Uuid, peer: Uuid) {
    let text = std::mem::take(&mut app.party_post_input);
    if let Ok(mut party) = app.party_manager.try_lock() {
        if let Err(e) = party.send_dm(server_id, peer, text) {
            tracing::warn!("Party DM failed: {}", e);
        }
    }
}

/// Spawn the async connect+join, mirroring the chat-manager connect pattern.
fn connect_clicked(app: &mut App) {
    let privkey = match app.identity.private_key() {
        Ok(k) => k,
        Err(e) => {
            tracing::error!("Party connect: cannot access private key: {}", e);
            return;
        }
    };
    let mgr = app.party_manager.clone();
    let address = app.party_address.trim().to_string();
    let username = app.party_username.trim().to_string();
    let password = if app.party_password.is_empty() {
        None
    } else {
        Some(app.party_password.clone())
    };
    app.party_password.clear();
    tokio::spawn(async move {
        let mut m = mgr.lock().await;
        if let Err(e) = m
            .connect_and_join(&address, &username, password, &privkey)
            .await
        {
            tracing::warn!("Party connect to {} failed: {}", address, e);
            m.set_last_error(format!("Connect to {address} failed: {e}"));
        }
    });
}

fn post_clicked(app: &mut App, server_id: Uuid, channel: Uuid) {
    let text = std::mem::take(&mut app.party_post_input);
    if let Ok(mut party) = app.party_manager.try_lock() {
        if let Err(e) = party.post(server_id, channel, text) {
            tracing::warn!("Party post failed: {}", e);
        }
    }
}

// --- Render snapshot (cloned out of the manager under a short lock) ---

struct ServerSnapshot {
    id: Uuid,
    name: String,
    fingerprint: String,
    status: PartyStatus,
    /// This client's member id on the server (once joined); needed to resolve DM
    /// thread ids.
    my_member: Option<Uuid>,
    channels: Vec<ChannelInfo>,
    members: Vec<MemberInfo>,
    /// Per channel/DM-thread: (sender id, text) pairs in display order.
    messages: std::collections::HashMap<Uuid, Vec<(Uuid, String)>>,
}

impl ServerSnapshot {
    fn username(&self, id: Uuid) -> String {
        self.members
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.username.clone())
            .unwrap_or_else(|| short_id(&id))
    }
}

impl ServerSnapshot {
    fn display_name(&self) -> String {
        if self.name.is_empty() {
            self.address_fallback()
        } else {
            self.name.clone()
        }
    }
    fn address_fallback(&self) -> String {
        // Name not yet known (pre-join); fall back to a short id.
        short_id(&self.id)
    }
}

struct Snapshot {
    servers: Vec<ServerSnapshot>,
}

/// Try to read the manager; returns `None` if it's momentarily locked (e.g. a
/// connect is in progress), so the caller can show a transient "Connecting…".
fn snapshot(app: &App) -> Option<Snapshot> {
    let party = app.party_manager.try_lock().ok()?;
    let servers = party
        .server_ids()
        .into_iter()
        .filter_map(|id| {
            let conn = party.server(id)?;
            let messages = conn
                .messages
                .iter()
                .map(|(ch, msgs)| {
                    let rendered = msgs
                        .iter()
                        .map(|env| {
                            let MessagePayload::Text(t) = &env.payload;
                            (env.sender, t.clone())
                        })
                        .collect::<Vec<_>>();
                    (*ch, rendered)
                })
                .collect();
            Some(ServerSnapshot {
                id,
                name: conn.server_name.clone(),
                fingerprint: conn.server_fingerprint.clone(),
                status: conn.status.clone(),
                my_member: conn.member_id,
                channels: conn.channels.clone(),
                members: conn.members.clone(),
                messages,
            })
        })
        .collect();
    Some(Snapshot { servers })
}

fn status_icon(status: &PartyStatus) -> &'static str {
    match status {
        PartyStatus::Connecting => "🟡",
        PartyStatus::Joined => "🟢",
        PartyStatus::Rejected(_) => "🔴",
        PartyStatus::Disconnected => "⚪",
    }
}

fn status_text(status: &PartyStatus) -> String {
    match status {
        PartyStatus::Connecting => "connecting…".to_string(),
        PartyStatus::Joined => "joined".to_string(),
        PartyStatus::Rejected(reason) => format!("rejected: {reason}"),
        PartyStatus::Disconnected => "disconnected".to_string(),
    }
}

fn short_fp(fp: &str) -> String {
    if fp.len() > 16 {
        format!("{}…", &fp[..16])
    } else {
        fp.to_string()
    }
}

fn short_id(id: &Uuid) -> String {
    id.to_string().chars().take(8).collect()
}
