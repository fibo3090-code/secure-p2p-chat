use clap::Parser;

use encodeur_rsa_rust::*;

#[derive(Parser)]
#[command(author, version, about = "P2P Encrypted Messaging Application")]
struct Args {
    /// Start as host (server mode)
    #[arg(short = 'H', long)]
    host: bool,

    /// Connect to host (format: IP:PORT or IP)
    #[arg(short, long)]
    connect: Option<String>,

    /// Port to use (default: 12345)
    #[arg(short, long, default_value_t = PORT_DEFAULT)]
    port: u16,

    /// Enable GUI mode (default)
    #[arg(long, default_value_t = true)]
    gui: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,encodeur_rsa_rust=debug".into()),
        )
        .init();

    let args = Args::parse();

    if args.gui || (!args.host && args.connect.is_none()) {
        // Launch GUI
        tracing::info!("Starting GUI mode");

        let native_options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1200.0, 800.0])
                .with_min_inner_size([800.0, 600.0]),
            ..Default::default()
        };

        let _ = eframe::run_native(
            "Encrypted P2P Messenger",
            native_options,
            Box::new(|cc| Box::new(gui::App::new(cc))),
        );
    } else if args.host {
        // CLI host mode
        tracing::info!("Starting host on port {}", args.port);
        println!("Starting host on port {}...", args.port);
        println!("Waiting for connections...");

        // Keep running
        tokio::signal::ctrl_c().await?;
    } else if let Some(addr) = args.connect {
        // CLI client mode
        let (host, port) = if addr.contains(':') {
            let parts: Vec<&str> = addr.split(':').collect();
            (
                parts[0].to_string(),
                parts[1].parse().unwrap_or(PORT_DEFAULT),
            )
        } else {
            (addr, args.port)
        };

        tracing::info!("Connecting to {}:{}", host, port);
        println!("Connecting to {}:{}...", host, port);

        // Keep running
        tokio::signal::ctrl_c().await?;
    }

    Ok(())
}

mod gui {
    use encodeur_rsa_rust::app::ChatManager;
    use encodeur_rsa_rust::types::*;
    use encodeur_rsa_rust::util::*;
    use encodeur_rsa_rust::PORT_DEFAULT;
    use chrono::Local;
    use eframe::egui;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use uuid::Uuid;

    pub struct App {
        chat_manager: Arc<Mutex<ChatManager>>,
        selected_chat: Option<Uuid>,
        input_text: String,
        show_connect_dialog: bool,
        connect_host: String,
        connect_port: String,
        show_host_dialog: bool,
        host_port: String,
        show_settings: bool,
        show_welcome: bool,
        file_to_send: Option<PathBuf>,
        show_about: bool,
        chat_to_delete: Option<Uuid>,
        history_path: PathBuf,
        show_emoji_picker: bool,
        last_typing_time: Option<std::time::Instant>,
        typing_stopped: bool,
    }

    impl App {
        pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
            let config = Config {
                download_dir: PathBuf::from("Downloads"),
                temp_dir: PathBuf::from("temp"),
                auto_accept_files: false,
                max_file_size: 1024 * 1024 * 1024,
                enable_notifications: true,
                enable_typing_indicators: true,
            };

            let mut chat_manager = ChatManager::new(config);
            
            // Auto-restore conversation history
            let history_path = PathBuf::from("Downloads").join("history.json");
            if history_path.exists() {
                if let Err(e) = chat_manager.load_history(&history_path) {
                    tracing::warn!("Failed to load history: {}", e);
                } else {
                    tracing::info!("Successfully loaded conversation history");
                }
            }

            Self {
                chat_manager: Arc::new(Mutex::new(chat_manager)),
                selected_chat: None,
                input_text: String::new(),
                show_connect_dialog: false,
                connect_host: String::new(),
                connect_port: PORT_DEFAULT.to_string(),
                show_host_dialog: false,
                host_port: PORT_DEFAULT.to_string(),
                show_settings: false,
                show_welcome: true, // Show welcome screen on first launch
                file_to_send: None,
                show_about: false,
                chat_to_delete: None,
                history_path,
                show_emoji_picker: false,
                last_typing_time: None,
                typing_stopped: false,
            }
        }

