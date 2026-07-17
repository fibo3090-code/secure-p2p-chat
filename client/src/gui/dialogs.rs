use crate::colorgrid::generate_color_grid;
use crate::gui::app_ui::{ActiveDialog, App};
use crate::gui::widgets::ColorGrid;
use crate::util::primary_local_ipv4;
use anyhow::anyhow;
use eframe::egui;
use egui_tracing::ui::Logs;

fn queue_history_save(history_path: std::path::PathBuf, manager: &mut crate::app::ChatManager) {
    let snapshot = match manager.history_snapshot() {
        Ok(snapshot) => snapshot,
        Err(e) => {
            manager.add_toast(
                crate::types::ToastLevel::Error,
                format!("Failed to prepare save: {}", e),
            );
            return;
        }
    };

    tokio::spawn(async move {
        let (history, key) = snapshot;
        match tokio::task::spawn_blocking(move || history.save_encrypted(&history_path, &key)).await
        {
            Ok(Err(e)) => tracing::warn!("Background settings save failed: {}", e),
            Err(e) => tracing::warn!("Background settings save task failed: {}", e),
            Ok(Ok(())) => {}
        }
    });
}

pub fn render_dialogs(app: &mut App, ctx: &egui::Context) {
    // NOTE: When identity.is_locked() || is_new_identity || force_password_setup,
    // update() shows only render_blocking_auth_screen and returns before calling render_dialogs.
    // So we never reach here in the blocking state. The set_password and unlock dialogs
    // are only used from the blocking auth screen in that case (except when called explicitly).

    match app.active_dialog {
        ActiveDialog::Welcome => render_welcome(app, ctx),
        ActiveDialog::DeleteChat => {
            if let Some(chat_id) = app.chat_to_delete {
                render_delete_confirmation(app, ctx, chat_id);
            } else {
                app.active_dialog = ActiveDialog::None;
            }
        }
        ActiveDialog::Host => render_host_dialog(app, ctx),
        ActiveDialog::Connect => render_connect_dialog(app, ctx),
        ActiveDialog::Contacts => render_contacts_window(app, ctx),
        ActiveDialog::AddContact => render_add_contact_dialog(app, ctx),
        ActiveDialog::CreateGroup => render_create_group_wizard(app, ctx),
        ActiveDialog::RenameChat => render_rename_dialog(app, ctx),
        ActiveDialog::Settings => render_settings_dialog(app, ctx),
        ActiveDialog::About => crate::gui::help_view::render_help_window(app, ctx),
        ActiveDialog::FingerprintVerification => render_fingerprint_dialog(app, ctx),
        ActiveDialog::Password => render_password_dialog(app, ctx),
        ActiveDialog::SetPassword => render_set_password_dialog(app, ctx),
        ActiveDialog::ClearHistory => render_clear_history_dialog(app, ctx),
        ActiveDialog::None => {}
    }

    if app.show_log_terminal {
        render_log_terminal(app, ctx);
    }
}

/// Full-screen blocking auth: unlock (password) or set password.
/// Shown when identity.is_locked() || is_new_identity || force_password_setup.
/// No other UI (sidebar, chats, menus) is visible; user cannot bypass.
pub fn render_blocking_auth_screen(app: &mut App, ctx: &egui::Context) {
    egui::CentralPanel::default()
        .frame(egui::Frame::default().fill(ctx.style().visuals.window_fill()))
        .show(ctx, |ui| {
            ui.vertical_centered_justified(|ui| {
                ui.add_space(60.0);
                ui.heading(
                    egui::RichText::new("Encrypted P2P Messenger")
                        .size(24.0)
                        .strong(),
                );
                ui.add_space(8.0);

                if app.identity.is_locked() {
                    ui.colored_label(
                        egui::Color32::GRAY,
                        "Enter your password to unlock and continue.",
                    );
                    ui.add_space(24.0);
                    render_unlock_form(app, ui);
                } else {
                    ui.colored_label(
                        egui::Color32::GRAY,
                        "Set a password to secure your identity. You must do this to continue.",
                    );
                    ui.add_space(24.0);
                    render_set_password_form(app, ui, false); // false = no Cancel when blocking
                }
            });
        });
}

fn render_unlock_form(app: &mut App, ui: &mut egui::Ui) {
    ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
        ui.set_max_width(320.0);
        let response = ui.add(
            egui::TextEdit::singleline(&mut app.password_input)
                .password(true)
                .hint_text("Password"),
        );
        ui.add_space(12.0);
        if crate::gui::widgets::primary_button(ui, "🔓 Unlock").clicked()
            || (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
        {
            match app.identity.decrypt(&app.password_input) {
                Ok(_) => {
                    app.identity_locked = false;
                    app.active_dialog = ActiveDialog::None;
                    app.password_input.clear();

                    // Derive and set history key from unlocked identity
                    if let Ok(history_key) = app.identity.history_key() {
                        // Scope for lock
                        {
                            if let Ok(mut manager) = app.chat_manager.try_lock() {
                                manager.set_history_key(history_key);

                                // Try to load history if not already loaded
                                if !app.history_loaded {
                                    match manager.load_history_auto(&app.history_path, &history_key)
                                    {
                                        Ok(_) => {
                                            tracing::info!("History loaded and unlocked");
                                            app.history_loaded = true;
                                        }
                                        Err(e) => {
                                            tracing::warn!("Failed to load history: {}", e);
                                        }
                                    }
                                }
                            }
                        }
                        // Lock dropped, now we can safely use app
                        if let Ok(mut manager) = app.chat_manager.try_lock() {
                            manager.add_toast(
                                crate::types::ToastLevel::Success,
                                "Identity unlocked!".to_string(),
                            );
                        }
                    } else if let Ok(mut manager) = app.chat_manager.try_lock() {
                        manager.add_toast(
                            crate::types::ToastLevel::Warning,
                            "Unlocked but could not derive history key. History may not auto-save."
                                .to_string(),
                        );
                    }
                }
                Err(e) => {
                    if let Ok(mut manager) = app.chat_manager.try_lock() {
                        manager.add_toast(
                            crate::types::ToastLevel::Error,
                            format!("Wrong password: {}", e),
                        );
                    }
                }
            }
        }
    });
}

