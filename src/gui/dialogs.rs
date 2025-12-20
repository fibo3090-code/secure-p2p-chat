use crate::gui::app_ui::App;
use crate::gui::widgets::ColorGrid;
use crate::util::{generate_color_grid, primary_local_ipv4};
use eframe::egui;
use egui_tracing::ui::Logs;

pub fn render_dialogs(app: &mut App, ctx: &egui::Context) {
    if app.show_welcome {
        render_welcome(app, ctx);
    }

    if let Some(chat_id) = app.chat_to_delete {
        render_delete_confirmation(app, ctx, chat_id);
    }

    if app.show_host_dialog {
        render_host_dialog(app, ctx);
    }

    if app.show_connect_dialog {
        render_connect_dialog(app, ctx);
    }

    if app.show_contacts {
        render_contacts_window(app, ctx);
    }

    if app.show_add_contact {
        render_add_contact_dialog(app, ctx);
    }

    if app.show_create_group {
        render_create_group_wizard(app, ctx);
    }

    if app.show_rename_dialog {
        render_rename_dialog(app, ctx);
    }

    if app.show_settings {
        render_settings_dialog(app, ctx);
    }

    if app.show_about {
        crate::gui::help_view::render_help_window(app, ctx);
    }

    if app.show_fingerprint_dialog {
        render_fingerprint_dialog(app, ctx);
    }

    if app.show_password_dialog {
        render_password_dialog(app, ctx);
    }

    if app.show_set_password_dialog {
        render_set_password_dialog(app, ctx);
    }

    if app.show_remove_password_dialog {
        render_remove_password_dialog(app, ctx);
    }

    if app.show_log_terminal {
        render_log_terminal(app, ctx);
    }

    if app.show_clear_history_dialog {
        render_clear_history_dialog(app, ctx);
    }
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
                        app.show_fingerprint_dialog = false;
                    }
                    if crate::gui::widgets::secondary_button(ui, "❌ Reject").clicked() {
                        if let Ok(mut manager) = app.chat_manager.try_lock() {
                            // Notify session/task that the fingerprint is rejected so it can abort
                            let _ = manager.confirm_fingerprint(chat_id, false);
                            // Remove chat locally
                            manager.delete_chat(chat_id);
                        }
                        app.show_fingerprint_dialog = false;
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
            ui.label("📁 Secure file transfer with progress tracking");
            ui.label("👥 Direct peer-to-peer connections (no server!)");
            ui.label("🛡️ Fingerprint verification for security");
            ui.label("💾 Message history persistence");
            ui.label("😊 Emoji picker, typing indicators, desktop notifications");

            ui.add_space(15.0);
            ui.separator();
            ui.add_space(10.0);

            ui.heading("🚀 Getting Started:");
            ui.add_space(5.0);

            ui.label("1️⃣ Host Mode: Start hosting to accept connections");
            ui.label("   • Click 'Connection' → 'Start Host'");
            ui.label("   • Share your IP address with others");
            ui.add_space(5.0);

            ui.label("2️⃣ Client Mode: Connect to someone hosting");
            ui.label("   • Click 'Connection' → 'Connect to Host'");
            ui.label("   • Enter the host's IP address and port");
            ui.add_space(5.0);

            ui.label("3️⃣ Verify Fingerprints: Always verify the fingerprint!");
            ui.label("   • Compare fingerprints via another channel");
            ui.label("   • This protects against man-in-the-middle attacks");

            ui.add_space(15.0);
            ui.separator();
            ui.add_space(10.0);

            ui.vertical_centered(|ui| {
                if ui
                    .button(egui::RichText::new("Let's Get Started! 🚀").size(16.0))
                    .clicked()
                {
                    app.show_welcome = false;
                }
                ui.add_space(5.0);
                if ui.small_button("Show this again later").clicked() {
                    app.show_welcome = false;
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
        .fixed_pos(egui::pos2(ctx.screen_rect().width() - 320.0, 60.0))
        .show(ctx, |ui| {
            ui.set_max_width(300.0);

            for toast in &all_toasts {
                let elapsed = toast.created_at.elapsed();
                let progress = elapsed.as_secs_f32() / toast.duration.as_secs_f32();

                if progress < 1.0 {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        let color = match toast.level {
                            crate::types::ToastLevel::Info => crate::gui::styling::ACCENT_PRIMARY,
                            crate::types::ToastLevel::Success => crate::gui::styling::SUCCESS,
                            crate::types::ToastLevel::Warning => crate::gui::styling::WARNING,
                            crate::types::ToastLevel::Error => crate::gui::styling::ERROR,
                        };

                        ui.colored_label(color, &toast.message);
                    });

                    ui.add_space(4.0);
                }
            }
        });

    // Cleanup expired app-level toasts
    let now = std::time::Instant::now();
    app.toasts
        .retain(|toast| now.duration_since(toast.created_at) < toast.duration);
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
                        // Auto-save after deletion
                        let _ = manager.save_history(&app.history_path);
                    }
                    app.chat_to_delete = None;
                }
                if crate::gui::widgets::secondary_button(ui, "Cancel").clicked() {
                    app.chat_to_delete = None;
                }
            });
        });
}