        fn render_sidebar(&mut self, ui: &mut egui::Ui) {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.heading("💬 Chats");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("➕").on_hover_text("New connection").clicked() {
                        ui.menu_button("▼", |ui| {
                            if ui.button("🎤 Host Connection").clicked() {
                                self.show_host_dialog = true;
                                ui.close_menu();
                            }
                            if ui.button("🔌 Connect to Host").clicked() {
                                self.show_connect_dialog = true;
                                ui.close_menu();
                            }
                        });
                    }
                });
            });
            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                if let Ok(manager) = self.chat_manager.try_lock() {
                    let mut chats: Vec<_> = manager.chats.values().collect();
                    chats.sort_by(|a, b| b.created_at.cmp(&a.created_at));

                    if chats.is_empty() {
                        ui.vertical_centered(|ui| {
                            ui.add_space(50.0);
                            ui.label(
                                egui::RichText::new("No active chats")
                                    .size(14.0)
                                    .color(egui::Color32::GRAY),
                            );
                            ui.label(
                                egui::RichText::new("Click ➕ to start a connection")
                                    .size(12.0)
                                    .color(egui::Color32::DARK_GRAY),
                            );
                        });
                    }

                    for chat in chats {
                        let is_selected = self.selected_chat == Some(chat.id);
                        let chat_id = chat.id;

                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 12.0;

                            // Make the entire row clickable
                            let response = ui.add(
                                egui::Button::new("")
                                    .fill(if is_selected {
                                        egui::Color32::from_rgb(40, 40, 50)
                                    } else {
                                        egui::Color32::TRANSPARENT
                                    })
                                    .min_size(egui::vec2(180.0, 50.0))
                                    .frame(false)
                            );

                            if response.clicked() {
                                self.selected_chat = Some(chat_id);
                            }

                            // Position elements absolutely over the button
                            let rect = response.rect;
                            
                            // Avatar with color based on fingerprint
                            let color = if let Some(fp) = &chat.peer_fingerprint {
                                fingerprint_to_color(fp)
                            } else {
                                egui::Color32::GRAY
                            };

                            let avatar_center = rect.left_center() + egui::vec2(30.0, 0.0);
                            ui.painter().circle_filled(avatar_center, 20.0, color);

                            let initials = get_initials(&chat.title);
                            ui.painter().text(
                                avatar_center,
                                egui::Align2::CENTER_CENTER,
                                initials,
                                egui::FontId::proportional(16.0),
                                egui::Color32::WHITE,
                            );

                            // Chat info
                            let text_start = rect.left_top() + egui::vec2(65.0, 8.0);
                            ui.painter().text(
                                text_start,
                                egui::Align2::LEFT_TOP,
                                &chat.title,
                                egui::FontId::proportional(15.0),
                                egui::Color32::WHITE,
                            );
                            
                            let last_msg_time = chat
                                .messages
                                .last()
                                .map(|m| format_timestamp_relative(&m.timestamp))
                                .unwrap_or_else(|| "No messages".to_string());
                            ui.painter().text(
                                text_start + egui::vec2(0.0, 20.0),
                                egui::Align2::LEFT_TOP,
                                last_msg_time,
                                egui::FontId::proportional(12.0),
                                egui::Color32::GRAY,
                            );

                            // Delete button
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button("🗑").on_hover_text("Delete chat").clicked() {
                                    self.chat_to_delete = Some(chat_id);
                                }
                            });
                        });

                        ui.add_space(4.0);
                        ui.separator();
                    }
                }
            });
        }

        fn render_welcome(&mut self, ctx: &egui::Context) {
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
                    ui.label("📁 Secure file transfer with progress tracking");
                    ui.label("👥 Direct peer-to-peer connections (no server!)");
                    ui.label("🔐 Fingerprint verification for security");
                    ui.label("💾 Message history persistence");

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
                            self.show_welcome = false;
                        }
                        ui.add_space(5.0);
                        if ui.small_button("Show this again later").clicked() {
                            self.show_welcome = false;
                        }
                    });

                    ui.add_space(10.0);
                });
        }

        fn render_chat(&mut self, ui: &mut egui::Ui, chat_id: Uuid) {
            // Handle dropped files
            let dropped_files = ui.input(|i| i.raw.dropped_files.clone());
            if !dropped_files.is_empty() {
                if let Some(file) = dropped_files.first() {
                    if let Some(path) = &file.path {
                        self.file_to_send = Some(path.clone());
                    }
                }
            }

            // Header with connection status
            egui::TopBottomPanel::top("chat_header")
                .exact_height(60.0)
                .show_inside(ui, |ui| {
                    if let Ok(manager) = self.chat_manager.try_lock() {
                        if let Some(chat) = manager.get_chat(chat_id) {
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                // Avatar
                                let color = if let Some(fp) = &chat.peer_fingerprint {
                                    fingerprint_to_color(fp)
                                } else {
                                    egui::Color32::GRAY
                                };

                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(40.0, 40.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().circle_filled(rect.center(), 20.0, color);

                                let initials = get_initials(&chat.title);
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
                                                .color(egui::Color32::LIGHT_BLUE),
                                        );
                                    } else {
                                        ui.label(
                                            egui::RichText::new("🟢 Connected")
                                                .size(12.0)
                                                .color(egui::Color32::from_rgb(0, 200, 0)),
                                        );
                                    }
                                });

                                // Fingerprint on right
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if let Some(fp) = &chat.peer_fingerprint {
                                        if ui.button("📋 Copy Fingerprint").clicked() {
                                            ui.output_mut(|o| o.copied_text = fp.clone());
                                        }
                                        ui.monospace(format_fingerprint_short(fp));
                                    }
                                });
                            });
                        }
                    }
                });

            // Input area - FIXED AT BOTTOM
            egui::TopBottomPanel::bottom("chat_input")
                .exact_height(120.0)
                .show_inside(ui, |ui| {
                    ui.add_space(5.0);

                    // File preview if selected
                    if self.file_to_send.is_some() {
                        let file_path = self.file_to_send.clone().unwrap();
                        let filename = file_path
                            .file_name()
                            .unwrap()
                            .to_string_lossy()
                            .to_string();

                        ui.horizontal(|ui| {
                            ui.label("📎 File to send:");
                            ui.label(
                                egui::RichText::new(&filename)
                                    .strong()
                                    .color(egui::Color32::from_rgb(100, 150, 255)),
                            );
                            if ui.small_button("❌ Cancel").clicked() {
                                self.file_to_send = None;
                            }
                            if ui.button("✅ Send File").clicked() {
                                // Implement file sending
                                if let Some(path) = self.file_to_send.take() {
                                    let manager = self.chat_manager.clone();
                                    tokio::spawn(async move {
                                        let mut mgr = manager.lock().await;
                                        if let Err(e) = mgr.send_file(chat_id, path).await {
                                            mgr.add_toast(ToastLevel::Error, format!("Failed to send file: {}", e));
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
                                self.file_to_send = Some(path);
                            }
                        }

                        // Emoji picker button
                        if ui
                            .button(egui::RichText::new("😊").size(20.0))
                            .on_hover_text("Emoji picker")
                            .clicked()
                        {
                            self.show_emoji_picker = !self.show_emoji_picker;
                        }

                        // Multiline text input
                        let text_width = ui.available_width() - 70.0;
                        let response = ui.add_sized(
                            [text_width, 70.0],
                            egui::TextEdit::multiline(&mut self.input_text)
                                .hint_text("💬 Type a message... (Ctrl+Enter to send)")
                                .desired_rows(3)
                                .lock_focus(false),
                        );

                        // Handle typing indicators
                        if response.changed() && !self.input_text.is_empty() {
                            let now = std::time::Instant::now();
                            let should_send_typing = self.last_typing_time
                                .map_or(true, |last| now.duration_since(last).as_secs() >= 2);
                            
                            if should_send_typing {
                                let manager = self.chat_manager.clone();
                                tokio::spawn(async move {
                                    let mgr = manager.lock().await;
                                    let _ = mgr.send_typing_start(chat_id);
                                });
                                self.last_typing_time = Some(now);
                                self.typing_stopped = false;
                            }
                        }

                        // Stop typing when text is cleared or after timeout
                        if self.input_text.is_empty() && !self.typing_stopped {
                            let manager = self.chat_manager.clone();
                            tokio::spawn(async move {
                                let mgr = manager.lock().await;
                                let _ = mgr.send_typing_stop(chat_id);
                            });
                            self.typing_stopped = true;
                        }

                        // Handle keyboard shortcuts
                        if response.has_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.ctrl)
                        {
                            self.send_message_clicked(chat_id);
                            // Stop typing on send
                            let manager = self.chat_manager.clone();
                            tokio::spawn(async move {
                                let mgr = manager.lock().await;
                                let _ = mgr.send_typing_stop(chat_id);
                            });
                            self.typing_stopped = true;
                        }

                        // Send button
                        let send_enabled = !self.input_text.trim().is_empty();
                        let mut send_button = egui::Button::new(
                            egui::RichText::new("📤\nSend").size(14.0).strong()
                        )
                        .min_size(egui::vec2(65.0, 70.0));

                        if send_enabled {
                            send_button = send_button.fill(egui::Color32::from_rgb(0, 120, 255));
                        }

                        if ui.add_enabled(send_enabled, send_button).clicked() {
                            self.send_message_clicked(chat_id);
                        }
                    });
                });

            // Emoji picker overlay
            if self.show_emoji_picker {
                egui::Window::new("😊 Emoji Picker")
                    .resizable(false)
                    .collapsible(false)
                    .default_width(300.0)
                    .show(ui.ctx(), |ui| {
                        ui.horizontal_wrapped(|ui| {
                            let common_emojis = [
                                "😊", "😂", "❤️", "👍", "👎", "🎉", "🔥", "💯",
                                "😍", "😎", "😢", "😭", "😡", "🤔", "👋", "🙏",
                                "✨", "⭐", "💪", "👏", "🎊", "🎈", "🚀", "💡",
                                "📱", "💻", "📷", "🎵", "🎮", "⚽", "🍕", "🍰",
                            ];
                            
                            for emoji in &common_emojis {
                                if ui.button(egui::RichText::new(*emoji).size(24.0)).clicked() {
                                    self.input_text.push_str(emoji);
                                    self.show_emoji_picker = false;
                                }
                            }
                        });
                        
                        ui.separator();
                        if ui.button("Close").clicked() {
                            self.show_emoji_picker = false;
                        }
                    });
            }

            // Messages area - fills remaining space
            egui::CentralPanel::default().show_inside(ui, |ui| {
                if let Ok(manager) = self.chat_manager.try_lock() {
                    if let Some(chat) = manager.get_chat(chat_id) {
                        if chat.messages.is_empty() {
                            ui.vertical_centered(|ui| {
                                ui.add_space(100.0);
                                ui.label(
                                    egui::RichText::new("🔒 End-to-end encrypted conversation")
                                        .size(16.0)
                                        .color(egui::Color32::GRAY),
                                );
                                ui.label(
                                    egui::RichText::new("Send your first message below!")
                                        .size(14.0)
                                        .color(egui::Color32::DARK_GRAY),
                                );
                            });
                        } else {
                            for message in &chat.messages {
                                self.render_message(ui, message);
                                ui.add_space(4.0);
                            }
                        }
                    }
                }
            });
        }

        fn render_message(&self, ui: &mut egui::Ui, message: &Message) {
            let align = if message.from_me {
                egui::Layout::right_to_left(egui::Align::TOP)
            } else {
                egui::Layout::left_to_right(egui::Align::TOP)
            };

            ui.with_layout(align, |ui| {
                // Message bubble with custom styling
                let bg_color = if message.from_me {
                    egui::Color32::from_rgb(0, 120, 255) // Blue for sent
                } else {
                    egui::Color32::from_rgb(60, 60, 70) // Dark gray for received
                };

                let frame = egui::Frame::none()
                    .fill(bg_color)
                    .rounding(egui::Rounding::same(12.0))
                    .inner_margin(egui::Margin::symmetric(12.0, 8.0))
                    .stroke(egui::Stroke::NONE);

                let frame_response = frame.show(ui, |ui| {
                    ui.set_max_width(400.0);

                    match &message.content {
                        MessageContent::Text { text } => {
                            // Text message with white color
                            ui.label(
                                egui::RichText::new(text)
                                    .color(egui::Color32::WHITE)
                                    .size(14.0),
                            );

                            // Small copy button
                            ui.add_space(2.0);
                            if ui
                                .small_button(
                                    egui::RichText::new("📋 Copy")
                                        .size(10.0)
                                        .color(egui::Color32::LIGHT_GRAY),
                                )
                                .clicked()
                            {
                                ui.output_mut(|o| o.copied_text = text.clone());
                            }
                        }
                        MessageContent::File {
                            filename,
                            size,
                            path,
                        } => {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("📄")
                                        .size(24.0)
                                        .color(egui::Color32::WHITE),
                                );
                                ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new(filename)
                                            .strong()
                                            .color(egui::Color32::WHITE),
                                    );
                                    ui.label(
                                        egui::RichText::new(format_size(*size))
                                            .small()
                                            .color(egui::Color32::LIGHT_GRAY),
                                    );
                                });
                            });

                            if let Some(p) = path {
                                ui.add_space(4.0);
                                if ui
                                    .button(
                                        egui::RichText::new("📂 Open File")
                                            .color(egui::Color32::WHITE),
                                    )
                                    .clicked()
                                {
                                    let _ = open::that(p);
                                }
                            }
                        }
                    }

                    ui.add_space(2.0);

                    // Timestamp with subtle styling
                    let timestamp_text = format_timestamp_relative(&message.timestamp);
                    ui.label(
                        egui::RichText::new(timestamp_text)
                            .size(10.0)
                            .color(egui::Color32::from_rgb(200, 200, 220)),
                    );
                });

                // Add hover effect
                if frame_response.response.hovered() {
                    ui.painter().rect_stroke(
                        frame_response.response.rect,
                        12.0,
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(150, 150, 150)),
                    );
                }
            });
        }

        fn render_toasts(&self, ctx: &egui::Context) {
            let toasts = if let Ok(manager) = self.chat_manager.try_lock() {
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
                                    ToastLevel::Info => egui::Color32::LIGHT_BLUE,
                                    ToastLevel::Success => egui::Color32::LIGHT_GREEN,
                                    ToastLevel::Warning => egui::Color32::YELLOW,
                                    ToastLevel::Error => egui::Color32::LIGHT_RED,
                                };

                                ui.colored_label(color, &toast.message);
                            });

                            ui.add_space(4.0);
                        }
                    }
                });
        }

        fn send_message_clicked(&mut self, chat_id: Uuid) {
            if self.input_text.trim().is_empty() {
                return;
            }

            let text = std::mem::take(&mut self.input_text);

            if let Ok(mut manager) = self.chat_manager.try_lock() {
                if let Err(e) = manager.send_message(chat_id, text) {
                    manager.add_toast(ToastLevel::Error, format!("Failed to send: {}", e));
                }
            }
        }

        fn start_host_clicked(&mut self) {
            let port = self.host_port.parse().unwrap_or(PORT_DEFAULT);
            let manager = self.chat_manager.clone();

            tokio::spawn(async move {
                let mut mgr = manager.lock().await;
                if let Err(e) = mgr.start_host(port).await {
                    mgr.add_toast(ToastLevel::Error, format!("Failed to start host: {}", e));
                }
            });
        }

        fn connect_clicked(&mut self) {
            let host = self.connect_host.clone();
            let port = self.connect_port.parse().unwrap_or(PORT_DEFAULT);
            let manager = self.chat_manager.clone();

            tokio::spawn(async move {
                let mut mgr = manager.lock().await;
                if let Err(e) = mgr.connect_to_host(&host, port).await {
                    mgr.add_toast(ToastLevel::Error, format!("Failed to connect: {}", e));
                }
            });
        }
    }

    impl eframe::App for App {
        fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
            // Poll session events to process received messages
            if let Ok(mut manager) = self.chat_manager.try_lock() {
                manager.poll_session_events();
                manager.cleanup_expired_toasts();
                
                // Auto-save history periodically
                static mut LAST_SAVE: Option<std::time::Instant> = None;
                unsafe {
                    let now = std::time::Instant::now();
                    let should_save = LAST_SAVE.map_or(true, |last| {
                        now.duration_since(last).as_secs() > 30
                    });
                    
                    if should_save && !manager.chats.is_empty() {
                        if let Err(e) = manager.save_history(&self.history_path) {
                            tracing::warn!("Failed to auto-save history: {}", e);
                        }
                        LAST_SAVE = Some(now);
                    }
                }
            }

            // Top panel - Menu bar
            egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
                egui::menu::bar(ui, |ui| {
                    ui.menu_button("🔌 Connection", |ui| {
                        if ui.button("🎤 Start Host").clicked() {
                            self.show_host_dialog = true;
                            ui.close_menu();
                        }
                        if ui.button("🔌 Connect to Host").clicked() {
                            self.show_connect_dialog = true;
                            ui.close_menu();
                        }
                    });

                    ui.menu_button("⚙️ Settings", |ui| {
                        if ui.button("⚙️ Preferences").clicked() {
                            self.show_settings = true;
                            ui.close_menu();
                        }
                    });

                    ui.menu_button("ℹ️ Help", |ui| {
                        if ui.button("👋 Show Welcome Screen").clicked() {
                            self.show_welcome = true;
                            ui.close_menu();
                        }
                        if ui.button("ℹ️ About").clicked() {
                            self.show_about = true;
                            ui.close_menu();
                        }
                    });
                });
            });

            // Sidebar - Chat list
            egui::SidePanel::left("sidebar")
                .default_width(250.0)
                .show(ctx, |ui| {
                    self.render_sidebar(ui);
                });

            // Main panel - Messages
            egui::CentralPanel::default().show(ctx, |ui| {
                if let Some(chat_id) = self.selected_chat {
                    self.render_chat(ui, chat_id);
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.label("Select a chat or start a new connection");
                    });
                }
            });

            // Toasts overlay
            self.render_toasts(ctx);

            // Delete confirmation dialog
            if let Some(chat_id) = self.chat_to_delete {
                egui::Window::new("⚠️ Delete Chat")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .show(ctx, |ui| {
                        ui.label("Are you sure you want to delete this chat?");
                        ui.label("This action cannot be undone.");
                        ui.add_space(10.0);
                        
                        ui.horizontal(|ui| {
                            if ui.button("❌ Delete").clicked() {
                                if let Ok(mut manager) = self.chat_manager.try_lock() {
                                    manager.delete_chat(chat_id);
                                    if self.selected_chat == Some(chat_id) {
                                        self.selected_chat = None;
                                    }
                                    // Auto-save after deletion
                                    let _ = manager.save_history(&self.history_path);
                                }
                                self.chat_to_delete = None;
                            }
                            if ui.button("Cancel").clicked() {
                                self.chat_to_delete = None;
                            }
                        });
                    });
            }

            // Dialogs
            if self.show_host_dialog {
                egui::Window::new("Start Host")
                    .collapsible(false)
                    .resizable(false)
                    .show(ctx, |ui| {
                        ui.label("Port:");
                        ui.text_edit_singleline(&mut self.host_port);

                        ui.horizontal(|ui| {
                            if ui.button("Start").clicked() {
                                self.start_host_clicked();
                                self.show_host_dialog = false;
                            }

                            if ui.button("Cancel").clicked() {
                                self.show_host_dialog = false;
                            }
                        });
                    });
            }

            if self.show_connect_dialog {
                egui::Window::new("Connect to Host")
                    .collapsible(false)
                    .resizable(false)
                    .show(ctx, |ui| {
                        ui.label("Host:");
                        ui.text_edit_singleline(&mut self.connect_host);

                        ui.label("Port:");
                        ui.text_edit_singleline(&mut self.connect_port);

                        ui.horizontal(|ui| {
                            if ui.button("Connect").clicked() {
                                self.connect_clicked();
                                self.show_connect_dialog = false;
                            }

                            if ui.button("Cancel").clicked() {
                                self.show_connect_dialog = false;
                            }
                        });
                    });
            }

            // Welcome screen
            if self.show_welcome {
                self.render_welcome(ctx);
            }

            // Settings dialog
            if self.show_settings {
                egui::Window::new("⚙️ Settings")
                    .collapsible(false)
                    .resizable(false)
                    .show(ctx, |ui| {
                        ui.heading("Application Settings");
                        ui.separator();

                        if let Ok(mut manager) = self.chat_manager.try_lock() {
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
                            let mut max_size_mb =
                                (manager.config.max_file_size / (1024 * 1024)) as u32;
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
                            if ui.button("Close").clicked() {
                                self.show_settings = false;
                            }
                        });
                    });
            }

            // About dialog
            if self.show_about {
                egui::Window::new("ℹ️ About")
                    .collapsible(false)
                    .resizable(false)
                    .show(ctx, |ui| {
                        ui.vertical_centered(|ui| {
                            ui.heading("Encrypted P2P Messenger");
                            ui.label("Version 1.0.0");
                            ui.add_space(10.0);
                        });

                        ui.separator();
                        ui.add_space(10.0);

                        ui.label("A secure, peer-to-peer messaging application");
                        ui.label("with end-to-end encryption.");
                        ui.add_space(10.0);

                        ui.label("🔒 Encryption: RSA-2048-OAEP + AES-256-GCM");
                        ui.label("🔐 Security: Fingerprint verification");
                        ui.label("📁 Features: File transfer, message history");
                        ui.add_space(10.0);

                        ui.separator();
                        ui.add_space(10.0);

                        ui.vertical_centered(|ui| {
                            if ui.button("Close").clicked() {
                                self.show_about = false;
                            }
                        });
                    });
            }

            // Request repaint for animations
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    // Utility functions
    fn fingerprint_to_color(fingerprint: &str) -> egui::Color32 {
        let hash = fingerprint
            .bytes()
            .take(3)
            .fold(0u32, |acc, b| acc.wrapping_mul(256).wrapping_add(b as u32));
        let r = ((hash >> 16) & 0xFF) as u8;
        let g = ((hash >> 8) & 0xFF) as u8;
        let b = (hash & 0xFF) as u8;
        // Ensure colors are vibrant enough
        let r = r.max(80);
        let g = g.max(80);
        let b = b.max(80);
        egui::Color32::from_rgb(r, g, b)
    }

    fn get_initials(name: &str) -> String {
        name.split_whitespace()
            .take(2)
            .filter_map(|word| word.chars().next())
            .collect::<String>()
            .to_uppercase()
    }

    fn format_timestamp_relative(dt: &chrono::DateTime<chrono::Utc>) -> String {
        let local: chrono::DateTime<Local> = dt.with_timezone(&Local);
        let now = Local::now();

        let days_diff = (now.date_naive() - local.date_naive()).num_days();

        match days_diff {
            0 => local.format("%H:%M").to_string(),
            1 => format!("Yesterday {}", local.format("%H:%M")),
            2..=6 => local.format("%A %H:%M").to_string(),
            _ => local.format("%Y-%m-%d %H:%M").to_string(),
        }
    }
}
