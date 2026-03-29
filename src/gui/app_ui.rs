use crate::app::ChatManager;
use crate::network::{DiscoveredPeer, Discovery};
use crate::types::*;

use crate::PORT_DEFAULT;

use directories::UserDirs;
use eframe::egui;
use egui_tracing::tracing::EventCollector;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum ActiveDialog {
    None,
    Contacts,
    AddContact,
    CreateGroup,
    RenameChat,
    Connect,
    Host,
    Settings,
    About,
    Password,
    SetPassword,
    FingerprintVerification,
    ClearHistory,
    Welcome,
    DeleteChat,
}

pub struct App {
    pub chat_manager: Arc<Mutex<ChatManager>>,
    pub identity: crate::identity::Identity,
    pub selected_chat: Option<Uuid>,
    pub input_text: String,
    pub active_dialog: ActiveDialog,
    // Contacts / groups UI state
    // pub show_contacts: bool, REMOVED
    // pub show_add_contact: bool, REMOVED
    pub contact_tab: usize, // 0=Manual, 1=Invite Link, 2=Generate My Link
    pub new_contact_name: String,
    pub new_contact_address: String,
    pub new_contact_fingerprint: String,
    pub new_contact_pubkey: String,
    // Help tab state
    pub help_tab: usize,
    pub invite_link_input: String,
    pub my_invite_link: Option<String>,
    // pub show_create_group: bool, REMOVED
    pub group_wizard_step: usize, // 0=Name, 1=Members, 2=Confirm
    pub group_selected: Vec<Uuid>,
    pub group_title: String,
    pub group_search: String,
    // Rename conversation dialog
    // pub show_rename_dialog: bool, REMOVED
    pub rename_chat_id: Option<Uuid>,
    pub rename_input: String,
    // pub show_connect_dialog: bool, REMOVED
    pub connect_host: String,
    pub connect_port: String,
    // pub show_host_dialog: bool, REMOVED
    pub host_port: String,
    // pub show_settings: bool, REMOVED
    // pub show_welcome: bool, REMOVED
    pub file_to_send: Option<PathBuf>,
    // pub show_about: bool, REMOVED
    pub chat_to_delete: Option<Uuid>,
    pub history_path: PathBuf,
    pub history_loaded: bool,
    pub show_emoji_picker: bool,
    pub last_typing_time: Option<std::time::Instant>,
    pub typing_stopped: bool,
    // Password dialogs
    // pub show_password_dialog: bool, REMOVED
    // pub show_set_password_dialog: bool, REMOVED
    pub password_input: String,
    pub new_password_input: String,
    pub confirm_password_input: String,
    pub identity_locked: bool,
    pub force_password_setup: bool,
    // Fingerprint verification dialog
    // pub show_fingerprint_dialog: bool, REMOVED
    pub fingerprint_to_verify: Option<String>,
    pub peer_name_to_verify: Option<String>,
    pub chat_id_to_verify: Option<Uuid>,
    pub show_log_terminal: bool,
    // pub show_clear_history_dialog: bool, REMOVED
    pub event_collector: EventCollector,
    pub toasts: Vec<Toast>,
    pub is_new_identity: bool,
    pub qr_code_texture: Option<egui::TextureHandle>,
    /// Discovered peers on the local network via mDNS.
    pub discovered_peers: Arc<StdMutex<Vec<DiscoveredPeer>>>,
    /// mDNS Discovery service instance.
    pub discovery: Option<Discovery>,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        event_collector: EventCollector,
    ) -> anyhow::Result<Self> {
        let chat_manager = ChatManager::new(Config::default());
        let theme = chat_manager.config.theme;
        cc.egui_ctx
            .set_visuals(crate::gui::styling::apply_custom_visuals(&theme));

        // Load fonts.
        let fonts = egui::FontDefinitions::default();
        cc.egui_ctx.set_fonts(fonts);

        let mut chat_manager = ChatManager::new(Config::default());
        let initial_show_log_terminal = chat_manager.config.show_log_terminal;

        let proj_dirs = directories::ProjectDirs::from("com", "chat-p2p", "EncryptedMessenger");

        // Auto-restore conversation history from platform-specific user data directory
        let (history_path, identity, is_new_identity) = if let Some(ref dirs) = proj_dirs {
            let data_dir = dirs.data_dir();
            std::fs::create_dir_all(data_dir).ok(); // Ensure directory exists

            // Load or create user identity
            let (identity, is_new) =
                match crate::identity::Identity::get_or_create(data_dir, "User") {
                    Ok(id) => id,
                    Err(e) => {
                        tracing::error!("Failed to load/create identity: {}. Trying fallback.", e);
                        let identity =
                            crate::identity::Identity::new_with_plaintext("User".to_string())?;
                        (identity, true)
                    }
                };

            (data_dir.join("history.json.enc"), identity, is_new)
        } else {
            // Fallback to relative path if directories crate fails
            tracing::warn!("Could not determine user data directory, using fallback path");
            let identity = crate::identity::Identity::new_with_plaintext("User".to_string())?;
            (
                PathBuf::from("Downloads").join("history.json.enc"),
                identity,
                true,
            )
        };

        tracing::info!("Using history path: {}", history_path.display());
        tracing::info!(
            "Using identity: {} (fingerprint: {}...)",
            identity.name,
            &identity.fingerprint[..16]
        );

        let active_dialog = if !history_path.exists() {
            ActiveDialog::Welcome
        } else {
            ActiveDialog::None
        };

        // Derive history key from identity if it's not locked
        if !identity.is_locked() {
            if let Ok(key) = identity.history_key() {
                chat_manager.set_history_key(key);

                // Try to load history with auto-migration support
                if let Err(e) = chat_manager.load_history_auto(&history_path, &key) {
                    tracing::warn!("Failed to load history: {}", e);
                }
            } else {
                tracing::warn!("Could not derive history key from identity");
            }
        } else if history_path.exists() {
            // History exists but identity is locked - can't load yet
            tracing::warn!("History exists but identity is locked. Will load after unlock.");
        }

        // If history wasn't loaded yet and no key was available (identity locked),
        // the history will be loaded/decrypted after the user unlocks the identity.

        // After loading history, override default relative paths with absolute paths if possible
        if let Some(dirs) = proj_dirs {
            // If download_dir is still the default "Downloads", resolve it to an absolute path.
            if chat_manager.config.download_dir == PathBuf::from("Downloads") {
                let download_path = UserDirs::new()
                    .and_then(|user_dirs| user_dirs.download_dir().map(|dir| dir.to_path_buf()))
                    .unwrap_or_else(|| dirs.data_dir().to_path_buf());
                chat_manager.config.download_dir = download_path.join("EncryptedP2PMessenger");
            }
            // If temp_dir is still the default "temp", resolve it.
            if chat_manager.config.temp_dir == PathBuf::from("temp") {
                chat_manager.config.temp_dir = dirs.data_dir().join("temp");
            }
        }

        // Capture config before moving manager
        let auto_host_enabled = chat_manager.config.auto_host_on_startup;
        let auto_host_port = chat_manager.config.listen_port;
        let auto_connect_enabled = chat_manager.config.auto_connect;
        let enable_mdns = chat_manager.config.enable_mdns;
        // Capture listen_port for initializing the UI field before moving manager
        let host_port_ui = auto_host_port.to_string();
        let initial_identity_locked = identity.is_locked();
        // Force password setup whenever the identity is not locked (i.e., private key available in plaintext)
        let force_password_setup = !initial_identity_locked;
        // Wrap manager in Arc<Mutex<..>> once and reuse
        let manager_arc = Arc::new(Mutex::new(chat_manager));
        // Do not start network (auto_host, auto_connect) until identity is unlocked or password set
        let auth_blocked = initial_identity_locked || is_new_identity || force_password_setup;
        if !auth_blocked {
            if auto_host_enabled {
                match identity.private_key() {
                    Ok(privkey) => {
                        tracing::info!(port = %auto_host_port, "Auto-host on startup is enabled; starting host");
                        let mgr_clone = manager_arc.clone();
                        tokio::spawn(async move {
                            let mut mgr = mgr_clone.lock().await;
                            if let Err(e) = mgr.start_host(auto_host_port, privkey).await {
                                mgr.add_toast(
                                    crate::types::ToastLevel::Error,
                                    format!("Failed to auto-start host: {}", e),
                                );
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Skipping auto-host: identity key unavailable");
                    }
                }
            }
            if auto_connect_enabled {
                match identity.private_key() {
                    Ok(privkey) => {
                        let mgr_clone = manager_arc.clone();
                        tokio::spawn(async move {
                            let mut mgr = mgr_clone.lock().await;
                            mgr.auto_reconnect_contacts(&privkey).await;
                        });
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Skipping auto-reconnect: identity key unavailable");
                    }
                }
            }
        }

        Ok(Self {
            chat_manager: manager_arc,
            identity,
            selected_chat: None,
            input_text: String::new(),
            active_dialog: if initial_identity_locked {
                ActiveDialog::Password
            } else {
                active_dialog
            },
            connect_host: String::new(),
            connect_port: PORT_DEFAULT.to_string(),
            host_port: host_port_ui,
            file_to_send: None,
            chat_to_delete: None,
            contact_tab: 0,
            help_tab: 0,
            new_contact_name: String::new(),
            new_contact_address: String::new(),
            new_contact_fingerprint: String::new(),
            new_contact_pubkey: String::new(),
            invite_link_input: String::new(),
            my_invite_link: None,
            group_wizard_step: 0,
            group_selected: Vec::new(),
            group_title: String::new(),
            group_search: String::new(),
            rename_chat_id: None,
            rename_input: String::new(),
            history_path,
            history_loaded: false,
            show_emoji_picker: false,
            last_typing_time: None,
            typing_stopped: false,
            password_input: String::new(),
            new_password_input: String::new(),
            confirm_password_input: String::new(),
            identity_locked: initial_identity_locked,
            force_password_setup,
            // Fingerprint verification dialog
            fingerprint_to_verify: None,
            peer_name_to_verify: None,
            chat_id_to_verify: None,
            show_log_terminal: initial_show_log_terminal,
            event_collector,
            toasts: Vec::new(),
            is_new_identity,
            qr_code_texture: None,
            discovered_peers: Arc::new(StdMutex::new(Vec::new())),
            discovery: if enable_mdns {
                // Create discovery only when mDNS is explicitly enabled in config.
                // This avoids broadcasting hostname and fingerprint by default.
                Discovery::new().ok()
            } else {
                None
            },
        })
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
        let privkey = match self.identity.private_key() {
            Ok(k) => k,
            Err(e) => {
                self.add_toast(
                    crate::types::ToastLevel::Error,
                    format!("Cannot start host: {}", e),
                );
                return;
            }
        };
        let manager = self.chat_manager.clone();

        tokio::spawn(async move {
            let mut mgr = manager.lock().await;
            if let Err(e) = mgr.start_host(port, privkey).await {
                mgr.add_toast(
                    crate::types::ToastLevel::Error,
                    format!("Failed to start host: {}", e),
                );
            }
        });
    }

    pub fn connect_clicked(&mut self) {
        let combined = if self.connect_host.contains(':') || self.connect_port.trim().is_empty() {
            self.connect_host.clone()
        } else {
            crate::util::format_host_port(
                self.connect_host.trim(),
                self.connect_port.parse().unwrap_or(crate::PORT_DEFAULT),
            )
        };
        let (host, port) = match crate::util::parse_host_port(&combined, Some(crate::PORT_DEFAULT))
        {
            Ok(parsed) => parsed,
            Err(e) => {
                self.add_toast(
                    crate::types::ToastLevel::Error,
                    format!("Cannot connect: {}", e),
                );
                return;
            }
        };
        let privkey = match self.identity.private_key() {
            Ok(k) => k,
            Err(e) => {
                self.add_toast(
                    crate::types::ToastLevel::Error,
                    format!("Cannot connect: {}", e),
                );
                return;
            }
        };
        let manager = self.chat_manager.clone();
        let existing_chat = self.selected_chat; // bind connection to the currently selected chat if any

        tokio::spawn(async move {
            let mut mgr = manager.lock().await;
            if let Err(e) = mgr
                .connect_to_host(&host, port, existing_chat, privkey)
                .await
            {
                mgr.add_toast(
                    crate::types::ToastLevel::Error,
                    format!("Failed to connect: {}", e),
                );
            }
        });
    }

    pub fn add_toast(&mut self, level: ToastLevel, message: String) {
        self.toasts.push(Toast {
            id: Uuid::new_v4(),
            level,
            message,
            created_at: std::time::Instant::now(),
            duration: std::time::Duration::from_secs(4),
        });
    }

    /// Returns true if any dialog that should be modal is currently open.
    pub fn is_any_modal_open(&self) -> bool {
        self.active_dialog != ActiveDialog::None
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Block entire UI until identity is unlocked or password is set. User cannot bypass.
        if self.identity.is_locked() || self.is_new_identity || self.force_password_setup {
            crate::gui::dialogs::render_blocking_auth_screen(self, ctx);
            crate::gui::dialogs::render_toasts(self, ctx);
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }

        // Always poll session events to keep the app responsive
        if let Ok(mut manager) = self.chat_manager.try_lock() {
            manager.poll_session_events();
            if let Some((fingerprint, peer_name, chat_id)) =
                manager.fingerprint_verification_request.take()
            {
                self.fingerprint_to_verify = Some(fingerprint);
                self.peer_name_to_verify = Some(peer_name);
                self.chat_id_to_verify = Some(chat_id);
                self.active_dialog = ActiveDialog::FingerprintVerification;
            }
            manager.cleanup_expired_toasts();

            // Auto-save history periodically
            use std::sync::atomic::{AtomicU64, Ordering};
            use std::sync::OnceLock;
            static LAST_SAVE_MILLIS: OnceLock<AtomicU64> = OnceLock::new();

            let last_save = LAST_SAVE_MILLIS.get_or_init(|| AtomicU64::new(0));
            let now_millis = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let last = last_save.load(Ordering::Relaxed);
            let should_save = last == 0 || now_millis.saturating_sub(last) > 30_000;

            if should_save && !manager.chats.is_empty() {
                match manager.history_snapshot() {
                    Ok((history, key)) => {
                        let path = self.history_path.clone();
                        last_save.store(now_millis, Ordering::Relaxed);
                        tokio::spawn(async move {
                            match tokio::task::spawn_blocking(move || {
                                history.save_encrypted(&path, &key)
                            })
                            .await
                            {
                                Ok(Err(e)) => tracing::warn!("Background auto-save failed: {}", e),
                                Err(e) => tracing::warn!("Background auto-save task failed: {}", e),
                                Ok(Ok(())) => {}
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!("Failed to prepare auto-save history snapshot: {}", e);
                    }
                }
            }

            // Auto-rehost: if auto-host is enabled and no placeholder host chat exists,
            // spawn a new host to replace the one that was consumed by a connection.
            if manager.config.auto_host_on_startup {
                let has_placeholder = manager.chats.values().any(|c| c.is_host_placeholder);
                if !has_placeholder {
                    use std::sync::atomic::{AtomicU64, Ordering};
                    use std::sync::OnceLock;
                    static LAST_REHOST_MILLIS: OnceLock<AtomicU64> = OnceLock::new();

                    let last_rehost = LAST_REHOST_MILLIS.get_or_init(|| AtomicU64::new(0));
                    let now_millis = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    let last = last_rehost.load(Ordering::Relaxed);
                    let mut should_rehost = false;
                    let timer_expired = last == 0 || now_millis.saturating_sub(last) > 1500;

                    if timer_expired {
                        if let Ok(manager) = self.chat_manager.try_lock() {
                            should_rehost = manager.check_rehost_needed();
                        }
                    }

                    if should_rehost {
                        last_rehost.store(now_millis, Ordering::Relaxed);
                        let port = manager.config.listen_port;
                        match self.identity.private_key() {
                            Ok(privkey) => {
                                let mgr_arc = self.chat_manager.clone();
                                tokio::spawn(async move {
                                    let mut mgr = mgr_arc.lock().await;
                                    if let Err(e) = mgr.start_host(port, privkey).await {
                                        mgr.add_toast(
                                            crate::types::ToastLevel::Error,
                                            format!("Failed to re-start host: {}", e),
                                        );
                                    } else {
                                        mgr.add_toast(
                                            crate::types::ToastLevel::Success,
                                            "Host restarted".to_string(),
                                        );
                                    }
                                });
                            }
                            Err(e) => {
                                manager.add_toast(
                                    crate::types::ToastLevel::Error,
                                    format!("Cannot re-host: {}", e),
                                );
                            }
                        }
                    }
                }
            }
        }

        let any_modal_open = self.is_any_modal_open();

        // Top panel - Menu bar
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.add_enabled_ui(!any_modal_open, |ui| {
                ui.horizontal(|ui| {
                    // Connection menu
                    ui.menu_button("🔌 Connection", |ui| {
                        if ui.button("🎤 Start Host").clicked() {
                            if let Ok(manager) = self.chat_manager.try_lock() {
                                self.host_port = manager.config.listen_port.to_string();
                            }
                            self.active_dialog = ActiveDialog::Host;
                            ui.close_menu();
                        }
                        if ui.button("🔌 Connect to Host").clicked() {
                            self.connect_host.clear();
                            self.connect_port = crate::PORT_DEFAULT.to_string();
                            self.active_dialog = ActiveDialog::Connect;
                            ui.close_menu();
                        }
                    });

                    if ui.button("Contacts").clicked() {
                        self.active_dialog = ActiveDialog::Contacts;
                    }

                    if ui.button("Settings").clicked() {
                        self.active_dialog = ActiveDialog::Settings;
                    }

                    if ui.button("Help").clicked() {
                        self.active_dialog = ActiveDialog::About;
                    }
                });
            });
        });

        // Status bar: hosting + connectivity summary
        egui::TopBottomPanel::top("status_panel")
            .resizable(false)
            .show(ctx, |ui| {
                ui.add_enabled_ui(!any_modal_open, |ui| {
                    ui.add_space(crate::gui::styling::SPACING_MEDIUM);
                    if let Ok(manager) = self.chat_manager.try_lock() {
                        let listen_port = manager.config.listen_port;
                        let is_listening = manager.is_hosting;
                        let sessions = manager.sessions_len();
                        let toasts = manager.toasts.len();

                        ui.horizontal_wrapped(|ui| {
                            if is_listening {
                                ui.colored_label(
                                    crate::gui::styling::SUCCESS,
                                    format!("🟢 Hosting on :{}", listen_port),
                                );
                                if ui.button("Copy address").clicked() {
                                    if let Some(ip) = crate::util::primary_local_ipv4() {
                                        ui.output_mut(|o| {
                                            o.copied_text =
                                                crate::util::format_host_port(&ip, listen_port)
                                        });
                                    } else {
                                        ui.output_mut(|o| {
                                            o.copied_text = format!("localhost:{listen_port}")
                                        });
                                    }
                                }
                            } else {
                                ui.colored_label(crate::gui::styling::ERROR, "⚠ Not hosting");
                            }

                            ui.separator();
                            ui.label(format!("Sessions: {sessions}"));
                            ui.separator();
                            ui.label(format!("Toasts: {toasts}"));
                        });
                    } else {
                        ui.colored_label(
                            crate::gui::styling::SUBTLE_TEXT_COLOR,
                            "Status unavailable (busy)",
                        );
                    }
                });
            });

        // Sidebar - Chat list
        egui::SidePanel::left("sidebar")
            .default_width(260.0)
            .show(ctx, |ui| {
                ui.add_enabled_ui(!any_modal_open, |ui| {
                    crate::gui::sidebar::render_sidebar(self, ui);
                });
            });

        // Main panel - Messages
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_enabled_ui(!any_modal_open, |ui| {
                if let Some(chat_id) = self.selected_chat {
                    crate::gui::chat_view::render_chat(self, ui, chat_id);
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.vertical_centered(|ui| {
                            ui.heading("No chat selected");
                            ui.label("Start hosting, connect to a peer, or invite a friend.");
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                if ui.button("🎤 Start Host").clicked() {
                                    if let Ok(manager) = self.chat_manager.try_lock() {
                                        self.host_port = manager.config.listen_port.to_string();
                                    }
                                    self.active_dialog = ActiveDialog::Host;
                                }
                                if ui.button("🔌 Connect").clicked() {
                                    self.connect_host.clear();
                                    self.connect_port = PORT_DEFAULT.to_string();
                                    self.active_dialog = ActiveDialog::Connect;
                                }
                                if ui.button("📨 Invite").clicked() {
                                    self.active_dialog = ActiveDialog::AddContact;
                                    self.contact_tab = 1; // invite link tab
                                }
                            });
                        });
                    });
                }
            });
        });

        // Toasts overlay
        crate::gui::dialogs::render_toasts(self, ctx);

        // Dialogs
        crate::gui::dialogs::render_dialogs(self, ctx);

        // Request repaint for animations
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}