/// Inner form for set/change password. `allow_cancel`: show enabled Cancel (e.g. from Settings).
fn render_set_password_form(app: &mut App, ui: &mut egui::Ui, allow_cancel: bool) {
    ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
        ui.set_max_width(340.0);
        if app.is_new_identity {
            ui.label("Welcome! Create a password to secure your new identity.");
        } else {
            ui.label("Set a password to encrypt your identity file.");
        }
        ui.label(
            egui::RichText::new("If you forget it, you cannot recover your identity.")
                .small()
                .weak(),
        );
        ui.add_space(12.0);

        ui.label("New Password:");
        ui.add(
            egui::TextEdit::singleline(&mut app.new_password_input)
                .password(true)
                .hint_text("New Password"),
        );
        ui.label("Confirm Password:");
        ui.add(
            egui::TextEdit::singleline(&mut app.confirm_password_input)
                .password(true)
                .hint_text("Confirm Password"),
        );
        ui.add_space(8.0);

        if app.new_password_input != app.confirm_password_input
            && !app.confirm_password_input.is_empty()
        {
            ui.colored_label(crate::gui::styling::ERROR, "Passwords do not match.");
        } else if app.new_password_input.is_empty() {
            ui.colored_label(
                crate::gui::styling::SUBTLE_TEXT_COLOR,
                "Enter and confirm a password.",
            );
        }

        ui.horizontal(|ui| {
            let can_set = !app.new_password_input.is_empty()
                && app.new_password_input == app.confirm_password_input;
            if ui
                .add_enabled(can_set, egui::Button::new("Set Password"))
                .clicked()
            {
                match app.identity.encrypt(&app.new_password_input) {
                    Ok(_) => {
                        let path = app.history_path.with_file_name("identity.json");
                        if let Err(e) = app.identity.save(&path) {
                            if let Ok(mut m) = app.chat_manager.try_lock() {
                                m.add_toast(
                                    crate::types::ToastLevel::Error,
                                    format!("Failed to save: {}", e),
                                );
                            }
                        } else {
                            if let Ok(mut m) = app.chat_manager.try_lock() {
                                m.add_toast(
                                    crate::types::ToastLevel::Success,
                                    "Password set. Unlocking…".to_string(),
                                );
                            }
                            // Decrypt in memory so we stay unlocked for this session
                            if app.identity.decrypt(&app.new_password_input).is_ok() {
                                app.identity_locked = false;

                                // Derive and set history key from unlocked identity
                                if let Ok(history_key) = app.identity.history_key() {
                                    if let Ok(mut m) = app.chat_manager.try_lock() {
                                        m.set_history_key(history_key);
                                    }
                                }
                            }
                            if app.is_new_identity {
                                app.is_new_identity = false;
                            }
                            app.force_password_setup = false;
                            app.active_dialog = ActiveDialog::None;
                            app.new_password_input.clear();
                            app.confirm_password_input.clear();
                        }
                    }
                    Err(e) => {
                        if let Ok(mut m) = app.chat_manager.try_lock() {
                            m.add_toast(
                                crate::types::ToastLevel::Error,
                                format!("Encryption failed: {}", e),
                            );
                        }
                    }
                }
            }
            if allow_cancel && ui.button("Cancel").clicked() {
                app.active_dialog = ActiveDialog::None;
                app.new_password_input.clear();
                app.confirm_password_input.clear();
            }
        });
    });
}

fn render_fingerprint_dialog(app: &mut App, ctx: &egui::Context) {
    if let (Some(fingerprint), Some(peer_name), Some(chat_id)) = (
        app.fingerprint_to_verify.as_ref(),
        app.peer_name_to_verify.as_ref(),
        app.chat_id_to_verify,
    ) {
        egui::Window::new("🛡️ Verify Peer Identity")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.heading(format!("Connecting to {}", peer_name));
                ui.add_space(10.0);
                ui.label("Please verify that the fingerprint below matches the one provided by your peer.");
                ui.add_space(10.0);

                let grid = generate_color_grid(fingerprint);
                ui.add(ColorGrid::new(grid));

                ui.add_space(10.0);
                ui.monospace(fingerprint);
                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    if crate::gui::widgets::primary_button(ui, "✅ Accept").clicked() {
                        if let Ok(mut manager) = app.chat_manager.try_lock() {
                            // Notify session/task that the fingerprint is accepted
                            let _ = manager.confirm_fingerprint(chat_id, true);
                            // Store fingerprint in chat record for future reference
                            if let Some(chat) = manager.chats.get_mut(&chat_id) {
                                chat.peer_fingerprint = Some(fingerprint.clone());
                            }
                            manager.add_toast(crate::types::ToastLevel::Success, "Fingerprint accepted".to_string());
                        }
                        app.active_dialog = ActiveDialog::None;
                    }
                    if crate::gui::widgets::secondary_button(ui, "❌ Reject").clicked() {
                        if let Ok(mut manager) = app.chat_manager.try_lock() {
                            // Notify session/task that the fingerprint is rejected so it can abort
                            let _ = manager.confirm_fingerprint(chat_id, false);
                            // Remove chat locally
                            manager.delete_chat(chat_id);
                        }
                        app.active_dialog = ActiveDialog::None;
                    }
                });
            });
    }
}

fn render_welcome(app: &mut App, ctx: &egui::Context) {
    egui::Window::new("🎉 Welcome to Encrypted P2P Messenger!")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(500.0);

            ui.vertical_centered(|ui| {
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new("Secure, Private, Peer-to-Peer Messaging")
                        .size(18.0)
                        .strong(),
                );
                ui.add_space(10.0);
            });

            ui.separator();
            ui.add_space(10.0);

            ui.heading("✨ Features:");
            ui.add_space(5.0);

            ui.label("🔒 End-to-end encryption with RSA-2048 & AES-256-GCM");
            ui.label("🔐 Forward secrecy with X25519 ECDH (protects past messages)");
            ui.label("📡 Local Peer Discovery: Automatically find peers nearby");
            ui.label("📁 Secure file transfer with progress tracking");
            ui.label("👥 Direct peer-to-peer connections (no server!)");
            ui.label("🛡️ Fingerprint verification & Trust-on-First-Use (TOFU)");
            ui.label("💾 Message history persistence");
            ui.label("😊 Emoji picker, typing indicators, desktop notifications");

            ui.add_space(15.0);
            ui.separator();
            ui.add_space(10.0);

            ui.heading("🚀 Getting Started:");
            ui.add_space(5.0);

            ui.label("1️⃣ Host Mode: Start hosting to accept connections");
            ui.label("   • Click 'Connection' → 'Start Host'");
            ui.label("   • You'll automatically appear to others on your local network!");
            ui.add_space(5.0);

            ui.label("2️⃣ Client Mode: Connect to someone");
            ui.label("   • Click 'Connection' → 'Connect to Host'");
            ui.label("   • Pick a peer from the list or enter an address manually");
            ui.add_space(5.0);

            ui.label("3️⃣ Security: Trust on First Use");
            ui.label("   • The first time you connect, we save the peer's fingerprint.");
            ui.label("   • If it changes later, we'll warn you!");

            ui.add_space(15.0);
            ui.separator();
            ui.add_space(10.0);

            ui.vertical_centered(|ui| {
                if ui
                    .button(egui::RichText::new("Let's Get Started! 🚀").size(16.0))
                    .clicked()
                {
                    app.active_dialog = ActiveDialog::None;
                }
                ui.add_space(5.0);
                if ui.small_button("Show this again later").clicked() {
                    app.active_dialog = ActiveDialog::None;
                }
            });

            ui.add_space(10.0);
        });
}

pub fn render_toasts(app: &mut App, ctx: &egui::Context) {
    // Combine app-level and manager-level toasts
    let mut all_toasts = app.toasts.clone();
    if let Ok(manager) = app.chat_manager.try_lock() {
        all_toasts.extend(manager.toasts.clone());
    }

    // Sort toasts by creation time
    all_toasts.sort_by_key(|t| t.created_at);

    egui::Area::new(egui::Id::new("toasts"))
        .fixed_pos(egui::pos2(
            ctx.screen_rect().width() - 320.0,
            60.0 + crate::gui::styling::SPACING_LARGE,
        ))
        .show(ctx, |ui| {
            ui.set_max_width(300.0);

            for toast in &all_toasts {
                let elapsed = toast.created_at.elapsed().as_secs_f32();
                let duration = toast.duration.as_secs_f32();
                let progress = elapsed / duration;

                if progress < 1.0 {
                    let alpha = toast_alpha(progress);
                    let (icon, base_color) = toast_style(toast.level);

                    let frame_fill = egui::Color32::from_rgba_unmultiplied(
                        base_color.r(),
                        base_color.g(),
                        base_color.b(),
                        25,
                    );
                    let text_color = ui.style().visuals.text_color();
                    let final_text_color = egui::Color32::from_rgba_unmultiplied(
                        text_color.r(),
                        text_color.g(),
                        text_color.b(),
                        alpha,
                    );
                    let icon_color = egui::Color32::from_rgba_unmultiplied(
                        base_color.r(),
                        base_color.g(),
                        base_color.b(),
                        alpha,
                    );
                    let stroke_color = icon_color;

                    let toast_frame = egui::Frame {
                        inner_margin: egui::Margin::symmetric(
                            crate::gui::styling::SPACING_LARGE,
                            crate::gui::styling::SPACING_MEDIUM,
                        ),
                        outer_margin: egui::Margin::same(0.0),
                        rounding: egui::Rounding::same(crate::gui::styling::RADIUS_DEFAULT),
                        shadow: egui::epaint::Shadow {
                            blur: 8.0,
                            spread: 2.0,
                            color: egui::Color32::from_black_alpha(alpha / 5),
                            ..Default::default()
                        },
                        fill: frame_fill,
                        stroke: egui::Stroke::new(1.0_f32, stroke_color),
                    };

                    toast_frame.show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(icon).color(icon_color).size(16.0));
                            ui.add_space(5.0);
                            ui.label(egui::RichText::new(&toast.message).color(final_text_color));
                        });
                    });

                    ui.add_space(8.0);
                }
            }
        });

    // Cleanup expired app-level toasts
    let now = std::time::Instant::now();
    app.toasts
        .retain(|toast| now.duration_since(toast.created_at) < toast.duration);
}

