use crate::gui::app_ui::App;
use crate::types::{Message, MessageContent};
use eframe::egui::{self, Color32};
use uuid::Uuid;

pub fn render_chat(app: &mut App, ui: &mut egui::Ui, chat_id: Uuid) {
    // Handle dropped files
    let dropped_files = ui.input(|i| i.raw.dropped_files.clone());
    if !dropped_files.is_empty()
        && let Some(file) = dropped_files.first()
        && let Some(path) = &file.path {
            app.file_to_send = Some(path.clone());
    }

    // Header with connection status
    egui::TopBottomPanel::top("chat_header")
        .exact_height(60.0)
        .show_inside(ui, |ui| {
            if let Ok(manager) = app.chat_manager.try_lock()
                && let Some(chat) = manager.get_chat(chat_id) {
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
                            } else {
                                ui.label(
                                    egui::RichText::new("🟢 Connected")
                                        .size(12.0)
                                        .color(crate::gui::styling::SUCCESS),
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
                        });
                    });
            }
        });

    // Input area - FIXED AT BOTTOM
    egui::TopBottomPanel::bottom("chat_input")
        .exact_height(120.0)
        .show_inside(ui, |ui| {
            ui.add_space(5.0);

            // File preview if selected
            if app.file_to_send.is_some() {
                let file_path = app.file_to_send.clone().unwrap();
                let filename = file_path.file_name().unwrap().to_string_lossy().to_string();

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
                    && let Some(path) = rfd::FileDialog::new().pick_file() {
                    app.file_to_send = Some(path);
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
                let text_width = ui.available_width() - 70.0;
                let response = ui.add_sized(
                    [text_width, 70.0],
                    egui::TextEdit::multiline(&mut app.input_text)
                        .hint_text("💬 Type a message... (Ctrl+Enter to send)")
                        .desired_rows(3)
                        .lock_focus(false),
                );

                // Handle typing indicators
                if response.changed() && !app.input_text.is_empty() {
                    let now = std::time::Instant::now();
                    let should_send_typing = app
                        .last_typing_time
                        .is_none_or(|last| now.duration_since(last).as_secs() >= 2);

                    if should_send_typing {
                        let manager = app.chat_manager.clone();
                        tokio::spawn(async move {
                            let mgr = manager.lock().await;
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
                        let mgr = manager.lock().await;
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
                        let mgr = manager.lock().await;
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
                if let Ok(manager) = app.chat_manager.try_lock()
                    && let Some(chat) = manager.get_chat(chat_id) {
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
                    // Simple Markdown Parsing (Bold/Italic)
                    // We split by whitespace and look for * or _ wrappers manually for now
                    // keeping it simple to avoid deps. Or just use RichText which supports some stats?
                    // egui::RichText doesn't auto-parse markdown.
                    // For "way better", let's use a nice font size.
                    
                    let mut job = egui::text::LayoutJob::default();
                    // Basic "Markdown" simulation for Bold and Code
                    // This is naive but works for simple cases without a parser crate
                    let mut chars = text.chars().peekable();
                    let mut current_text = String::new();
                    let format = egui::TextFormat {
                        font_id: egui::FontId::proportional(15.0), // Slightly larger font
                        color: text_color,
                        ..Default::default()
                    };

                    while let Some(c) = chars.next() {
                        if c == '`' {
                            // Flush current
                            if !current_text.is_empty() {
                                job.append(&current_text, 0.0, format.clone());
                                current_text.clear();
                            }
                            // Read code block
                            let mut code_text = String::new();
                            while let Some(code_c) = chars.next() {
                                if code_c == '`' { break; }
                                code_text.push(code_c);
                            }
                            // Append code styled
                            let code_format = egui::TextFormat {
                                font_id: egui::FontId::monospace(13.0),
                                background: Color32::BLACK.gamma_multiply(0.2), // Dim background
                                color: if is_me { Color32::WHITE } else { crate::gui::styling::ACCENT_SECONDARY },
                                ..format.clone()
                            };
                            job.append(&code_text, 0.0, code_format);
                        } else {
                            current_text.push(c);
                        }
                    }
                    if !current_text.is_empty() {
                        job.append(&current_text, 0.0, format);
                    }
                    
                    ui.label(job);

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
                            let (rect, _) = ui.allocate_exact_size(egui::vec2(40.0, 40.0), egui::Sense::hover());
                            ui.painter().rect_filled(
                                rect,
                                egui::Rounding::same(8.0),
                                Color32::from_white_alpha(30)
                            );
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "📄",
                                egui::FontId::proportional(24.0),
                                text_color
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
                            if ui.add(
                                egui::Button::new("📂 Open")
                                    .fill(Color32::from_white_alpha(20))
                                    .stroke(egui::Stroke::NONE)
                            ).clicked() {
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
            ui.with_layout(egui::Layout::right_to_left(egui::Align::BOTTOM), |ui| {
                let timestamp_text = crate::gui::widgets::format_timestamp_relative(&message.timestamp);
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