fn render_host_dialog(app: &mut App, ctx: &egui::Context) {
    egui::Window::new("Start Host")
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label("Port:");
            ui.text_edit_singleline(&mut app.host_port);

            ui.horizontal(|ui| {
                if crate::gui::widgets::primary_button(ui, "Start").clicked() {
                    tracing::info!("Start host button clicked");
                    app.start_host_clicked();
                    app.show_host_dialog = false;
                }

                if crate::gui::widgets::secondary_button(ui, "Cancel").clicked() {
                    app.show_host_dialog = false;
                }
            });
        });
}

fn render_connect_dialog(app: &mut App, ctx: &egui::Context) {
    egui::Window::new("Connect to Host")
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label("Host:");
            ui.text_edit_singleline(&mut app.connect_host);

            ui.label("Port:");
            ui.text_edit_singleline(&mut app.connect_port);

            ui.horizontal(|ui| {
                if crate::gui::widgets::primary_button(ui, "Connect").clicked() {
                    tracing::info!("Connect to host button clicked");
                    app.connect_clicked();
                    app.show_connect_dialog = false;
                }

                if crate::gui::widgets::secondary_button(ui, "Cancel").clicked() {
                    app.show_connect_dialog = false;
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
                    app.show_add_contact = true;
                }

                if ui.button("🧩 Create Group").clicked() {
                    app.show_create_group = true;
                    app.group_selected.clear();
                }
            });

            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                if let Ok(manager) = app.chat_manager.try_lock() {
                    for contact in manager.contacts.values() {
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
                                // Check if there's already a mapped chat for this contact
                                let existing_chat_id = {
                                    if let Ok(manager) = app.chat_manager.try_lock() {
                                        manager.contact_to_chat.get(&contact.id).copied()
                                    } else {
                                        None
                                    }
                                };

                                if let Some(chat_id) = existing_chat_id {
                                    // If there's a mapped chat, select it.
                                    app.selected_chat = Some(chat_id);
                                    app.show_contacts = false;
                                } else {
                                    // Otherwise, create a new chat entry locally first for responsiveness.
                                    let chat_id = uuid::Uuid::new_v4();
                                    app.selected_chat = Some(chat_id);

                                    // If contact has no address, prompt user to open Connect dialog and bind to this chat
                                    let should_prompt_connect = contact.address.as_ref().map(|s| s.trim().is_empty()).unwrap_or(true);
                                    if should_prompt_connect {
                                        // Pre-open connect dialog; the connect action will now bind to selected_chat
                                        app.show_connect_dialog = true;
                                    }

                                    // Clone the necessary data before spawning the task
                                    let manager_clone = app.chat_manager.clone();
                                    let contact_clone = contact.clone();
                                    let history_path = app.history_path.clone();

                                    // Spawn a task to do the real work: create chat in manager and connect.
                                    tokio::spawn(async move {
                                        let mut mgr = manager_clone.lock().await;
                                        // 1. Create the chat object and add it to the manager
                                        let chat = crate::types::Chat {
                                            id: chat_id,
                                            title: contact_clone.name.clone(),
                                            peer_fingerprint: contact_clone.fingerprint.clone(),
                                            participants: vec![contact_clone.id],
                                            messages: Vec::new(),
                                            created_at: chrono::Utc::now(),
                                            peer_typing: false,
                                            typing_since: None,
                                            send_seq: 0,
                                            recv_seq: 0,
                                        };
                                        mgr.chats.insert(chat_id, chat);
                                        mgr.associate_contact_with_chat(contact_clone.id, chat_id);

                                        // 2. Save history
                                        if let Err(e) = mgr.save_history(&history_path) {
                                            tracing::error!("Failed to save history after creating chat: {}", e);
                                        }

                                        // 3. Asynchronously connect to the peer — only if an address is present
                                        if contact_clone.address.is_some() {
                                            if let Err(e) = mgr.connect_to_contact(contact_clone.id, Some(chat_id)).await {
                                                mgr.add_toast(
                                                    crate::types::ToastLevel::Error,
                                                    format!("Failed to connect to {}: {}", contact_clone.name, e),
                                                );
                                            }
                                        } else {
                                            // Inform the user a connection is needed via Connect dialog
                                            mgr.add_toast(
                                                crate::types::ToastLevel::Info,
                                                format!("No address for {}. Open 'Connect to Host' to connect this chat.", contact_clone.name),
                                            );
                                        }
                                    });
                                    app.show_contacts = false; // Close dialog after action
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
                }
            });

            ui.horizontal(|ui| {
                if ui.button("Close").clicked() {
                    app.show_contacts = false;
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
                                app.show_add_contact = false;
                            }
                        }

                        if crate::gui::widgets::secondary_button(ui, "Cancel").clicked() {
                            app.show_add_contact = false;
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

                    if !app.invite_link_input.is_empty() {
                        ui.label(
                            egui::RichText::new("✅ Link detected")
                                .color(crate::gui::styling::SUCCESS),
                        );
                        // Attempt to parse the link and pre-fill fields
                        if let Ok(manager) = app.chat_manager.try_lock() {
                            match manager.parse_invite_link(&app.invite_link_input) {
                                Ok(contact) => {
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

                            if !name.is_empty() {
                                tracing::info!("Adding contact from link: {}", name);
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

                                app.invite_link_input.clear();
                                app.new_contact_name.clear();
                                app.new_contact_address.clear();
                                app.new_contact_fingerprint.clear();
                                app.new_contact_pubkey.clear();
                                app.show_add_contact = false;
                            }
                        }

                        if crate::gui::widgets::secondary_button(ui, "Cancel").clicked() {
                            app.show_add_contact = false;
                        }
                    });
                }
                // Share My Link tab (NEW!)
                2 => {
                    ui.label("📤 Share this link with your friends so they can add you:");
                    ui.add_space(10.0);

                    // Generate link using actual identity and best-effort local address
                    if app.my_invite_link.is_none() {
                        if let Ok(manager) = app.chat_manager.try_lock() {
                            let port = manager.config.listen_port;
                            let invite_addr =
                                primary_local_ipv4().map(|ip| format!("{}:{}", ip, port));
                            match app.identity.generate_invite_link(invite_addr) {
                                Ok(link) => {
                                    app.my_invite_link = Some(link);
                                }
                                Err(e) => {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "❌ Failed to generate link: {}",
                                            e
                                        ))
                                        .color(crate::gui::styling::ERROR),
                                    );
                                }
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
                                    let image = image::load_from_memory_with_format(
                                        &qr_bytes,
                                        image::ImageFormat::Png,
                                    )
                                    .expect("Failed to load QR image");
                                    let size = [image.width() as _, image.height() as _];
                                    let image_buffer = image.to_rgba8();
                                    let pixels = image_buffer.as_flat_samples();
                                    let texture = ctx.load_texture(
                                        "qr_code",
                                        egui::ImageData::Color(std::sync::Arc::new(egui::ColorImage {
                                            size,
                                            pixels: pixels.as_slice().to_vec().chunks_exact(4).map(|p| egui::Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3])).collect(),
                                        })),
                                        egui::TextureOptions::LINEAR,
                                    );
                                    app.qr_code_texture = Some(texture);
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
                        app.show_add_contact = false;
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
                            app.show_create_group = false;
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
                            app.show_create_group = false;
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
                            app.show_create_group = false;
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
                                app.show_create_group = false;
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
        let mut close_dialog = false;
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
                            let _ = manager.save_history(&app.history_path);
                            ctx.request_repaint();
                            close_dialog = true;
                        }
                    } else {
                        show_lock_error = true;
                    }
                }

                if crate::gui::widgets::secondary_button(ui, "❌ Cancel").clicked() {
                    close_dialog = true;
                }
            });

        if show_lock_error {
            app.add_toast(
                crate::types::ToastLevel::Error,
                "Could not lock chat manager. Please try again.".to_string(),
            );
        }

        if close_dialog {
            app.show_rename_dialog = false;
            app.rename_chat_id = None;
            app.rename_input.clear();
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
                        let _ = manager.save_history(&app.history_path);
                        // If enabled, start hosting immediately using current listen_port
                        if auto_host {
                            let port = manager.config.listen_port;
                            let mgr_arc = app.chat_manager.clone();
                            tokio::spawn(async move {
                                let mut mgr = mgr_arc.lock().await;
                                if let Err(e) = mgr.start_host(port).await {
                                    mgr.add_toast(
                                        crate::types::ToastLevel::Error,
                                        format!("Failed to start host: {}", e),
                                    );
                                }
                            });
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
                            let _ = manager.save_history(&app.history_path);
                        }
                    }
                });

                // Show my IP address (best-effort primary local IPv4)
                ui.add_space(8.0);
                ui.label("My IP address (primary, best-effort):");
                let my_ip = {
                    use std::net::{SocketAddr, UdpSocket};
                    (|| -> Option<String> {
                        let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
                        // Use a public resolver to determine the outbound interface without sending data
                        sock.connect("8.8.8.8:80").ok()?;
                        let addr: SocketAddr = sock.local_addr().ok()?;
                        Some(addr.ip().to_string())
                    })()
                    .unwrap_or_else(|| "Unavailable".to_string())
                };
                ui.monospace(my_ip);

                ui.add_space(10.0);

                ui.label("Download Directory:");
                ui.horizontal(|ui| {
                    ui.label(manager.config.download_dir.display().to_string());
                    if ui.button("📁 Browse").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            manager.config.download_dir = path;
                            let _ = manager.save_history(&app.history_path);
                        }
                    }
                });

                ui.add_space(10.0);

                if ui.checkbox(
                    &mut manager.config.auto_accept_files,
                    "Auto-accept file transfers",
                ).changed() {
                    let _ = manager.save_history(&app.history_path);
                }

                ui.add_space(10.0);

                ui.label("Maximum file size:");
                let mut max_size_mb = (manager.config.max_file_size / (1024 * 1024)) as u32;
                if ui.add(egui::Slider::new(&mut max_size_mb, 1..=10240).suffix(" MB")).changed() {
                    manager.config.max_file_size = (max_size_mb as u64) * 1024 * 1024;
                    let _ = manager.save_history(&app.history_path);
                }

                ui.add_space(10.0);

                if ui.checkbox(
                    &mut manager.config.enable_notifications,
                    "Enable desktop notifications",
                ).changed() {
                    let _ = manager.save_history(&app.history_path);
                }

                ui.add_space(10.0);

                if ui.checkbox(
                    &mut manager.config.enable_typing_indicators,
                    "Enable typing indicators",
                ).changed() {
                    let _ = manager.save_history(&app.history_path);
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
                            ui.selectable_value(&mut manager.config.theme, crate::types::Theme::Forest, "Forest").changed()
                        }).inner.unwrap_or(false) {
                            let _ = manager.save_history(&app.history_path);
                            // Apply theme immediately
                            ctx.set_visuals(crate::gui::styling::apply_custom_visuals(&manager.config.theme));
                        }
                });

                ui.add_space(10.0);

                // Font size slider
                ui.horizontal(|ui| {
                    ui.label("Font Size:");
                    if ui.add(egui::Slider::new(&mut manager.config.font_size, 10..=20).suffix("px")).changed() {
                        let _ = manager.save_history(&app.history_path);
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
                    let _ = manager.save_history(&app.history_path);
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
                            let _ = manager.save_history(&app.history_path);
                        }
                });

                ui.add_space(10.0);

                let mut show_log = app.show_log_terminal;
                if ui.checkbox(&mut show_log, "Show Log Terminal").changed() {
                    app.show_log_terminal = show_log;
                    manager.config.show_log_terminal = show_log;
                    let _ = manager.save_history(&app.history_path);
                }
            }

            ui.add_space(20.0);
            ui.heading("Security");
            ui.separator();
            ui.add_space(10.0);

            ui.horizontal(|ui| {
                if crate::gui::widgets::primary_button(ui, "Set/Change Password").clicked() {
                    app.show_set_password_dialog = true;
                }

                if app.identity.encrypted_private_key.is_some()
                    && crate::gui::widgets::secondary_button(ui, "Remove Password").clicked() {
                        app.show_remove_password_dialog = true;
                    }
            });


            ui.add_space(20.0);
            ui.heading("Danger Zone");
            ui.separator();
            ui.add_space(10.0);

            if crate::gui::widgets::primary_button(ui, "Clear Chat History").clicked() {
                app.show_clear_history_dialog = true;
            }

            ui.add_space(10.0);
            ui.separator();
            ui.horizontal(|ui| {
                if crate::gui::widgets::secondary_button(ui, "Close").clicked() {
                    app.show_settings = false;
                }
            });
        });
}

