use crate::gui::app_ui::App;
use eframe::egui;
use crate::gui::widgets::ColorGrid;
use crate::util::generate_color_grid;

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
        render_about_dialog(app, ctx);
    }

    if app.show_fingerprint_dialog {
        render_fingerprint_dialog(app, ctx);
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
    let toasts = if let Ok(manager) = app.chat_manager.try_lock() {
        manager.toasts.clone()
    } else {
        Vec::new()
    };

    egui::Area::new(egui::Id::new("toasts"))
        .fixed_pos(egui::pos2(ctx.screen_rect().width() - 320.0, 60.0))
        .show(ctx, |ui| {
            ui.set_max_width(300.0);

            for toast in &toasts {
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
                                        };
                                        mgr.chats.insert(chat_id, chat);
                                        mgr.associate_contact_with_chat(contact_clone.id, chat_id);

                                        // 2. Save history
                                        if let Err(e) = mgr.save_history(&history_path) {
                                            tracing::error!("Failed to save history after creating chat: {}", e);
                                        }

                                        // 3. Asynchronously connect to the peer
                                        if let Err(e) = mgr.connect_to_contact(contact_clone.id, Some(chat_id)).await {
                                            mgr.add_toast(
                                                crate::types::ToastLevel::Error,
                                                format!("Failed to connect to {}: {}", contact_clone.name, e),
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
                    .button(egui::RichText::new("📝 Manual").color(if app.contact_tab == 0 {
                        crate::gui::styling::ACCENT_PRIMARY
                    } else {
                        crate::gui::styling::SUBTLE_TEXT_COLOR
                    }))
                    .clicked()
                {
                    app.contact_tab = 0;
                }
                if ui
                    .button(egui::RichText::new("🔗 Invite Link").color(if app.contact_tab == 1 {
                        crate::gui::styling::ACCENT_PRIMARY
                    } else {
                        crate::gui::styling::SUBTLE_TEXT_COLOR
                    }))
                    .clicked()
                {
                    app.contact_tab = 1;
                }
                if ui
                    .button(egui::RichText::new("📤 Share My Link").color(if app.contact_tab == 2 {
                        crate::gui::styling::ACCENT_PRIMARY
                    } else {
                        crate::gui::styling::SUBTLE_TEXT_COLOR
                    }))
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

                            if !name.is_empty() {
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

                    ui.label("Paste invite link (chat-p2p://invite/...");
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
                                    app.new_contact_name = contact.name;
                                    app.new_contact_address = contact.address.unwrap_or_default();
                                    app.new_contact_fingerprint = contact.fingerprint.unwrap_or_default();
                                    app.new_contact_pubkey = contact.public_key.unwrap_or_default();
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

                    // Generate link using actual identity
                    if app.my_invite_link.is_none() {
                        // For now, we'll use a placeholder address for the invite link.
                        // In a real-world scenario, this would be the user's public IP and listening port.
                        let my_address = Some("YOUR_IP:PORT".to_string()); 
                        match app.identity.generate_invite_link(my_address) {
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

                    if let Some(link) = &app.my_invite_link {
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(link).monospace());
                                if crate::gui::widgets::secondary_button(ui, "📋 Copy").clicked() {
                                    ui.output_mut(|o| o.copied_text = link.clone());
                                }
                            });
                        });
                    }

                    ui.add_space(10.0);

                    let grid = generate_color_grid(&app.identity.fingerprint);
                    ui.add(ColorGrid::new(grid));

                    ui.add_space(10.0);
                    ui.label("💡 Tip: You can share this via:");
                    ui.label("  • Email, WhatsApp, SMS");
                    ui.label("  • QR code (future feature)");

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
        egui::Window::new("Rename Conversation")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("New title:");
                let response = ui.text_edit_singleline(&mut app.rename_input);
                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    if crate::gui::widgets::primary_button(ui, "✅ Save").clicked() {
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
                                // Save history to persist changes
                                let _ = manager.save_history(&app.history_path);
                            }
                        }
                        app.show_rename_dialog = false;
                        app.rename_chat_id = None;
                        app.rename_input.clear();
                    }

                    if crate::gui::widgets::secondary_button(ui, "❌ Cancel").clicked() {
                        app.show_rename_dialog = false;
                        app.rename_chat_id = None;
                        app.rename_input.clear();
                    }
                });

                // Allow Enter key to save
                if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    if let Ok(mut manager) = app.chat_manager.try_lock() {
                        let _ = manager.rename_chat(chat_id, app.rename_input.clone());
                        let _ = manager.save_history(&app.history_path);
                    }
                    app.show_rename_dialog = false;
                    app.rename_chat_id = None;
                    app.rename_input.clear();
                }
            });
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
                ui.label("Download Directory:");
                ui.horizontal(|ui| {
                    ui.label(manager.config.download_dir.display().to_string());
                    if ui.button("📁 Browse").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            manager.config.download_dir = path;
                        }
                    }
                });

                ui.add_space(10.0);

                ui.checkbox(
                    &mut manager.config.auto_accept_files,
                    "Auto-accept file transfers",
                );

                ui.add_space(10.0);

                ui.label("Maximum file size:");
                let mut max_size_mb = (manager.config.max_file_size / (1024 * 1024)) as u32;
                ui.add(egui::Slider::new(&mut max_size_mb, 1..=10240).suffix(" MB"));
                manager.config.max_file_size = (max_size_mb as u64) * 1024 * 1024;

                ui.add_space(10.0);

                ui.checkbox(
                    &mut manager.config.enable_notifications,
                    "Enable desktop notifications",
                );

                ui.add_space(10.0);

                ui.checkbox(
                    &mut manager.config.enable_typing_indicators,
                    "Enable typing indicators",
                );
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

fn render_about_dialog(app: &mut App, ctx: &egui::Context) {
    egui::Window::new("ℹ️ About")
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.heading("Encrypted P2P Messenger");
                ui.label("Version 1.2.0");
                ui.add_space(10.0);
            });

            ui.separator();
            ui.add_space(10.0);

            ui.label("A secure, peer-to-peer messaging application");
            ui.label("with end-to-end encryption and forward secrecy.");
            ui.add_space(10.0);

            ui.label("🔒 Encryption: RSA-2048-OAEP + AES-256-GCM");
            ui.label("🔐 Forward Secrecy: X25519 ECDH + HKDF-SHA256");
            ui.label("🛡️ Security: Fingerprint verification");
            ui.label("📁 Features: File transfer, message history, typing indicators");
            ui.add_space(10.0);

            ui.separator();
            ui.add_space(10.0);

            ui.vertical_centered(|ui| {
                if crate::gui::widgets::secondary_button(ui, "Close").clicked() {
                    app.show_about = false;
                }
            });
        });
}