fn toast_style(level: crate::types::ToastLevel) -> (&'static str, egui::Color32) {
    match level {
        crate::types::ToastLevel::Info => ("ℹ", crate::gui::styling::ACCENT_PRIMARY),
        crate::types::ToastLevel::Success => ("✔", crate::gui::styling::SUCCESS),
        crate::types::ToastLevel::Warning => ("⚠", crate::gui::styling::WARNING),
        crate::types::ToastLevel::Error => ("❌", crate::gui::styling::ERROR),
    }
}

fn toast_alpha(progress: f32) -> u8 {
    let alpha_f32 = if progress < 0.15 {
        progress / 0.15
    } else if progress > 0.75 {
        1.0 - ((progress - 0.75) / 0.25)
    } else {
        1.0
    };

    (alpha_f32 * 255.0).min(255.0) as u8
}

fn render_delete_confirmation(app: &mut App, ctx: &egui::Context, chat_id: uuid::Uuid) {
    egui::Window::new("⚠️ Delete Chat")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label("Are you sure you want to delete this chat?");
            ui.label("This action cannot be undone.");
            ui.add_space(10.0);

            ui.horizontal(|ui| {
                if crate::gui::widgets::primary_button(ui, "❌ Delete").clicked() {
                    tracing::info!("Delete button clicked for chat_id: {}", chat_id);
                    if let Ok(mut manager) = app.chat_manager.try_lock() {
                        manager.delete_chat(chat_id);
                        if app.selected_chat == Some(chat_id) {
                            app.selected_chat = None;
                        }
                        queue_history_save(app.history_path.clone(), &mut manager);
                    }
                    app.chat_to_delete = None;
                }
                if crate::gui::widgets::secondary_button(ui, "Cancel").clicked() {
                    app.chat_to_delete = None;
                }
            });
        });
}

/// Apply the password typed in the Host/Connect dialog to the chat manager so the
/// subsequent host/connect uses it (host requires it; client supplies it).
fn apply_connection_password(app: &mut App) {
    let pw = std::mem::take(&mut app.connection_password_input);
    if let Ok(mut m) = app.chat_manager.try_lock() {
        m.set_connection_password(if pw.is_empty() { None } else { Some(pw) });
    }
}

fn render_host_dialog(app: &mut App, ctx: &egui::Context) {
    egui::Window::new("Start Host")
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label("Port:");
            ui.text_edit_singleline(&mut app.host_port);

            ui.add_space(6.0);
            ui.label("Connection password (optional):");
            ui.add(
                egui::TextEdit::singleline(&mut app.connection_password_input)
                    .password(true)
                    .hint_text("leave blank for no password"),
            );
            ui.small("Peers must enter this exact password to connect.");

            ui.horizontal(|ui| {
                if crate::gui::widgets::primary_button(ui, "Start").clicked() {
                    tracing::info!("Start host button clicked");
                    apply_connection_password(app);
                    app.start_host_clicked();
                    app.active_dialog = ActiveDialog::None;
                }

                if crate::gui::widgets::secondary_button(ui, "Cancel").clicked() {
                    app.active_dialog = ActiveDialog::None;
                }
            });
        });
}

fn render_connect_dialog(app: &mut App, ctx: &egui::Context) {
    // Poll for discovered peers
    if let Some(ref discovery) = app.discovery {
        discovery.poll(&app.discovered_peers);
    }

    egui::Window::new("Connect to Host")
        .collapsible(false)
        .resizable(false)
        .min_width(350.0)
        .show(ctx, |ui| {
            // Show discovered peers
            let peers = app
                .discovered_peers
                .lock()
                .ok()
                .map(|p| p.clone())
                .unwrap_or_default();
            if !peers.is_empty() {
                ui.heading("📡 Nearby Users");
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .max_height(150.0)
                    .show(ui, |ui| {
                        for peer in &peers {
                            ui.horizontal(|ui| {
                                let label =
                                    format!("{} ({}:{})", peer.name, peer.address, peer.port);
                                if ui.button(&label).clicked() {
                                    app.connect_host = peer.address.clone();
                                    app.connect_port = peer.port.to_string();
                                }
                            });
                        }
                    });
                ui.separator();
            } else {
                ui.label(
                    egui::RichText::new("🔍 Searching for nearby users...")
                        .italics()
                        .color(crate::gui::styling::SUBTLE_TEXT_COLOR),
                );
                ui.add_space(8.0);
            }

            ui.label("Host:");
            ui.text_edit_singleline(&mut app.connect_host);

            ui.label("Port:");
            ui.text_edit_singleline(&mut app.connect_port);

            ui.add_space(6.0);
            ui.label("Connection password (if required):");
            ui.add(
                egui::TextEdit::singleline(&mut app.connection_password_input)
                    .password(true)
                    .hint_text("leave blank if the host has none"),
            );

            ui.horizontal(|ui| {
                if crate::gui::widgets::primary_button(ui, "Connect").clicked() {
                    tracing::info!("Connect to host button clicked");
                    apply_connection_password(app);
                    app.connect_clicked();
                    app.active_dialog = ActiveDialog::None;
                }

                if crate::gui::widgets::secondary_button(ui, "Cancel").clicked() {
                    app.active_dialog = ActiveDialog::None;
                }
            });
        });
}