fn render_password_dialog(app: &mut App, ctx: &egui::Context) {
    egui::Window::new("🔒 Identity Locked")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label("Please enter your password to unlock your identity.");
            ui.add_space(10.0);

            let response = ui.add(
                egui::TextEdit::singleline(&mut app.password_input)
                    .password(true)
                    .hint_text("Password"),
            );
            ui.add_space(10.0);

            ui.horizontal(|ui| {
                if crate::gui::widgets::primary_button(ui, "🔓 Unlock").clicked()
                    || (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                {
                    match app.identity.decrypt(&app.password_input) {
                        Ok(_) => {
                            app.identity_locked = false;
                            app.show_password_dialog = false;
                            app.password_input.clear();
                            if let Ok(mut manager) = app.chat_manager.try_lock() {
                                manager.add_toast(
                                    crate::types::ToastLevel::Success,
                                    "Identity unlocked!".to_string(),
                                );
                            }
                        }
                        Err(e) => {
                            if let Ok(mut manager) = app.chat_manager.try_lock() {
                                manager.add_toast(
                                    crate::types::ToastLevel::Error,
                                    format!("Failed to decrypt: {}", e),
                                );
                            }
                        }
                    }
                }
            });
        });
}

pub fn render_set_password_dialog(app: &mut App, ctx: &egui::Context) {
    egui::Window::new("🔑 Set Password")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label("Enter a new password to encrypt your identity file.");
            ui.label("If you forget this password, you will lose access to your identity.");
            ui.add_space(10.0);

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
            ui.add_space(10.0);

            if app.new_password_input != app.confirm_password_input {
                ui.colored_label(crate::gui::styling::ERROR, "Passwords do not match.");
            }

            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !app.new_password_input.is_empty()
                            && app.new_password_input == app.confirm_password_input,
                        egui::Button::new("Set Password"),
                    )
                    .clicked()
                {
                    match app.identity.encrypt(&app.new_password_input) {
                        Ok(_) => {
                            if let Err(e) = app
                                .identity
                                .save(&app.history_path.with_file_name("identity.json"))
                            {
                                if let Ok(mut manager) = app.chat_manager.try_lock() {
                                    manager.add_toast(
                                        crate::types::ToastLevel::Error,
                                        format!("Failed to save identity: {}", e),
                                    );
                                }
                            } else {
                                if let Ok(mut manager) = app.chat_manager.try_lock() {
                                    manager.add_toast(
                                        crate::types::ToastLevel::Success,
                                        "Password set and identity encrypted!".to_string(),
                                    );
                                }
                                if app.is_new_identity {
                                    app.is_new_identity = false;
                                    app.show_welcome = true;
                                }
                                app.show_set_password_dialog = false;
                                app.new_password_input.clear();
                                app.confirm_password_input.clear();
                            }
                        }
                        Err(e) => {
                            if let Ok(mut manager) = app.chat_manager.try_lock() {
                                manager.add_toast(
                                    crate::types::ToastLevel::Error,
                                    format!("Failed to encrypt: {}", e),
                                );
                            }
                        }
                    }
                }

                if ui.add_enabled(!app.is_new_identity, egui::Button::new("Cancel")).clicked() {
                    app.show_set_password_dialog = false;
                    app.new_password_input.clear();
                    app.confirm_password_input.clear();
                }
            });
        });
}

