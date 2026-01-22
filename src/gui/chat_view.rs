use crate::gui::app_ui::App;
use crate::types::{Message, MessageContent};
use eframe::egui::{self, Color32};
use uuid::Uuid;

pub fn render_chat(app: &mut App, ui: &mut egui::Ui, chat_id: Uuid) {
    // Handle dropped files
    let dropped_files = ui.input(|i| i.raw.dropped_files.clone());
    if !dropped_files.is_empty() {
        if let Some(file) = dropped_files.first() {
            if let Some(path) = &file.path {
                app.file_to_send = Some(path.clone());
            }
        }
    }

    // Header with connection status
    egui::TopBottomPanel::top("chat_header")
        .exact_height(60.0)
        .show_inside(ui, |ui| {
            if let Ok(manager) = app.chat_manager.try_lock() {
                if let Some(chat) = manager.get_chat(chat_id) {
                    let connected = manager.is_connected(&chat_id);
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        // Avatar
                        let color = if let Some(fp) = &chat.peer_fingerprint {
                            crate::gui::widgets::fingerprint_to_color(fp)
                        } else {
                            egui::Color32::GRAY
                        };

                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(40.0, 40.0), egui::Sense::hover());
                        ui.painter().circle_filled(rect.center(), 20.0, color);

                        let initials = crate::gui::widgets::get_initials(&chat.title);
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            initials,
                            egui::FontId::proportional(16.0),
                            egui::Color32::WHITE,
                        );

                        ui.add_space(8.0);

                        // Title and status
                        ui.vertical(|ui| {
                            ui.heading(&chat.title);
                            // Show typing indicator or connection status
                            if chat.peer_typing {
                                ui.label(
                                    egui::RichText::new("✍️ typing...")
                                        .size(12.0)
                                        .color(ui.visuals().text_color().gamma_multiply(0.7)),
                                );
                            } else if connected {
                                ui.label(
                                    egui::RichText::new("🟢 Connected")
                                        .size(12.0)
                                        .color(crate::gui::styling::SUCCESS),
                                );
                            } else {
                                ui.label(
                                    egui::RichText::new("🟠 Disconnected")
                                        .size(12.0)
                                        .color(crate::gui::styling::WARNING),
                                );
                            }
                        });

                        // Fingerprint on right
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if let Some(fp) = &chat.peer_fingerprint {
                                if ui.button("📋 Copy Fingerprint").clicked() {
                                    ui.output_mut(|o| o.copied_text = fp.clone());
                                }
                                ui.monospace(crate::util::format_fingerprint_short(fp));
                            }

                            if !connected {
                                // Offer reconnect if a contact is associated
                                if let Some((&contact_id, _)) = manager
                                    .contact_to_chat
                                    .iter()
                                    .find(|&(_, &cid)| cid == chat_id)
                                {
                                    if ui.button("Retry connect").clicked() {
                                        let mgr = app.chat_manager.clone();
                                        tokio::spawn(async move {
                                            let mut m = mgr.lock().await;
                                            if let Err(e) = m
                                                .connect_to_contact(contact_id, Some(chat_id))
                                                .await
                                            {
                                                m.add_toast(
                                                    crate::types::ToastLevel::Error,
                                                    format!("Reconnect failed: {}", e),
                                                );
                                            }
                                        });
                                    }
                                }
                            }
                        });
                    });
                }
            }
        });

    // Input area - FIXED AT BOTTOM
    egui::TopBottomPanel::bottom("chat_input")
        .min_height(70.0) // Minimum height for the input panel
        .max_height(200.0) // Maximum height for the input panel
        .show_inside(ui, |ui| {
            ui.add_space(5.0);

            // File preview if selected
            if let Some(file_path) = app.file_to_send.clone() {
                let filename = file_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown".to_string());

                ui.horizontal(|ui| {
                    ui.label("📎 File to send:");
                    ui.label(
                        egui::RichText::new(&filename)
                            .strong()
                            .color(ui.visuals().selection.bg_fill),
                    );
                    if ui.small_button("❌ Cancel").clicked() {
                        app.file_to_send = None;
                    }
                    if ui.button("✅ Send File").clicked() {
                        // Implement file sending
                        if let Some(path) = app.file_to_send.take() {
                            let manager = app.chat_manager.clone();
                            tokio::spawn(async move {
                                let mut mgr = manager.lock().await;
                                if let Err(e) = mgr.send_file(chat_id, path).await {
                                    mgr.add_toast(
                                        crate::types::ToastLevel::Error,
                                        format!("Failed to send file: {}", e),
                                    );
                                }
                            });
                        }
                    }
                });
                ui.separator();
            }

            // Input bar
            ui.horizontal(|ui| {
                // File attach button
                if ui
                    .button(egui::RichText::new("📎").size(20.0))
                    .on_hover_text("Attach file (or drag & drop)")
                    .clicked()
                {
                    if let Some(path) = rfd::FileDialog::new().pick_file() {
                        app.file_to_send = Some(path);
                    }
                }

                // Emoji picker button
                if ui
                    .button(egui::RichText::new("😊").size(20.0))
                    .on_hover_text("Emoji picker")
                    .clicked()
                {
                    app.show_emoji_picker = !app.show_emoji_picker;
                }

                // Multiline text input
                // let text_width = ui.available_width() - 70.0; // This is no longer needed directly
                let response = egui::ScrollArea::vertical()
                    .max_height(100.0) // Set max height for the text input before it scrolls
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut app.input_text)
                                .hint_text("💬 Type a message... (Ctrl+Enter to send)")
                                .desired_rows(1) // Start with 1 desired row, let it grow
                                .lock_focus(false)
                                .desired_width(ui.available_width()), // Use available width
                        )
                    })
                    .inner; // Get the response from the inner TextEdit

                // Handle typing indicators
                if response.changed() && !app.input_text.is_empty() {
                    let now = std::time::Instant::now();
                    let should_send_typing = app
                        .last_typing_time
                        .is_none_or(|last| now.duration_since(last).as_secs() >= 2);

                    if should_send_typing {
                        let manager = app.chat_manager.clone();
                        tokio::spawn(async move {
                            let mut mgr = manager.lock().await;
                            let _ = mgr.send_typing_start(chat_id);
                        });
                        app.last_typing_time = Some(now);
                        app.typing_stopped = false;
                    }
                }

                // Stop typing when text is cleared or after timeout
                if app.input_text.is_empty() && !app.typing_stopped {
                    let manager = app.chat_manager.clone();
                    tokio::spawn(async move {
                        let mut mgr = manager.lock().await;
                        let _ = mgr.send_typing_stop(chat_id);
                    });
                    app.typing_stopped = true;
                }

                // Handle keyboard shortcuts
                if response.has_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.ctrl)
                {
                    app.send_message_clicked(chat_id);
                    // Stop typing on send
                    let manager = app.chat_manager.clone();
                    tokio::spawn(async move {
                        let mut mgr = manager.lock().await;
                        let _ = mgr.send_typing_stop(chat_id);
                    });
                    app.typing_stopped = true;
                }

                // Send button
                let send_enabled = !app.input_text.trim().is_empty();
                let mut send_button =
                    egui::Button::new(egui::RichText::new("📤\nSend").size(14.0).strong())
                        .min_size(egui::vec2(65.0, 70.0));

                if send_enabled {
                    send_button = send_button.fill(ui.visuals().selection.bg_fill);
                }

                if ui.add_enabled(send_enabled, send_button).clicked() {
                    app.send_message_clicked(chat_id);
                }
            });
        });

    // Emoji picker overlay
    if app.show_emoji_picker {
        egui::Window::new("😊 Emoji Picker")
            .resizable(false)
            .collapsible(false)
            .default_width(300.0)
            .show(ui.ctx(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    let common_emojis = [
                        "😊", "😂", "❤️", "👍", "👎", "🎉", "🔥", "💯", "😍", "😎", "😢", "😭",
                        "😡", "🤔", "👋", "🙏", "✨", "⭐", "💪", "👏", "🎊", "🎈", "🚀", "💡",
                        "📱", "💻", "📷", "🎵", "🎮", "⚽", "🍕", "🍰",
                    ];

                    for emoji in &common_emojis {
                        if ui.button(egui::RichText::new(*emoji).size(24.0)).clicked() {
                            app.input_text.push_str(emoji);
                            app.show_emoji_picker = false;
                        }
                    }
                });

                ui.separator();
                if ui.button("Close").clicked() {
                    app.show_emoji_picker = false;
                }
            });
    }

    // Messages area - fills remaining space
    egui::CentralPanel::default().show_inside(ui, |ui| {
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                if let Ok(manager) = app.chat_manager.try_lock() {
                    if let Some(chat) = manager.get_chat(chat_id) {
                        if chat.messages.is_empty() {
                            ui.vertical_centered(|ui| {
                                ui.add_space(100.0);
                                ui.label(
                                    egui::RichText::new("🔒 End-to-end encrypted conversation")
                                        .size(16.0)
                                        .color(ui.visuals().text_color().gamma_multiply(0.7)),
                                );
                                ui.label(
                                    egui::RichText::new("Send your first message below!")
                                        .size(14.0)
                                        .color(ui.visuals().text_color().gamma_multiply(0.7)),
                                );
                            });
                        } else {
                            for message in &chat.messages {
                                render_message(app, ui, message);
                                ui.add_space(8.0);
                            }
                        }
                    }
                }
            });
    });
}