fn render_contacts_window(app: &mut App, ctx: &egui::Context) {
    egui::Window::new("👥 Contacts")
        .collapsible(false)
        .resizable(true)
        .default_width(400.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("➕ Add Contact").clicked() {
                    // Reset fields before showing the dialog to ensure a clean state
                    app.new_contact_name.clear();
                    app.new_contact_address.clear();
                    app.new_contact_fingerprint.clear();
                    app.new_contact_pubkey.clear();
                    app.invite_link_input.clear();
                    app.my_invite_link = None;
                    app.my_invite_link_addr = None;
                    app.qr_code_texture = None;
                    app.active_dialog = ActiveDialog::AddContact;
                }

                if ui.button("🧩 Create Group").clicked() {
                    app.active_dialog = ActiveDialog::CreateGroup;
                    app.group_selected.clear();
                }
            });

            ui.separator();

            let contact_snapshots: Vec<(crate::types::Contact, Option<uuid::Uuid>)> =
                if let Ok(manager) = app.chat_manager.try_lock() {
                    manager
                        .contacts
                        .values()
                        .map(|contact| {
                            let chat_id = manager.contact_to_chat.get(&contact.id).copied();
                            (contact.clone(), chat_id)
                        })
                        .collect()
                } else {
                    Vec::new()
                };

            egui::ScrollArea::vertical().show(ui, |ui| {
                for (contact, existing_chat_id) in contact_snapshots {
                    ui.horizontal(|ui| {
                        let mut is_selected = app.group_selected.contains(&contact.id);
                        if ui.checkbox(&mut is_selected, "").changed() {
                            if is_selected {
                                if !app.group_selected.contains(&contact.id) {
                                    app.group_selected.push(contact.id);
                                }
                            } else {
                                app.group_selected.retain(|id| id != &contact.id);
                            }
                        }

                        ui.label(&contact.name);
                        if let Some(fp) = &contact.fingerprint {
                            ui.monospace(crate::util::format_fingerprint_short(fp));
                        }

                        if ui.small_button("🔗").on_hover_text("Open chat").clicked() {
                            if let Some(chat_id) = existing_chat_id {
                                // If there's a mapped chat, select it.
                                app.selected_chat = Some(chat_id);
                                app.active_dialog = ActiveDialog::None;
                            } else {
                                // Otherwise, create a new chat entry locally first for responsiveness.
                                let chat_id = uuid::Uuid::new_v4();
                                app.selected_chat = Some(chat_id);

                                // If contact has no address, prompt user to open Connect dialog and bind to this chat
                                let should_prompt_connect = contact
                                    .address
                                    .as_ref()
                                    .map(|s| s.trim().is_empty())
                                    .unwrap_or(true);
                                if should_prompt_connect {
                                    // Pre-open connect dialog; the connect action will now bind to selected_chat
                                    app.connect_host.clear();
                                    app.connect_port = crate::PORT_DEFAULT.to_string();
                                    app.active_dialog = ActiveDialog::Connect;
                                }

                                // Clone the necessary data before spawning the task
                                let manager_clone = app.chat_manager.clone();
                                let contact_clone = contact.clone();
                                let history_path = app.history_path.clone();
                                let privkey = app.identity.private_key().ok();
                                if privkey.is_none() && contact_clone.address.is_some() {
                                    app.add_toast(
                                        crate::types::ToastLevel::Error,
                                        "Cannot connect: identity key unavailable".to_string(),
                                    );
                                }

                                // Spawn a task to do the real work: create chat in manager and connect.
                                tokio::spawn(async move {
                                    let mut mgr = manager_clone.lock().await;
                                    // 1. Create the chat object and add it to the manager
                                    let chat = crate::types::Chat {
                                        id: chat_id,
                                        title: contact_clone.name.clone(),
                                        kind: crate::types::ChatKind::Dm,
                                        transport: crate::types::Transport::Direct,
                                        peer_fingerprint: contact_clone.fingerprint.clone(),
                                        participants: vec![contact_clone.id],
                                        messages: Vec::new(),
                                        created_at: chrono::Utc::now(),
                                        peer_typing: false,
                                        typing_since: None,
                                        send_seq: 0,
                                        recv_seq: 0,
                                        is_host_placeholder: false,
                                    };
                                    mgr.chats.insert(chat_id, chat);
                                    mgr.associate_contact_with_chat(contact_clone.id, chat_id);

                                    // 2. Save history
                                    if let Err(e) = mgr.save_history(&history_path) {
                                        tracing::error!(
                                            "Failed to save history after creating chat: {}",
                                            e
                                        );
                                    }

                                    // 3. Asynchronously connect — for any contact we
                                    // can route (a direct address OR a relay
                                    // server+token). ChatManager::connect_to_contact
                                    // is the single source of truth and picks the
                                    // transport from the contact's data, so a
                                    // relay-only contact reaches the relay path
                                    // instead of opening a dead direct chat.
                                    let can_connect = contact_clone.address.is_some()
                                        || (contact_clone.relay_server.is_some()
                                            && contact_clone.relay_token.is_some());
                                    if can_connect {
                                        if let Some(ref pk) = privkey {
                                            if let Err(e) = mgr
                                                .connect_to_contact(contact_clone.id, Some(chat_id), pk)
                                                .await
                                            {
                                                mgr.add_toast(
                                                    crate::types::ToastLevel::Error,
                                                    format!(
                                                        "Failed to connect to {}: {}",
                                                        contact_clone.name, e
                                                    ),
                                                );
                                            }
                                        }
                                    } else {
                                        // Inform the user a connection is needed via Connect dialog
                                        mgr.add_toast(
                                            crate::types::ToastLevel::Info,
                                            format!(
                                                "No address or relay for {}. Open 'Connect to Host' to connect this chat.",
                                                contact_clone.name
                                            ),
                                        );
                                    }
                                });
                                app.active_dialog = ActiveDialog::None; // Close dialog after action
                            }
                        }

                        if ui
                            .small_button("🗑")
                            .on_hover_text("Delete contact")
                            .clicked()
                        {
                            let manager = app.chat_manager.clone();
                            let contact_id = contact.id;
                            let history_path = app.history_path.clone();
                            tokio::spawn(async move {
                                let mut mgr = manager.lock().await;
                                mgr.remove_contact(contact_id);
                                let _ = mgr.save_history(&history_path);
                            });
                        }
                    });
                    ui.separator();
                }
            });

            ui.horizontal(|ui| {
                if ui.button("Close").clicked() {
                    app.active_dialog = ActiveDialog::None;
                }
            });
        });
}