fn render_remove_password_dialog(app: &mut App, ctx: &egui::Context) {
    egui::Window::new("🔑 Remove Password")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label("Enter your current password to remove encryption from your identity file.");
            ui.add_space(10.0);

            let response = ui.add(
                egui::TextEdit::singleline(&mut app.remove_password_input)
                    .password(true)
                    .hint_text("Current Password"),
            );
            ui.add_space(10.0);

            ui.horizontal(|ui| {
                if (crate::gui::widgets::primary_button(ui, "Confirm Removal").clicked()
                    || (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))))
                    && !app.remove_password_input.is_empty()
                {
                    match app.identity.remove_password(&app.remove_password_input) {
                        Ok(_) => {
                            if let Err(e) = app
                                .identity
                                .save(&app.history_path.with_file_name("identity.json"))
                            {
                                if let Ok(mut manager) = app.chat_manager.try_lock() {
                                    manager.add_toast(
                                        crate::types::ToastLevel::Error,
                                        format!("Failed to save identity: {}", e),
                                    );
                                }
                            } else {
                                if let Ok(mut manager) = app.chat_manager.try_lock() {
                                    manager.add_toast(
                                        crate::types::ToastLevel::Success,
                                        "Password removed and identity is now unencrypted!".to_string(),
                                    );
                                }
                                app.show_remove_password_dialog = false;
                                app.remove_password_input.clear();
                            }
                        }
                        Err(e) => {
                            if let Ok(mut manager) = app.chat_manager.try_lock() {
                                manager.add_toast(
                                    crate::types::ToastLevel::Error,
                                    format!("Failed to remove password: {}", e),
                                );
                            }
                        }
                    }
                }

                if crate::gui::widgets::secondary_button(ui, "Cancel").clicked() {
                    app.show_remove_password_dialog = false;
                    app.remove_password_input.clear();
                }
            });
        });
}

