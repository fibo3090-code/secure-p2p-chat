use crate::app::ChatManager;
use crate::types::*;

use crate::PORT_DEFAULT;

use eframe::egui;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;
use egui_tracing::tracing::EventCollector;

pub struct App {
    pub chat_manager: Arc<Mutex<ChatManager>>,
    pub identity: crate::identity::Identity,
    pub selected_chat: Option<Uuid>,
    pub input_text: String,
    // Contacts / groups UI state
    pub show_contacts: bool,
    pub show_add_contact: bool,
    pub contact_tab: usize, // 0=Manual, 1=Invite Link, 2=Generate My Link
    pub new_contact_name: String,
    pub new_contact_address: String,
    pub new_contact_fingerprint: String,
    pub new_contact_pubkey: String,
    pub invite_link_input: String,
    pub my_invite_link: Option<String>,
    pub show_create_group: bool,
    pub group_wizard_step: usize, // 0=Name, 1=Members, 2=Confirm
    pub group_selected: Vec<Uuid>,
    pub group_title: String,
    pub group_search: String,
    // Rename conversation dialog
    pub show_rename_dialog: bool,
    pub rename_chat_id: Option<Uuid>,
    pub rename_input: String,
    pub show_connect_dialog: bool,
    pub connect_host: String,
    pub connect_port: String,
    pub show_host_dialog: bool,
    pub host_port: String,
    pub show_settings: bool,
    pub show_welcome: bool,
    pub file_to_send: Option<PathBuf>,
    pub show_about: bool,
    pub chat_to_delete: Option<Uuid>,
    pub history_path: PathBuf,
    pub show_emoji_picker: bool,
    pub last_typing_time: Option<std::time::Instant>,
    pub typing_stopped: bool,
    // Fingerprint verification dialog
    pub show_fingerprint_dialog: bool,
    pub fingerprint_to_verify: Option<String>,
    pub peer_name_to_verify: Option<String>,
    pub chat_id_to_verify: Option<Uuid>,
    pub show_log_terminal: bool,
    pub show_clear_history_dialog: bool,
    pub event_collector: EventCollector,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, event_collector: EventCollector) -> Self {
        cc.egui_ctx
            .set_visuals(crate::gui::styling::apply_custom_visuals());

        // Load fonts. Embedding the TTF files at compile time requires the files to exist.
        // Make embedding optional so builds don't fail when the `assets/` files are not present.
    #[cfg(feature = "embed_fonts")]
    let mut fonts = egui::FontDefinitions::default();
    #[cfg(not(feature = "embed_fonts"))]
    let fonts = egui::FontDefinitions::default();

        // If you want to embed the Inter fonts into the binary, enable the
        // `embed_fonts` feature in Cargo.toml and ensure the files exist at
        // `assets/Inter-Regular.ttf` and `assets/Inter-Bold.ttf`.
        #[cfg(feature = "embed_fonts")]
        {
            fonts.font_data.insert(
                "Inter-Regular".to_owned(),
                egui::FontData::from_static(include_bytes!("../../assets/Inter-Regular.ttf")),
            );
            fonts.font_data.insert(
                "Inter-Bold".to_owned(),
                egui::FontData::from_static(include_bytes!("../../assets/Inter-Bold.ttf")),
            );

            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "Inter-Regular".to_owned());

            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .insert(0, "Inter-Regular".to_owned());
        }

        cc.egui_ctx.set_fonts(fonts);

        let config = Config::default();

        let mut chat_manager = ChatManager::new(config);
        let initial_show_log_terminal = chat_manager.config.show_log_terminal;

        // Auto-restore conversation history from platform-specific user data directory
        // Windows: %APPDATA%\chat-p2p\history.json
        // Linux: ~/.local/share/chat-p2p/history.json
        // macOS: ~/Library/Application Support/chat-p2p/history.json
        let (history_path, identity) = if let Some(proj_dirs) =
            directories::ProjectDirs::from("com", "chat-p2p", "EncryptedMessenger")
        {
            let data_dir = proj_dirs.data_dir();
            std::fs::create_dir_all(data_dir).ok(); // Ensure directory exists

            // Load or create user identity
            let identity = crate::identity::Identity::get_or_create(data_dir, "User")
                .unwrap_or_else(|e| {
                    tracing::error!("Failed to load/create identity: {}", e);
                    crate::identity::Identity::new("User".to_string())
                        .expect("Failed to create identity")
                });

            (data_dir.join("history.json"), identity)
        } else {
            // Fallback to relative path if directories crate fails
            tracing::warn!("Could not determine user data directory, using fallback path");
            let identity = crate::identity::Identity::new("User".to_string())
                .expect("Failed to create identity");
            (PathBuf::from("Downloads").join("history.json"), identity)
        };

        tracing::info!("Using history path: {}", history_path.display());
        tracing::info!(
            "Using identity: {} (fingerprint: {}...)",
            identity.name,
            &identity.fingerprint[..16]
        );

        if history_path.exists() {
            if let Err(e) = chat_manager.load_history(&history_path) {
                tracing::warn!("Failed to load history: {}", e);
            } else {
                tracing::info!("Successfully loaded conversation history");
            }
        }

        // Capture config before moving manager
        let auto_host_enabled = chat_manager.config.auto_host_on_startup;
        let auto_host_port = chat_manager.config.listen_port;
        // Capture listen_port for initializing the UI field before moving manager
        let host_port_ui = auto_host_port.to_string();
        // Wrap manager in Arc<Mutex<..>> once and reuse
        let manager_arc = Arc::new(Mutex::new(chat_manager));
        // Auto-start host on startup if enabled in settings
        if auto_host_enabled {
            tracing::info!(port = %auto_host_port, "Auto-host on startup is enabled; starting host");
            let mgr_clone = manager_arc.clone();
            tokio::spawn(async move {
                let mut mgr = mgr_clone.lock().await;
                if let Err(e) = mgr.start_host(auto_host_port).await {
                    mgr.add_toast(
                        crate::types::ToastLevel::Error,
                        format!("Failed to auto-start host: {}", e),
                    );
                }
            });
        }

        Self {
            chat_manager: manager_arc,
            identity,
            selected_chat: None,
            input_text: String::new(),
            show_connect_dialog: false,
            connect_host: String::new(),
            connect_port: PORT_DEFAULT.to_string(),
            show_host_dialog: false,
            host_port: host_port_ui,
            show_settings: false,
            show_welcome: true, // Show welcome screen on first launch
            file_to_send: None,
            show_about: false,
            chat_to_delete: None,
            show_contacts: false,
            show_add_contact: false,
            contact_tab: 0,
            new_contact_name: String::new(),
            new_contact_address: String::new(),
            new_contact_fingerprint: String::new(),
            new_contact_pubkey: String::new(),
            invite_link_input: String::new(),
            my_invite_link: None,
            show_create_group: false,
            group_wizard_step: 0,
            group_selected: Vec::new(),
            group_title: String::new(),
            group_search: String::new(),
            show_rename_dialog: false,
            rename_chat_id: None,
            rename_input: String::new(),
            history_path,
            show_emoji_picker: false,
            last_typing_time: None,
            typing_stopped: false,
            // Fingerprint verification dialog
            show_fingerprint_dialog: false,
            fingerprint_to_verify: None,
            peer_name_to_verify: None,
            chat_id_to_verify: None,
            show_log_terminal: initial_show_log_terminal,
            show_clear_history_dialog: false,
            event_collector,
        }
    }

    pub fn send_message_clicked(&mut self, chat_id: Uuid) {
        if self.input_text.trim().is_empty() {
            return;
        }

        let text = std::mem::take(&mut self.input_text);

        if let Ok(mut manager) = self.chat_manager.try_lock() {
            if let Err(e) = manager.send_message(chat_id, text) {
                manager.add_toast(
                    crate::types::ToastLevel::Error,
                    format!("Failed to send: {}", e),
                );
            }
        }
    }

    pub fn start_host_clicked(&mut self) {
        let port = self.host_port.parse().unwrap_or(crate::PORT_DEFAULT);
        let manager = self.chat_manager.clone();

        tokio::spawn(async move {
            let mut mgr = manager.lock().await;
            if let Err(e) = mgr.start_host(port).await {
                mgr.add_toast(
                    crate::types::ToastLevel::Error,
                    format!("Failed to start host: {}", e),
                );
            }
        });
    }

    pub fn connect_clicked(&mut self) {
      let mut host = self.connect_host.clone();
      let mut port = self.connect_port.parse().unwrap_or(crate::PORT_DEFAULT);
      if let Some(colon) = host.find(':') {
          let (h, p) = host.split_at(colon);
          // Clone slices to owned Strings to avoid borrowing `host` while reassigning it
          let h_str = h.to_string();
          let p_str = p[1..].to_string(); // skip ':'
          if let Ok(pn) = p_str.parse::<u16>() { port = pn; }
          host = h_str;
      }
      let manager = self.chat_manager.clone();
      let existing_chat = self.selected_chat; // bind connection to the currently selected chat if any

      tokio::spawn(async move {
          let mut mgr = manager.lock().await;
          if let Err(e) = mgr.connect_to_host(&host, port, existing_chat).await {
              mgr.add_toast(
                  crate::types::ToastLevel::Error,
                  format!("Failed to connect: {}", e),
              );
          }
      });
  }

}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll session events to process received messages
        if let Ok(mut manager) = self.chat_manager.try_lock() {
            manager.poll_session_events();
            if let Some((fingerprint, peer_name, chat_id)) = manager.fingerprint_verification_request.take() {
                self.fingerprint_to_verify = Some(fingerprint);
                self.peer_name_to_verify = Some(peer_name);
                self.chat_id_to_verify = Some(chat_id);
                self.show_fingerprint_dialog = true;
            }
            manager.cleanup_expired_toasts();

            // Auto-save history periodically
            static mut LAST_SAVE: Option<std::time::Instant> = None;
            unsafe {
                let now = std::time::Instant::now();
                let should_save =
                    LAST_SAVE.is_none_or(|last| now.duration_since(last).as_secs() > 30);

                if should_save && !manager.chats.is_empty() {
                    if let Err(e) = manager.save_history(&self.history_path) {
                        tracing::warn!("Failed to auto-save history: {}", e);
                    }
                    LAST_SAVE = Some(now);
                }
            }

            // Auto-rehost: if auto-host is enabled and no placeholder host chat exists,
            // spawn a new host to replace the one that was consumed by a connection.
            if manager.config.auto_host_on_startup {
                let has_placeholder = manager
                    .chats
                    .values()
                    .any(|c| c.title.starts_with("Host on :"));
                if !has_placeholder {
                    static mut LAST_REHOST: Option<std::time::Instant> = None;
                    unsafe {
                        let now = std::time::Instant::now();
                        let should_rehost = LAST_REHOST
                            .is_none_or(|last| now.duration_since(last).as_millis() > 1500);
                        if should_rehost {
                            let port = manager.config.listen_port;
                            let mgr_arc = self.chat_manager.clone();
                            tokio::spawn(async move {
                                let mut mgr = mgr_arc.lock().await;
                                if let Err(e) = mgr.start_host(port).await {
                                    mgr.add_toast(
                                        crate::types::ToastLevel::Error,
                                        format!("Failed to re-start host: {}", e),
                                    );
                                } else {
                                    mgr.add_toast(
                                        crate::types::ToastLevel::Success,
                                        "Host relancé".to_string(),
                                    );
                                }
                            });
                            LAST_REHOST = Some(now);
                        }
                    }
                }
            }
        }

        // Top panel - Menu bar
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Connection menu
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

                if ui.button("Contacts").clicked() {
                    self.show_contacts = true;
                }

                if ui.button("Settings").clicked() {
                    self.show_settings = true;
                }

                if ui.button("Help").clicked() {
                    self.show_welcome = true;
                }
            });
        });

        // Sidebar - Chat list
        egui::SidePanel::left("sidebar")
            .default_width(250.0)
            .show(ctx, |ui| {
                crate::gui::sidebar::render_sidebar(self, ui);
            });

        // Main panel - Messages
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(chat_id) = self.selected_chat {
                crate::gui::chat_view::render_chat(self, ui, chat_id);
            } else {
                ui.centered_and_justified(|ui| {
                    ui.label("Select a chat or start a new connection");
                });
            }
        });

        // Toasts overlay
        crate::gui::dialogs::render_toasts(self, ctx);

        // Dialogs
        crate::gui::dialogs::render_dialogs(self, ctx);

        // Request repaint for animations
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}