fn render_add_contact_dialog(app: &mut App, ctx: &egui::Context) {
    egui::Window::new("➕ Add Contact")
        .collapsible(false)
        .resizable(false)
        .min_width(500.0)
        .show(ctx, |ui| {
            // Tabs - use simple buttons instead of selectable_label to avoid checkboxes
            ui.horizontal(|ui| {
                if ui
                    .button(
                        egui::RichText::new("🔗 Invite Link").color(if app.contact_tab == 1 {
                            crate::gui::styling::ACCENT_PRIMARY
                        } else {
                            crate::gui::styling::SUBTLE_TEXT_COLOR
                        }),
                    )
                    .clicked()
                {
                    app.contact_tab = 1;
                }
                if ui
                    .button(
                        egui::RichText::new("📝 Manual").color(if app.contact_tab == 0 {
                            crate::gui::styling::ACCENT_PRIMARY
                        } else {
                            crate::gui::styling::SUBTLE_TEXT_COLOR
                        }),
                    )
                    .clicked()
                {
                    app.contact_tab = 0;
                }
                if ui
                    .button(egui::RichText::new("📤 Share My Link").color(
                        if app.contact_tab == 2 {
                            crate::gui::styling::ACCENT_PRIMARY
                        } else {
                            crate::gui::styling::SUBTLE_TEXT_COLOR
                        },
                    ))
                    .clicked()
                {
                    app.contact_tab = 2;
                }
            });

            ui.separator();
            ui.add_space(10.0);

            match app.contact_tab {
                // Manual tab (existing functionality)
                0 => {
                    ui.label("⚠️ Note: Manual entry requires exact fingerprint and public key");
                    ui.add_space(10.0);

                    ui.label("Name:");
                    ui.text_edit_singleline(&mut app.new_contact_name);

                    ui.label("Address (IP:Port - optional):");
                    ui.text_edit_singleline(&mut app.new_contact_address);

                    ui.label("Fingerprint (64 hex chars - optional):");
                    ui.text_edit_singleline(&mut app.new_contact_fingerprint);

                    ui.label("Public key PEM (optional):");
                    ui.text_edit_multiline(&mut app.new_contact_pubkey);

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if crate::gui::widgets::primary_button(ui, "➕ Add Contact").clicked() {
                            let name = app.new_contact_name.trim().to_string();
                            let address = if app.new_contact_address.trim().is_empty() {
                                None
                            } else {
                                Some(app.new_contact_address.trim().to_string())
                            };
                            let fp = if app.new_contact_fingerprint.trim().is_empty() {
                                None
                            } else {
                                Some(app.new_contact_fingerprint.trim().to_string())
                            };
                            let pk = if app.new_contact_pubkey.trim().is_empty() {
                                None
                            } else {
                                Some(app.new_contact_pubkey.trim().to_string())
                            };

                            let mut validation_error = None;
                            if let Some(ref addr) = address {
                                // Basic validation: check if it has a colon
                                if !addr.contains(':') {
                                    validation_error = Some("Address must be in host:port format");
                                }
                            }

                            if let Some(err) = validation_error {
                                app.toasts.push(crate::types::Toast {
                                    id: uuid::Uuid::new_v4(),
                                    level: crate::types::ToastLevel::Error,
                                    message: err.to_string(),
                                    created_at: std::time::Instant::now(),
                                    duration: std::time::Duration::from_secs(3),
                                });
                            } else if !name.is_empty() {
                                tracing::info!("Adding contact manually: {}", name);
                                let manager = app.chat_manager.clone();
                                let history_path = app.history_path.clone();
                                tokio::spawn(async move {
                                    let mut mgr = manager.lock().await;
                                    mgr.add_contact(name, address, fp, pk);
                                    let _ = mgr.save_history(&history_path);
                                    mgr.add_toast(
                                        crate::types::ToastLevel::Success,
                                        "Contact added!".to_string(),
                                    );
                                });

                                app.new_contact_name.clear();
                                app.new_contact_address.clear();
                                app.new_contact_fingerprint.clear();
                                app.new_contact_pubkey.clear();
                                app.active_dialog = ActiveDialog::None;
                            }
                        }

                        if crate::gui::widgets::secondary_button(ui, "Cancel").clicked() {
                            app.active_dialog = ActiveDialog::None;
                        }
                    });
                }
                // Invite Link tab (NEW!)
                1 => {
                    ui.label("✨ Easy way: Just paste an invite link from your friend!");
                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        ui.label("Paste invite link (chat-p2p://invite/...");
                        if ui.button("📋 Paste").clicked() {
                            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                if let Ok(text) = clipboard.get_text() {
                                    app.invite_link_input = text;
                                }
                            }
                        }
                    });
                    ui.text_edit_singleline(&mut app.invite_link_input);

                    let mut parsed_contact = None;
                    if !app.invite_link_input.is_empty() {
                        ui.label(
                            egui::RichText::new("✅ Link detected")
                                .color(crate::gui::styling::SUCCESS),
                        );
                        // Attempt to parse the link and pre-fill fields
                        if let Ok(manager) = app.chat_manager.try_lock() {
                            match manager.parse_invite_link(&app.invite_link_input) {
                                Ok(contact) => {
                                    parsed_contact = Some(contact.clone());
                                    let had_address = contact.address.is_some();
                                    app.new_contact_name = contact.name;
                                    app.new_contact_address = contact.address.unwrap_or_default();
                                    app.new_contact_fingerprint =
                                        contact.fingerprint.unwrap_or_default();
                                    app.new_contact_pubkey = contact.public_key.unwrap_or_default();
                                    if had_address {
                                        ui.label(
                                            egui::RichText::new(
                                                "IP and port auto-filled from the link.",
                                            )
                                            .color(crate::gui::styling::SUCCESS),
                                        );
                                    }
                                }
                                Err(e) => {
                                    ui.label(
                                        egui::RichText::new(format!("❌ Invalid link: {}", e))
                                            .color(crate::gui::styling::ERROR),
                                    );
                                }
                            }
                        }
                    }

                    ui.add_space(10.0);

                    ui.label("Name:");
                    ui.text_edit_singleline(&mut app.new_contact_name);

                    ui.label("Address (IP:Port - optional):");
                    ui.text_edit_singleline(&mut app.new_contact_address);

                    ui.label("Fingerprint (64 hex chars - optional):");
                    ui.text_edit_singleline(&mut app.new_contact_fingerprint);

                    ui.label("Public key PEM (optional):");
                    ui.text_edit_multiline(&mut app.new_contact_pubkey);

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if crate::gui::widgets::primary_button(ui, "➕ Add from Link").clicked() {
                            if let Some(contact) = parsed_contact.clone() {
                                tracing::info!("Adding contact from link: {}", contact.name);
                                let manager = app.chat_manager.clone();
                                let history_path = app.history_path.clone();

                                tokio::spawn(async move {
                                    let mut mgr = manager.lock().await;
                                    mgr.import_contact(contact);
                                    let _ = mgr.save_history(&history_path);
                                    mgr.add_toast(
                                        crate::types::ToastLevel::Success,
                                        "Contact added!".to_string(),
                                    );
                                });

                                app.invite_link_input.clear();
                                app.new_contact_name.clear();
                                app.new_contact_address.clear();
                                app.new_contact_fingerprint.clear();
                                app.new_contact_pubkey.clear();
                                app.active_dialog = ActiveDialog::None;
                            }
                        }

                        if crate::gui::widgets::secondary_button(ui, "Cancel").clicked() {
                            app.active_dialog = ActiveDialog::None;
                        }
                    });
                }
                // Share My Link tab (NEW!)
                2 => {
                    ui.label("📤 Share this link with your friends so they can add you:");
                    ui.add_space(10.0);

                    // Generate link using actual identity; prefer the UPnP
                    // external address (reachable from outside the LAN) over
                    // the best-effort local one. The external address resolves
                    // asynchronously (up to 15s after hosting starts), so a link
                    // first built from the LAN address is regenerated once the
                    // external address appears or changes.
                    let invite_addr = {
                        let port = app
                            .chat_manager
                            .try_lock()
                            .map(|m| m.config.listen_port)
                            .ok();
                        let external = app
                            .chat_manager
                            .try_lock()
                            .ok()
                            .and_then(|m| m.external_address.clone());
                        external.or_else(|| {
                            port.and_then(|port| {
                                primary_local_ipv4().map(|ip| format!("{}:{}", ip, port))
                            })
                        })
                    };
                    if app.my_invite_link.is_none() || app.my_invite_link_addr != invite_addr {
                        match app
                            .identity
                            .generate_signed_invite_link(invite_addr.clone())
                        {
                            Ok(link) => {
                                app.my_invite_link = Some(link);
                                app.my_invite_link_addr = invite_addr;
                                // Force the QR to re-render for the new link.
                                app.qr_code_texture = None;
                            }
                            Err(e) => {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "❌ Failed to generate signed link: {}",
                                        e
                                    ))
                                    .color(crate::gui::styling::ERROR),
                                );
                            }
                        }
                    }

                    if let Some(link) = &app.my_invite_link {
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(link).monospace());
                                if crate::gui::widgets::secondary_button(ui, "📋 Copy").clicked()
                                {
                                    ui.output_mut(|o| o.copied_text = link.clone());
                                }
                            });
                        });
                        ui.add_space(10.0);

                        // Generate and display QR code
                        if app.qr_code_texture.is_none() {
                            if let Ok(manager) = app.chat_manager.try_lock() {
                                if let Ok(qr_bytes) = manager.generate_invite_qr(link) {
                                    if let Ok(image) = image::load_from_memory_with_format(
                                        &qr_bytes,
                                        image::ImageFormat::Png,
                                    ) {
                                        let size = [image.width() as _, image.height() as _];
                                        let image_buffer = image.to_rgba8();
                                        let pixels = image_buffer.as_flat_samples();
                                        let texture = ctx.load_texture(
                                            "qr_code",
                                            egui::ImageData::Color(std::sync::Arc::new(
                                                egui::ColorImage {
                                                    size,
                                                    pixels: pixels
                                                        .as_slice()
                                                        .to_vec()
                                                        .chunks_exact(4)
                                                        .map(|p| {
                                                            egui::Color32::from_rgba_unmultiplied(
                                                                p[0], p[1], p[2], p[3],
                                                            )
                                                        })
                                                        .collect(),
                                                },
                                            )),
                                            egui::TextureOptions::LINEAR,
                                        );
                                        app.qr_code_texture = Some(texture);
                                    } else {
                                        tracing::warn!("Failed to decode QR image from bytes");
                                    }
                                }
                            }
                        }
                        if let Some(texture) = &app.qr_code_texture {
                            ui.image(texture);
                        }
                    }

                    ui.add_space(10.0);

                    let grid = generate_color_grid(&app.identity.fingerprint);
                    ui.add(ColorGrid::new(grid));

                    ui.add_space(10.0);
                    ui.label("💡 Tip: You can share this via:");
                    ui.label("  • Email, WhatsApp, SMS");

                    ui.add_space(10.0);
                    if crate::gui::widgets::secondary_button(ui, "Close").clicked() {
                        app.active_dialog = ActiveDialog::None;
                    }
                }
                _ => {} // Should not happen
            }
        });
}