fn render_message(_app: &App, ui: &mut egui::Ui, message: &Message) {
    let is_me = message.from_me;
    let align = if is_me {
        egui::Layout::right_to_left(egui::Align::TOP)
    } else {
        egui::Layout::left_to_right(egui::Align::TOP)
    };

    ui.with_layout(align, |ui| {
        // Modern Message Bubbles
        // Differentiate colors:
        // - Me: Accent color (Gradient-ish feel via bright accent)
        // - Them: Muted secondary background
        let bg_color = if is_me {
            ui.visuals().widgets.active.bg_fill
        } else {
            ui.visuals().widgets.inactive.bg_fill
        };

        let text_color = if is_me {
            Color32::WHITE // Always white on accent
        } else {
            ui.visuals().text_color()
        };

        // Rounding: bubble effect (sharper corner on the sender side)
        let rounding = if is_me {
            egui::Rounding {
                nw: 18.0,
                ne: 4.0, // Top-right sharp for "me"
                sw: 18.0,
                se: 18.0,
            }
        } else {
            egui::Rounding {
                nw: 4.0, // Top-left sharp for "them"
                ne: 18.0,
                sw: 18.0,
                se: 18.0,
            }
        };

        let frame = egui::Frame::none()
            .fill(bg_color)
            .rounding(rounding)
            .inner_margin(egui::Margin::symmetric(14.0, 10.0)) // More breathing room
            .stroke(egui::Stroke::NONE);

        let frame_response = frame.show(ui, |ui| {
            ui.set_max_width(400.0);

            match &message.content {
                MessageContent::Text { text } => {
                    // Use egui_commonmark for proper Markdown rendering
                    // We create a unique ID for cache based on message ID
                    let mut cache = egui_commonmark::CommonMarkCache::default();
                    ui.visuals_mut().override_text_color = Some(text_color);
                    egui_commonmark::CommonMarkViewer::new().show(ui, &mut cache, text);

                    // Copy action (only visible on hover to reduce clutter)
                    if ui.rect_contains_pointer(ui.max_rect()) {
                        ui.add_space(4.0);
                        if ui.small_button("📋").on_hover_text("Copy").clicked() {
                            ui.output_mut(|o| o.copied_text = text.clone());
                        }
                    }
                }
                MessageContent::File {
                    filename,
                    size,
                    path,
                } => {
                    // Modern File Card
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            // Icon container
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(40.0, 40.0), egui::Sense::hover());
                            ui.painter().rect_filled(
                                rect,
                                egui::Rounding::same(8.0),
                                Color32::from_white_alpha(30),
                            );
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "📄",
                                egui::FontId::proportional(24.0),
                                text_color,
                            );

                            ui.add_space(8.0);

                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new(filename)
                                        .strong()
                                        .size(14.0)
                                        .color(text_color),
                                );
                                ui.label(
                                    egui::RichText::new(crate::util::format_size(*size))
                                        .size(11.0)
                                        .color(text_color.gamma_multiply(0.8)),
                                );
                            });
                        });

                        if let Some(p) = path {
                            ui.add_space(8.0);
                            ui.separator();
                            ui.add_space(4.0);
                            if ui
                                .add(
                                    egui::Button::new("📂 Open")
                                        .fill(Color32::from_white_alpha(20))
                                        .stroke(egui::Stroke::NONE),
                                )
                                .clicked()
                            {
                                let _ = open::that(p);
                            }
                        }
                    });
                }
                MessageContent::Edited { new_text } => {
                    ui.label(
                        egui::RichText::new(format!("{} (Edited)", new_text))
                            .italics()
                            .color(text_color.gamma_multiply(0.8))
                            .size(14.0),
                    );
                }
            }

            ui.add_space(4.0);

            // Timestamp (bottom right of bubble)
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                let timestamp_text =
                    crate::gui::widgets::format_timestamp_relative(&message.timestamp);
                ui.label(
                    egui::RichText::new(timestamp_text)
                        .size(9.0)
                        .color(text_color.gamma_multiply(0.6)),
                );
            });
        });

        // Add subtle hover lift effect
        if frame_response.response.hovered() {
            ui.painter().rect_stroke(
                frame_response.response.rect,
                rounding,
                egui::Stroke::new(1.5, ui.visuals().widgets.active.bg_fill.gamma_multiply(0.5)),
            );
        }
    });
}