fn render_clear_history_dialog(app: &mut App, ctx: &egui::Context) {
    egui::Window::new("⚠️ Clear Chat History")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label("Are you sure you want to clear all chat history?");
            ui.label(
                egui::RichText::new(
                    "This action cannot be undone and will delete all messages and contacts.",
                )
                .color(crate::gui::styling::ERROR),
            );
            ui.add_space(10.0);

            ui.horizontal(|ui| {
                if crate::gui::widgets::primary_button(ui, "❌ Clear All").clicked() {
                    if let Ok(mut manager) = app.chat_manager.try_lock() {
                        manager.clear_history(&app.history_path);
                        app.selected_chat = None;
                        manager.add_toast(
                            crate::types::ToastLevel::Success,
                            "Chat history cleared!".to_string(),
                        );
                    }
                    app.show_clear_history_dialog = false;
                }
                if crate::gui::widgets::secondary_button(ui, "Cancel").clicked() {
                    app.show_clear_history_dialog = false;
                }
            });
        });
}

fn render_log_terminal(_app: &mut App, ctx: &egui::Context) {
    egui::Window::new("Log Terminal")
        .collapsible(true)
        .resizable(true)
        .default_width(600.0)
        .default_height(400.0)
        .show(ctx, |ui| {
            let avail = ui.available_size();
            egui::ScrollArea::both()
                .auto_shrink([true, true])
                .show(ui, |ui| {
                    // Constrain the Logs widget to the current window size to prevent growth
                    ui.add_sized(avail, Logs::new(_app.event_collector.clone()));
                });
        });
}