fn render_create_group_wizard(app: &mut App, ctx: &egui::Context) {
    let step_titles = [
        "Step 1: Name Your Group",
        "Step 2: Select Members",
        "Step 3: Review & Create",
    ];
    let title = format!(
        "🧩 Create Group - {}",
        step_titles[app.group_wizard_step.min(2)]
    );

    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .default_width(450.0)
        .show(ctx, |ui| {
            // Progress indicator
            ui.horizontal(|ui| {
                for i in 0..3 {
                    if i == app.group_wizard_step {
                        ui.label(egui::RichText::new(format!("● {}", i + 1)).strong().color(crate::gui::styling::ACCENT_PRIMARY));
                    } else if i < app.group_wizard_step {
                        ui.label(egui::RichText::new(format!("✓ {}", i + 1)).color(crate::gui::styling::SUCCESS));
                    } else {
                        ui.label(egui::RichText::new(format!("○ {}", i + 1)).weak());
                    }
                    if i < 2 {
                        ui.label("─");
                    }
                }
            });
            ui.separator();
            ui.add_space(5.0);

            match app.group_wizard_step {
                // Step 0: Group Name & Description
                0 => {
                    ui.label(egui::RichText::new("Give your group a name").heading());
                    ui.add_space(10.0);

                    ui.label("Group name:");
                    let name_response = ui.text_edit_singleline(&mut app.group_title);

                    let name_valid = !app.group_title.trim().is_empty();
                    if !name_valid && name_response.lost_focus() {
                        ui.label(egui::RichText::new("⚠ Group name is required").color(crate::gui::styling::ERROR));
                    }

                    ui.add_space(5.0);
                    ui.label(egui::RichText::new("💡 Tip: Choose a descriptive name like \"Project Team\" or \"Family Chat\"").weak().italics());
                    ui.add_space(15.0);

                    ui.horizontal(|ui| {
                        if crate::gui::widgets::secondary_button(ui, "Cancel").clicked() {
                            app.active_dialog = ActiveDialog::None;
                            app.group_wizard_step = 0;
                            app.group_title.clear();
                            app.group_selected.clear();
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add_enabled(name_valid, egui::Button::new("Next ▶")).clicked() {
                                app.group_wizard_step = 1;
                            }
                        });
                    });
                }

                // Step 1: Select Members
                1 => {
                    ui.label(egui::RichText::new("Add members to your group").heading());
                    ui.add_space(10.0);

                    // Search bar
                    ui.horizontal(|ui| {
                        ui.label("🔍 Search:");
                        ui.text_edit_singleline(&mut app.group_search);
                        if ui.small_button("✖").on_hover_text("Clear search").clicked() {
                            app.group_search.clear();
                        }
                    });
                    ui.add_space(5.0);

                    // Member selection list
                    egui::Frame::group(ui.style())
                        .inner_margin(egui::Margin::same(8.0))
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .max_height(200.0)
                                .show(ui, |ui| {
                                    if let Ok(manager) = app.chat_manager.try_lock() {
                                        let search_lower = app.group_search.to_lowercase();
                                        let mut contacts: Vec<_> = manager.contacts.values().collect();
                                        contacts.sort_by(|a, b| a.name.cmp(&b.name));

                                        let mut found_any = false;
                                        for contact in contacts {
                                            // Filter by search
                                            if !search_lower.is_empty() && !contact.name.to_lowercase().contains(&search_lower) {
                                                continue;
                                            }
                                            found_any = true;

                                            ui.horizontal(|ui| {
                                                let mut checked = app.group_selected.contains(&contact.id);
                                                if ui.checkbox(&mut checked, "").changed() {
                                                    if checked {
                                                        if !app.group_selected.contains(&contact.id) {
                                                            app.group_selected.push(contact.id);
                                                        }
                                                    } else {
                                                        app.group_selected.retain(|id| id != &contact.id);
                                                    }
                                                }

                                                ui.label(&contact.name);
                                                if let Some(fp) = &contact.fingerprint {
                                                    ui.monospace(crate::util::format_fingerprint_short(fp));
                                                }
                                            });
                                        }

                                        if !found_any {
                                            ui.label(egui::RichText::new("No contacts found").weak().italics());
                                        }
                                    } else {
                                        ui.label(egui::RichText::new("Loading contacts...").weak().italics());
                                    }
                                });
                        });

                    ui.add_space(5.0);
                    ui.label(format!("✅ {} member(s) selected", app.group_selected.len()));

                    if app.group_selected.is_empty() {
                        ui.label(egui::RichText::new("⚠ At least one member is required").color(crate::gui::styling::WARNING).italics());
                    }

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if crate::gui::widgets::secondary_button(ui, "◀ Back").clicked() {
                            app.group_wizard_step = 0;
                        }

                        if crate::gui::widgets::secondary_button(ui, "Cancel").clicked() {
                            app.active_dialog = ActiveDialog::None;
                            app.group_wizard_step = 0;
                            app.group_title.clear();
                            app.group_selected.clear();
                            app.group_search.clear();
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let can_proceed = !app.group_selected.is_empty();
                            if ui.add_enabled(can_proceed, egui::Button::new("Next ▶")).clicked() {
                                app.group_wizard_step = 2;
                            }
                        });
                    });
                }

                // Step 2: Review & Create
                2 => {
                    ui.label(egui::RichText::new("Review and create your group").heading());
                    ui.add_space(10.0);

                    // Summary
                    egui::Frame::group(ui.style())
                        .inner_margin(egui::Margin::same(10.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Group Name:").strong());
                                ui.label(&app.group_title);
                            });

                            ui.add_space(5.0);

                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Members:").strong());
                                ui.label(format!("{}", app.group_selected.len()));
                            });

                            ui.add_space(5.0);

                            // List member names
                            if let Ok(manager) = app.chat_manager.try_lock() {
                                ui.label(egui::RichText::new("Member list:").weak());
                                egui::ScrollArea::vertical()
                                    .max_height(120.0)
                                    .show(ui, |ui| {
                                        for contact_id in &app.group_selected {
                                            if let Some(contact) = manager.contacts.get(contact_id) {
                                                ui.label(format!("  • {}", contact.name));
                                            }
                                        }
                                    });
                            }
                        });

                    ui.add_space(10.0);
                    ui.label(egui::RichText::new("🎉 Everything looks good? Click Create to start your group!").weak().italics());

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if crate::gui::widgets::secondary_button(ui, "◀ Back").clicked() {
                            app.group_wizard_step = 1;
                        }

                        if crate::gui::widgets::secondary_button(ui, "Cancel").clicked() {
                            app.active_dialog = ActiveDialog::None;
                            app.group_wizard_step = 0;
                            app.group_title.clear();
                            app.group_selected.clear();
                            app.group_search.clear();
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if crate::gui::widgets::primary_button(ui, "✓ Create Group").clicked() {
                                let participants = app.group_selected.clone();
                                let title = Some(app.group_title.trim().to_string());
                                let manager = app.chat_manager.clone();
                                let history_path = app.history_path.clone();

                                tokio::spawn(async move {
                                    let mut mgr = manager.lock().await;
                                    let _chat_id = mgr.create_group_chat(participants, title);
                                    let _ = mgr.save_history(&history_path);
                                    mgr.add_toast(crate::types::ToastLevel::Success, "Group created!".to_string());
                                });

                                // Close wizard and reset
                                app.active_dialog = ActiveDialog::None;
                                app.group_wizard_step = 0;
                                app.group_selected.clear();
                                app.group_title.clear();
                                app.group_search.clear();
                            }
                        });
                    });
                }

                _ => {
                    // Fallback - should never happen
                    ui.label("Invalid wizard step");
                    if crate::gui::widgets::secondary_button(ui, "Reset").clicked() {
                        app.group_wizard_step = 0;
                    }
                }
            }
        });
}

fn render_rename_dialog(app: &mut App, ctx: &egui::Context) {
    if let Some(chat_id) = app.rename_chat_id {
        tracing::info!("Rendering rename dialog for chat_id: {}", chat_id);
        let mut show_lock_error = false;

        egui::Window::new("Rename Conversation")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("New title:");
                let response = ui.text_edit_singleline(&mut app.rename_input);
                ui.add_space(10.0);

                let save_button = crate::gui::widgets::primary_button(ui, "✅ Save");
                let enter_pressed =
                    response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

                if save_button.clicked() || enter_pressed {
                    tracing::info!(
                        "Save action triggered for rename dialog, chat_id: {}",
                        chat_id
                    );
                    if let Ok(mut manager) = app.chat_manager.try_lock() {
                        if let Err(e) = manager.rename_chat(chat_id, app.rename_input.clone()) {
                            manager.add_toast(
                                crate::types::ToastLevel::Error,
                                format!("Failed to rename: {}", e),
                            );
                        } else {
                            manager.add_toast(
                                crate::types::ToastLevel::Success,
                                "Chat renamed successfully!".to_string(),
                            );
                            queue_history_save(app.history_path.clone(), &mut manager);
                            ctx.request_repaint();
                            app.active_dialog = ActiveDialog::None;
                        }
                    } else {
                        show_lock_error = true;
                    }
                }

                if crate::gui::widgets::secondary_button(ui, "❌ Cancel").clicked() {
                    app.active_dialog = ActiveDialog::None;
                }
            });

        if show_lock_error {
            app.add_toast(
                crate::types::ToastLevel::Error,
                "Could not lock chat manager. Please try again.".to_string(),
            );
        }
    }
}

fn render_settings_dialog(app: &mut App, ctx: &egui::Context) {
    egui::Window::new("⚙️ Settings")
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.heading("Application Settings");
            ui.separator();

            if let Ok(mut manager) = app.chat_manager.try_lock() {
                // Auto-host on startup
                ui.horizontal(|ui| {
                    let mut auto_host = manager.config.auto_host_on_startup;
                    if ui.checkbox(&mut auto_host, "Auto-host (listen) on startup").changed() {
                        manager.config.auto_host_on_startup = auto_host;
                        queue_history_save(app.history_path.clone(), &mut manager);
                        // If enabled, start hosting immediately using current listen_port
                        if auto_host {
                            match app.identity.private_key() {
                                Ok(privkey) => {
                                    let port = manager.config.listen_port;
                                    let mgr_arc = app.chat_manager.clone();
                                    tokio::spawn(async move {
                                        let mut mgr = mgr_arc.lock().await;
                                        if let Err(e) = mgr.start_host(port, privkey).await {
                                            mgr.add_toast(
                                                crate::types::ToastLevel::Error,
                                                format!("Failed to start host: {}", e),
                                            );
                                        }
                                    });
                                }
                                Err(e) => {
                                    manager.add_toast(
                                        crate::types::ToastLevel::Error,
                                        format!("Cannot start host: {}", e),
                                    );
                                }
                            }
                        } else {
                            // No stop_host yet; inform user it will stop on next launch
                            manager.add_toast(
                                crate::types::ToastLevel::Info,
                                "Auto-host disabled. Existing listeners (if any) will stop on next app restart.".to_string(),
                            );
                        }
                    }
                });

                ui.horizontal(|ui| {
                    ui.label("Listen port:");
                    let mut port_str = manager.config.listen_port.to_string();
                    if ui.text_edit_singleline(&mut port_str).changed() {
                        if let Ok(p) = port_str.parse::<u16>() {
                            manager.config.listen_port = p;
                            app.host_port = p.to_string(); // keep Host dialog in sync
                            queue_history_save(app.history_path.clone(), &mut manager);
                        }
                    }
                });

                if ui
                    .checkbox(
                        &mut manager.config.enable_upnp,
                        "UPnP port mapping (ask the router to make the host reachable from the internet)",
                    )
                    .changed()
                {
                    queue_history_save(app.history_path.clone(), &mut manager);
                }

                // Show my IP address (best-effort primary local IPv4)
                ui.add_space(8.0);
                ui.label("My IP address (primary, best-effort):");
                let my_ip = primary_local_ipv4().unwrap_or_else(|| "Unavailable".to_string());
                ui.monospace(my_ip);
                if let Some(ext) = manager.external_address.clone() {
                    ui.label("External address (UPnP):");
                    ui.monospace(ext);
                }

                ui.add_space(10.0);

                ui.label("Download Directory:");
                ui.horizontal(|ui| {
                    ui.label(manager.config.download_dir.display().to_string());
                    if ui.button("📁 Browse").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            manager.config.download_dir = path;
                            queue_history_save(app.history_path.clone(), &mut manager);
                        }
                    }
                });

                ui.add_space(10.0);

                if ui.checkbox(
                    &mut manager.config.auto_accept_files,
                    "Auto-accept file transfers",
                ).changed() {
                    queue_history_save(app.history_path.clone(), &mut manager);
                }

                ui.add_space(10.0);



                ui.add_space(10.0);

                if ui.checkbox(
                    &mut manager.config.enable_notifications,
                    "Enable desktop notifications",
                ).changed() {
                    queue_history_save(app.history_path.clone(), &mut manager);
                }

                ui.add_space(10.0);

                if ui.checkbox(
                    &mut manager.config.enable_typing_indicators,
                    "Enable typing indicators",
                ).changed() {
                    queue_history_save(app.history_path.clone(), &mut manager);
                }

                ui.add_space(10.0);

                // Theme selection
                ui.horizontal(|ui| {
                    ui.label("Theme:");
                    if egui::ComboBox::from_id_salt("theme_selector")
                        .selected_text(format!("{:?}", manager.config.theme))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut manager.config.theme, crate::types::Theme::Light, "Light").changed() ||
                            ui.selectable_value(&mut manager.config.theme, crate::types::Theme::Dark, "Dark").changed() ||
                            ui.selectable_value(&mut manager.config.theme, crate::types::Theme::Midnight, "Midnight").changed() ||
                            ui.selectable_value(&mut manager.config.theme, crate::types::Theme::Forest, "Forest").changed() ||
                            ui.selectable_value(&mut manager.config.theme, crate::types::Theme::Rose, "Rose").changed()
                        }).inner.unwrap_or(false) {
                            queue_history_save(app.history_path.clone(), &mut manager);
                            // Apply theme immediately
                            ctx.set_visuals(crate::gui::styling::apply_custom_visuals(&manager.config.theme));
                        }
                });

                ui.add_space(10.0);

                // Font size slider
                ui.horizontal(|ui| {
                    ui.label("Font Size:");
                    if ui.add(egui::Slider::new(&mut manager.config.font_size, 10..=20).suffix("px")).changed() {
                        queue_history_save(app.history_path.clone(), &mut manager);
                        // Apply font size immediately
                        let mut style = (*ctx.style()).clone();
                        if let Some(s) = style.text_styles.get_mut(&egui::TextStyle::Body) {
                            s.size = manager.config.font_size as f32;
                        }
                        if let Some(s) = style.text_styles.get_mut(&egui::TextStyle::Button) {
                            s.size = manager.config.font_size as f32;
                        }
                        ctx.set_style(style);
                    }
                });

                ui.add_space(10.0);

                // Auto-connect checkbox
                if ui.checkbox(
                    &mut manager.config.auto_connect,
                    "Auto-connect to last known peer",
                ).changed() {
                    queue_history_save(app.history_path.clone(), &mut manager);
                }

                ui.add_space(10.0);

                // Notification sound selection
                ui.horizontal(|ui| {
                    ui.label("Notification Sound:");
                    if egui::ComboBox::from_id_salt("notification_sound_selector")
                        .selected_text(format!("{:?}", manager.config.notification_sound))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut manager.config.notification_sound, crate::types::NotificationSound::None, "None").changed() ||
                            ui.selectable_value(&mut manager.config.notification_sound, crate::types::NotificationSound::Default, "Default").changed()
                        }).inner.unwrap_or(false) {
                            queue_history_save(app.history_path.clone(), &mut manager);
                        }
                });

                ui.add_space(10.0);

                let mut show_log = app.show_log_terminal;
                if ui.checkbox(&mut show_log, "Show Log Terminal").changed() {
                    app.show_log_terminal = show_log;
                    manager.config.show_log_terminal = show_log;
                    queue_history_save(app.history_path.clone(), &mut manager);
                }
            }

            ui.add_space(20.0);
            ui.heading("Support");
            ui.separator();
            ui.add_space(10.0);

            ui.horizontal(|ui| {
                if crate::gui::widgets::primary_button(ui, "Export Diagnostics Bundle").clicked() {
                    app.export_diagnostics_bundle();
                }
                if crate::gui::widgets::secondary_button(ui, "Open Data Directory").clicked() {
                    app.open_data_directory();
                }
            });
            ui.label(
                egui::RichText::new(
                    "Diagnostics bundles include app state metadata and logs, but not private keys.",
                )
                .small()
                .weak(),
            );

            ui.add_space(20.0);
            ui.heading("Security");
            ui.separator();
            ui.add_space(10.0);

            ui.horizontal(|ui| {
                if crate::gui::widgets::primary_button(ui, "Set/Change Password").clicked() {
                    app.active_dialog = ActiveDialog::SetPassword;
                }
            });

            ui.label(
                egui::RichText::new(
                    "Removing password protection is disabled because identities are required to remain encrypted on disk.",
                )
                .small()
                .weak(),
            );


            ui.add_space(20.0);
            ui.heading("Danger Zone");
            ui.separator();
            ui.add_space(10.0);

            if crate::gui::widgets::primary_button(ui, "Clear Chat History").clicked() {
                app.active_dialog = ActiveDialog::ClearHistory;
            }

            ui.add_space(10.0);
            ui.separator();
            ui.horizontal(|ui| {
                if crate::gui::widgets::secondary_button(ui, "Close").clicked() {
                    app.active_dialog = ActiveDialog::None;
                }
            });
        });
}

fn render_password_dialog(app: &mut App, ctx: &egui::Context) {
    egui::Window::new("🔒 Unlock")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label("Enter your password to unlock your identity.");
            ui.add_space(10.0);
            render_unlock_form(app, ui);
        });
}

pub fn render_set_password_dialog(app: &mut App, ctx: &egui::Context) {
    egui::Window::new("🔑 Set Password")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            render_set_password_form(app, ui, true); // allow Cancel when opened from Settings
        });
}

fn render_clear_history_dialog(app: &mut App, ctx: &egui::Context) {
    egui::Window::new("⚠️ Clear All Data (Including Identity)")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(egui::RichText::new("WARNING: Complete Data Wipe").color(crate::gui::styling::ERROR));
            ui.label("This will delete:");
            ui.label("  • All chat messages and contacts");
            ui.label("  • Your cryptographic identity and keys");
            ui.label("  • Your password protection");
            ui.label(
                egui::RichText::new(
                    "You will need to create a NEW identity when you restart the app.\nThis action CANNOT be undone!",
                )
                .color(crate::gui::styling::ERROR),
            );
            ui.add_space(10.0);

            ui.horizontal(|ui| {
                if crate::gui::widgets::primary_button(ui, "❌ Delete Everything").clicked() {
                    let data_dir = app
                        .history_path
                        .parent()
                        .map(|dir| dir.to_path_buf())
                        .unwrap_or_default();
                    let identity_path = app.history_path.with_file_name("identity.json");
                    let wipe_result = if let Ok(mut manager) = app.chat_manager.try_lock() {
                        manager.delete_all_data(&data_dir, &app.history_path, &identity_path)
                    } else {
                        Err(anyhow!("Could not lock chat manager"))
                    };

                    match wipe_result {
                        Ok(()) => {
                            if let Ok(mut manager) = app.chat_manager.try_lock() {
                                manager.add_toast(
                                    crate::types::ToastLevel::Success,
                                    "All local data deleted. A new identity is required before you continue."
                                        .to_string(),
                                );
                            }
                            if let Ok(new_identity) =
                                crate::identity::Identity::new_with_plaintext("User".to_string())
                            {
                                app.identity = new_identity;
                                app.identity_locked = false;
                                app.is_new_identity = true;
                                app.force_password_setup = true;
                                app.selected_chat = None;
                                app.input_text.clear();
                                app.contact_tab = 0;
                            }
                        }
                        Err(e) => {
                            if let Ok(mut manager) = app.chat_manager.try_lock() {
                                manager.add_toast(
                                    crate::types::ToastLevel::Error,
                                    format!("Failed to delete all data: {}", e),
                                );
                            }
                        }
                    }
                    app.active_dialog = ActiveDialog::None;
                }
                if crate::gui::widgets::secondary_button(ui, "Cancel").clicked() {
                    app.active_dialog = ActiveDialog::None;
                }
            });
        });
}

fn render_log_terminal(app: &mut App, ctx: &egui::Context) {
    egui::Window::new("Log Terminal")
        .collapsible(true)
        .resizable(true)
        .default_width(600.0)
        .default_height(400.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("📋 Copy All Logs").clicked() {
                    let log_text = crate::support::format_event_logs(&app.event_collector);
                    ui.output_mut(|o| o.copied_text = log_text);
                    if let Ok(mut manager) = app.chat_manager.try_lock() {
                        manager.add_toast(
                            crate::types::ToastLevel::Success,
                            "Logs copied to clipboard".to_string(),
                        );
                    }
                }
                if ui.button("🧰 Export Diagnostics").clicked() {
                    app.export_diagnostics_bundle();
                }
                if ui.button("🗑 Clear Logs").clicked() {
                    // Optional: Clear logs if API allows. EventCollector usually doesn't expose a clear() that is easy.
                    // But let's leave it for now or check if it exists.
                }
            });
            ui.separator();

            let avail = ui.available_size();
            egui::ScrollArea::both()
                .auto_shrink([true, true])
                .show(ui, |ui| {
                    // Constrain the Logs widget to the current window size to prevent growth
                    ui.add_sized(avail, Logs::new(app.event_collector.clone()));
                });
        });
}
