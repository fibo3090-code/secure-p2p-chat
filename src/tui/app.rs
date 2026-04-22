use crate::app::chat_manager::ChatManager;
use crate::identity::Identity;
use crate::types::Config;
use anyhow::Result;
use egui_tracing::tracing::EventCollector;
use ratatui::widgets::ListState;
use ratatui_crossterm::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiFocus {
    ChatList,
    MessageView,
    Input,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiMode {
    Normal,
    Command,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TuiCommand {
    Host(Option<u16>),
    Connect {
        host: String,
        port: u16,
    },
    HostRelay {
        relay: String,
        token: Option<String>,
    },
    ConnectRelay {
        relay: String,
        token: String,
    },
    Disconnect,
    Diagnostics,
    Rename(String),
    Help,
    Quit,
}

pub struct TuiApp {
    pub chat_manager: ChatManager,
    pub chat_list_state: ListState,
    pub chat_ids: Vec<Uuid>,
    pub input_text: String,
    pub message_scroll: u16,
    pub identity_name: String,
    pub event_collector: EventCollector,
    pub focus: TuiFocus,
    pub mode: TuiMode,
    pub command_buffer: String,
    pub status_line: String,
    pub should_quit: bool,
    identity: Identity,
    history_path: PathBuf,
    pending_command: Option<TuiCommand>,
}

impl TuiApp {
    pub fn new(event_collector: EventCollector) -> Result<Self> {
        let mut chat_manager = ChatManager::new(Config::default());

        let proj_dirs = directories::ProjectDirs::from("com", "chat-p2p", "EncryptedMessenger");

        let (history_path, identity, _is_new_identity) = if let Some(ref dirs) = proj_dirs {
            let data_dir = dirs.data_dir();
            std::fs::create_dir_all(data_dir).ok();

            let (identity, is_new) = Identity::get_or_create(data_dir, "TUI_User")?;
            (data_dir.join("history.json.enc"), identity, is_new)
        } else {
            let identity = Identity::new_with_plaintext("TUI_User".to_string())?;
            (PathBuf::from("history.json.enc"), identity, true)
        };

        if !identity.is_locked() {
            if let Ok(key) = identity.history_key() {
                chat_manager.set_history_key(key);
                if let Err(e) = chat_manager.load_history_auto(&history_path, &key) {
                    tracing::warn!("Failed to load TUI history: {}", e);
                }
            }
        }

        let mut app = Self {
            chat_manager,
            chat_list_state: ListState::default(),
            chat_ids: Vec::new(),
            input_text: String::new(),
            message_scroll: 0,
            identity_name: identity.name.clone(),
            event_collector,
            focus: TuiFocus::Input,
            mode: TuiMode::Normal,
            command_buffer: String::new(),
            status_line: "Ready. Press :help for commands.".to_string(),
            should_quit: false,
            identity,
            history_path,
            pending_command: None,
        };
        app.sync_chat_ids();
        app.refresh_status_line();

        Ok(app)
    }

    pub fn selected_chat_id(&self) -> Option<Uuid> {
        self.chat_list_state
            .selected()
            .and_then(|idx| self.chat_ids.get(idx).copied())
    }

    pub fn sync_chat_ids(&mut self) {
        let selected_chat_id = self.selected_chat_id();
        let mut chats: Vec<_> = self.chat_manager.chats.values().collect();
        chats.sort_by_key(|chat| chat.created_at);
        self.chat_ids = chats.into_iter().map(|c| c.id).collect();

        if self.chat_ids.is_empty() {
            self.chat_list_state.select(None);
            return;
        }

        if let Some(selected) = selected_chat_id {
            if let Some(new_idx) = self.chat_ids.iter().position(|id| *id == selected) {
                self.chat_list_state.select(Some(new_idx));
                return;
            }
        }

        let invalid_selection = self
            .chat_list_state
            .selected()
            .is_none_or(|idx| idx >= self.chat_ids.len());
        if invalid_selection {
            self.chat_list_state.select(Some(0));
        }
    }

    pub fn tick(&mut self) {
        self.chat_manager.poll_session_events();
        self.chat_manager.cleanup_expired_toasts();
        self.sync_chat_ids();
        self.refresh_status_line();
    }

    fn refresh_status_line(&mut self) {
        let session_count = self.chat_manager.sessions_len();
        let selected_state = if let Some(chat_id) = self.selected_chat_id() {
            if self.chat_manager.is_connected(&chat_id) {
                "connected"
            } else {
                "disconnected"
            }
        } else {
            "no-chat"
        };

        let focus_label = match self.focus {
            TuiFocus::ChatList => "focus:chats",
            TuiFocus::MessageView => "focus:messages",
            TuiFocus::Input => "focus:input",
        };
        let mode_label = match self.mode {
            TuiMode::Normal => "mode:normal",
            TuiMode::Command => "mode:command",
        };

        let mut line = format!(
            "{} | {} | sessions:{} | selected:{}",
            focus_label, mode_label, session_count, selected_state
        );

        if let Some(toast) = self.chat_manager.toasts.last() {
            line.push_str(&format!(" | last:{}", toast.message));
        } else {
            line.push_str(" | Tab focus | : commands | Enter send | Ctrl+J newline");
        }

        self.status_line = line;
    }

    pub fn enter_command_mode(&mut self) {
        self.mode = TuiMode::Command;
        self.command_buffer.clear();
    }

    pub fn cancel_command_mode(&mut self) {
        self.mode = TuiMode::Normal;
        self.command_buffer.clear();
    }

    pub fn take_pending_command(&mut self) -> Option<TuiCommand> {
        self.pending_command.take()
    }

    pub fn parse_command(raw: &str) -> std::result::Result<TuiCommand, String> {
        let input = raw.trim().trim_start_matches(':').trim();
        if input.is_empty() {
            return Err("Empty command. Try :help".to_string());
        }

        let mut parts = input.split_whitespace();
        let cmd = parts.next().unwrap_or_default();

        match cmd {
            "host" => {
                let port = parts
                    .next()
                    .map(|p| {
                        p.parse::<u16>()
                            .map_err(|_| "Invalid port. Example: :host 9000".to_string())
                    })
                    .transpose()?;
                Ok(TuiCommand::Host(port))
            }
            "connect" => {
                let target = parts
                    .next()
                    .ok_or_else(|| "Usage: :connect <host[:port]>".to_string())?;
                let (host, port) = if let Some((h, p)) = target.rsplit_once(':') {
                    let parsed_port = p
                        .parse::<u16>()
                        .map_err(|_| "Invalid port in :connect".to_string())?;
                    (h.to_string(), parsed_port)
                } else {
                    (target.to_string(), crate::PORT_DEFAULT)
                };
                if host.trim().is_empty() {
                    return Err("Host cannot be empty".to_string());
                }
                Ok(TuiCommand::Connect { host, port })
            }
            "host-relay" => {
                let relay = parts
                    .next()
                    .ok_or_else(|| "Usage: :host-relay <relay[:port]> [token]".to_string())?;
                let token = parts.next().map(str::to_string);
                Ok(TuiCommand::HostRelay {
                    relay: relay.to_string(),
                    token,
                })
            }
            "connect-relay" => {
                let relay = parts
                    .next()
                    .ok_or_else(|| "Usage: :connect-relay <relay[:port]> <token>".to_string())?;
                let token = parts
                    .next()
                    .ok_or_else(|| "Usage: :connect-relay <relay[:port]> <token>".to_string())?;
                Ok(TuiCommand::ConnectRelay {
                    relay: relay.to_string(),
                    token: token.to_string(),
                })
            }
            "disconnect" => Ok(TuiCommand::Disconnect),
            "diagnostics" | "diag" => Ok(TuiCommand::Diagnostics),
            "rename" => {
                let title = input
                    .strip_prefix("rename")
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                if title.is_empty() {
                    return Err("Usage: :rename <new title>".to_string());
                }
                Ok(TuiCommand::Rename(title))
            }
            "help" => Ok(TuiCommand::Help),
            "quit" | "q" => Ok(TuiCommand::Quit),
            _ => Err(format!("Unknown command: {}. Try :help", cmd)),
        }
    }

    pub fn submit_command(&mut self) {
        match Self::parse_command(&self.command_buffer) {
            Ok(cmd) => {
                self.pending_command = Some(cmd);
                self.mode = TuiMode::Normal;
                self.command_buffer.clear();
            }
            Err(e) => {
                self.chat_manager
                    .add_toast(crate::types::ToastLevel::Error, e);
                self.mode = TuiMode::Normal;
                self.command_buffer.clear();
            }
        }
    }

    pub fn cycle_focus(&mut self) {
        self.focus = match self.focus {
            TuiFocus::ChatList => TuiFocus::MessageView,
            TuiFocus::MessageView => TuiFocus::Input,
            TuiFocus::Input => TuiFocus::ChatList,
        };
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) {
        if self.mode == TuiMode::Command {
            match key.code {
                KeyCode::Esc => self.cancel_command_mode(),
                KeyCode::Enter => self.submit_command(),
                KeyCode::Backspace => {
                    self.command_buffer.pop();
                }
                KeyCode::Char(c) => {
                    self.command_buffer.push(c);
                }
                _ => {}
            }
            return;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('l') {
            self.copy_logs();
            return;
        }

        if key.code == KeyCode::Tab {
            self.cycle_focus();
            return;
        }

        if key.code == KeyCode::Esc {
            self.focus = TuiFocus::ChatList;
            return;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('j') {
            if self.focus == TuiFocus::Input {
                self.input_text.push('\n');
            }
            return;
        }

        match key.code {
            KeyCode::Char(':') => {
                if self.focus != TuiFocus::Input || self.input_text.is_empty() {
                    self.enter_command_mode();
                } else {
                    self.input_text.push(':');
                }
            }
            KeyCode::Char('q') if self.focus != TuiFocus::Input => {
                self.should_quit = true;
            }
            KeyCode::Down => match self.focus {
                TuiFocus::ChatList => self.next_chat(),
                TuiFocus::MessageView => self.scroll_down(),
                TuiFocus::Input => {}
            },
            KeyCode::Up => match self.focus {
                TuiFocus::ChatList => self.previous_chat(),
                TuiFocus::MessageView => self.scroll_up(),
                TuiFocus::Input => {}
            },
            KeyCode::PageDown => self.scroll_down(),
            KeyCode::PageUp => self.scroll_up(),
            KeyCode::Enter if self.focus == TuiFocus::Input => self.send_message(),
            KeyCode::Backspace if self.focus == TuiFocus::Input => {
                self.input_text.pop();
            }
            KeyCode::Char(c) if self.focus == TuiFocus::Input => {
                self.input_text.push(c);
            }
            _ => {}
        }
    }

    pub async fn execute_command(&mut self, cmd: TuiCommand) {
        match cmd {
            TuiCommand::Host(port_override) => {
                let port = port_override.unwrap_or(self.chat_manager.config.listen_port);
                match self.identity.private_key() {
                    Ok(privkey) => match self.chat_manager.start_host(port, privkey).await {
                        Ok(_) => {
                            self.chat_manager.add_toast(
                                crate::types::ToastLevel::Success,
                                format!("Hosting on :{}", port),
                            );
                        }
                        Err(e) => self.chat_manager.add_toast(
                            crate::types::ToastLevel::Error,
                            format!("Failed to start host: {}", e),
                        ),
                    },
                    Err(e) => self.chat_manager.add_toast(
                        crate::types::ToastLevel::Error,
                        format!("Cannot start host: {}", e),
                    ),
                }
            }
            TuiCommand::Connect { host, port } => match self.identity.private_key() {
                Ok(privkey) => match self
                    .chat_manager
                    .connect_to_host(&host, port, None, privkey)
                    .await
                {
                    Ok(chat_id) => {
                        self.chat_manager.add_toast(
                            crate::types::ToastLevel::Info,
                            format!("Connecting to {}:{}...", host, port),
                        );
                        self.sync_chat_ids();
                        if let Some(idx) = self.chat_ids.iter().position(|id| *id == chat_id) {
                            self.chat_list_state.select(Some(idx));
                        }
                    }
                    Err(e) => self.chat_manager.add_toast(
                        crate::types::ToastLevel::Error,
                        format!("Connect failed: {}", e),
                    ),
                },
                Err(e) => self.chat_manager.add_toast(
                    crate::types::ToastLevel::Error,
                    format!("Cannot connect: {}", e),
                ),
            },
            TuiCommand::HostRelay { relay, token } => match self.identity.private_key() {
                Ok(privkey) => match self
                    .chat_manager
                    .start_host_via_relay(&relay, token, privkey)
                    .await
                {
                    Ok((chat_id, relay_token)) => {
                        if let Ok(invite_link) =
                            self.identity.generate_signed_invite_link_with_route(
                                None,
                                Some(relay.clone()),
                                Some(relay_token.clone()),
                            )
                        {
                            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                let _ = clipboard.set_text(invite_link);
                            }
                        }
                        self.chat_manager.add_toast(
                            crate::types::ToastLevel::Success,
                            format!("Relay host ready via {} (token {})", relay, relay_token),
                        );
                        self.sync_chat_ids();
                        if let Some(idx) = self.chat_ids.iter().position(|id| *id == chat_id) {
                            self.chat_list_state.select(Some(idx));
                        }
                    }
                    Err(e) => self.chat_manager.add_toast(
                        crate::types::ToastLevel::Error,
                        format!("Failed to start relay host: {}", e),
                    ),
                },
                Err(e) => self.chat_manager.add_toast(
                    crate::types::ToastLevel::Error,
                    format!("Cannot start relay host: {}", e),
                ),
            },
            TuiCommand::ConnectRelay { relay, token } => match self.identity.private_key() {
                Ok(privkey) => match self
                    .chat_manager
                    .connect_via_relay(&relay, &token, None, privkey)
                    .await
                {
                    Ok(chat_id) => {
                        self.chat_manager.add_toast(
                            crate::types::ToastLevel::Info,
                            format!("Connecting via relay {}...", relay),
                        );
                        self.sync_chat_ids();
                        if let Some(idx) = self.chat_ids.iter().position(|id| *id == chat_id) {
                            self.chat_list_state.select(Some(idx));
                        }
                    }
                    Err(e) => self.chat_manager.add_toast(
                        crate::types::ToastLevel::Error,
                        format!("Relay connect failed: {}", e),
                    ),
                },
                Err(e) => self.chat_manager.add_toast(
                    crate::types::ToastLevel::Error,
                    format!("Cannot connect via relay: {}", e),
                ),
            },
            TuiCommand::Disconnect => {
                if let Some(chat_id) = self.selected_chat_id() {
                    let is_placeholder = self
                        .chat_manager
                        .get_chat(chat_id)
                        .map(|c| c.is_host_placeholder)
                        .unwrap_or(false);
                    if is_placeholder {
                        self.chat_manager.stop_hosting();
                    } else {
                        self.chat_manager.delete_chat(chat_id);
                    }
                } else {
                    self.chat_manager.add_toast(
                        crate::types::ToastLevel::Warning,
                        "No selected chat to disconnect".to_string(),
                    );
                }
            }
            TuiCommand::Diagnostics => {
                self.export_diagnostics_bundle();
            }
            TuiCommand::Rename(new_title) => {
                if let Some(chat_id) = self.selected_chat_id() {
                    if let Err(e) = self.chat_manager.rename_chat(chat_id, new_title) {
                        self.chat_manager.add_toast(
                            crate::types::ToastLevel::Error,
                            format!("Rename failed: {}", e),
                        );
                    } else {
                        self.chat_manager.add_toast(
                            crate::types::ToastLevel::Success,
                            "Chat renamed".to_string(),
                        );
                    }
                } else {
                    self.chat_manager.add_toast(
                        crate::types::ToastLevel::Warning,
                        "No selected chat to rename".to_string(),
                    );
                }
            }
            TuiCommand::Help => {
                self.chat_manager.add_toast(
                    crate::types::ToastLevel::Info,
                    "Commands: :host [port], :connect <host[:port]>, :host-relay <relay[:port]> [token], :connect-relay <relay[:port]> <token>, :disconnect, :diagnostics, :rename <title>, :help, :quit"
                        .to_string(),
                );
            }
            TuiCommand::Quit => {
                self.should_quit = true;
            }
        }
        self.refresh_status_line();
    }

    pub fn copy_logs(&mut self) {
        let log_text = crate::support::format_event_logs(&self.event_collector);

        match arboard::Clipboard::new() {
            Ok(mut clipboard) => {
                if let Err(e) = clipboard.set_text(log_text) {
                    tracing::error!("Failed to copy logs to clipboard: {}", e);
                    self.chat_manager.add_toast(
                        crate::types::ToastLevel::Error,
                        format!("Failed to copy logs: {}", e),
                    );
                } else {
                    tracing::info!("Logs copied to clipboard (TUI)");
                    self.chat_manager.add_toast(
                        crate::types::ToastLevel::Success,
                        "Logs copied to clipboard".to_string(),
                    );
                }
            }
            Err(e) => {
                tracing::error!("Failed to initialize clipboard: {}", e);
                self.chat_manager.add_toast(
                    crate::types::ToastLevel::Error,
                    format!("Clipboard error: {}", e),
                );
            }
        }
        self.refresh_status_line();
    }

    pub fn export_diagnostics_bundle(&mut self) {
        let identity_path = self.history_path.with_file_name("identity.json");
        let report = crate::support::DiagnosticsReport {
            generated_at_utc: chrono::Utc::now().to_rfc3339(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            history_path: self.history_path.display().to_string(),
            identity_path: identity_path.display().to_string(),
            history_exists: self.history_path.exists(),
            identity_exists: identity_path.exists(),
            identity_locked: self.identity.is_locked(),
            identity_name: self.identity.name.clone(),
            identity_fingerprint_prefix: self.identity.fingerprint.chars().take(16).collect(),
            chats: self.chat_manager.chats.len(),
            contacts: self.chat_manager.contacts.len(),
            sessions: self.chat_manager.sessions_len(),
            active_toasts: self.chat_manager.toasts.len(),
            discovered_peers: 0,
            config: crate::support::DiagnosticsConfig::from(&self.chat_manager.config),
        };
        let logs = crate::support::format_event_logs(&self.event_collector);
        let base_dir = self
            .history_path
            .parent()
            .map(|dir| dir.join("diagnostics"))
            .unwrap_or_else(crate::support::default_diagnostics_dir);

        match crate::support::export_diagnostics_bundle(&base_dir, &report, &logs) {
            Ok(bundle_dir) => self.chat_manager.add_toast(
                crate::types::ToastLevel::Success,
                format!("Diagnostics exported to {}", bundle_dir.display()),
            ),
            Err(e) => self.chat_manager.add_toast(
                crate::types::ToastLevel::Error,
                format!("Failed to export diagnostics: {}", e),
            ),
        }
        self.refresh_status_line();
    }

    pub fn next_chat(&mut self) {
        if self.chat_ids.is_empty() {
            return;
        }
        let i = match self.chat_list_state.selected() {
            Some(i) => {
                if i >= self.chat_ids.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.chat_list_state.select(Some(i));
        self.message_scroll = 0;
    }

    pub fn previous_chat(&mut self) {
        if self.chat_ids.is_empty() {
            return;
        }
        let i = match self.chat_list_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.chat_ids.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.chat_list_state.select(Some(i));
        self.message_scroll = 0;
    }

    pub fn scroll_up(&mut self) {
        self.message_scroll = self.message_scroll.saturating_sub(1);
    }

    pub fn scroll_down(&mut self) {
        self.message_scroll = self.message_scroll.saturating_add(1);
    }

    pub fn send_message(&mut self) {
        if let Some(selected_index) = self.chat_list_state.selected() {
            if let Some(chat_id) = self.chat_ids.get(selected_index) {
                let text_to_send = self.input_text.trim().to_string();
                if !text_to_send.is_empty() {
                    if let Err(e) = self.chat_manager.send_message(*chat_id, text_to_send) {
                        tracing::error!("Failed to send message: {}", e);
                    }
                    self.input_text.clear();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{TuiApp, TuiCommand, TuiFocus, TuiMode};
    use crate::app::chat_manager::SessionHandle;
    use egui_tracing::tracing::EventCollector;
    use ratatui_crossterm::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tokio::sync::mpsc;
    use uuid::Uuid;

    use crate::core::ProtocolMessage;

    // Helper to setup a chat that can actually receive messages (has a session)
    // We return the receiver so it doesn't get dropped (which closes the channel)
    fn setup_chat_with_session(
        app: &mut TuiApp,
        name: &str,
    ) -> (Uuid, mpsc::UnboundedReceiver<ProtocolMessage>) {
        let chat_id = Uuid::new_v4();
        app.chat_manager
            .create_local_chat_for_test(chat_id, name.to_string());

        // Create dummy session
        let (tx, rx) = mpsc::unbounded_channel();
        let session = SessionHandle { from_app_tx: tx };
        app.chat_manager.add_session_for_test(chat_id, session);

        app.chat_ids = vec![chat_id];
        app.chat_list_state.select(Some(0));
        (chat_id, rx)
    }

    #[test]
    fn test_new_app() {
        let app = TuiApp::new(EventCollector::new()).unwrap();
        assert_eq!(app.input_text, "");
        assert_eq!(app.message_scroll, 0);
        assert_eq!(app.mode, TuiMode::Normal);
    }

    #[test]
    fn test_chat_selection() {
        let mut app = TuiApp::new(EventCollector::new()).unwrap();
        app.chat_ids = vec![Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
        app.chat_list_state.select(Some(0));

        app.next_chat();
        assert_eq!(app.chat_list_state.selected(), Some(1));
        app.next_chat();
        assert_eq!(app.chat_list_state.selected(), Some(2));
        app.next_chat();
        assert_eq!(app.chat_list_state.selected(), Some(0)); // Wraps around

        app.previous_chat();
        assert_eq!(app.chat_list_state.selected(), Some(2)); // Wraps around
        app.previous_chat();
        assert_eq!(app.chat_list_state.selected(), Some(1));
        app.previous_chat();
        assert_eq!(app.chat_list_state.selected(), Some(0));
    }

    #[test]
    fn test_chat_selection_empty() {
        let mut app = TuiApp::new(EventCollector::new()).unwrap();
        app.chat_ids = vec![];
        app.chat_list_state.select(None);

        app.next_chat();
        assert_eq!(app.chat_list_state.selected(), None);

        app.previous_chat();
        assert_eq!(app.chat_list_state.selected(), None);
    }

    #[test]
    fn test_scrolling() {
        let mut app = TuiApp::new(EventCollector::new()).unwrap();
        app.message_scroll = 5;

        app.scroll_up();
        assert_eq!(app.message_scroll, 4);
        app.scroll_up();
        assert_eq!(app.message_scroll, 3);

        app.scroll_down();
        assert_eq!(app.message_scroll, 4);

        // Test saturating sub
        app.message_scroll = 0;
        app.scroll_up();
        assert_eq!(app.message_scroll, 0);
    }

    #[test]
    fn test_send_message_clears_input() {
        let mut app = TuiApp::new(EventCollector::new()).unwrap();
        let (_chat_id, _rx) = setup_chat_with_session(&mut app, "Test Chat");
        app.input_text = "Hello, world!".to_string();

        app.send_message();

        assert_eq!(app.input_text, "");
    }

    #[test]
    fn test_input_text_append() {
        let mut app = TuiApp::new(EventCollector::new()).unwrap();
        app.input_text = "Hello".to_string();

        app.input_text.push('!');
        assert_eq!(app.input_text, "Hello!");

        app.input_text.push_str(" World");
        assert_eq!(app.input_text, "Hello! World");
    }

    #[test]
    fn test_input_text_delete() {
        let mut app = TuiApp::new(EventCollector::new()).unwrap();
        app.input_text = "Test".to_string();

        app.input_text.pop();
        assert_eq!(app.input_text, "Tes");

        app.input_text.pop();
        app.input_text.pop();
        app.input_text.pop();
        assert_eq!(app.input_text, "");

        // Pop empty string should be safe
        app.input_text.pop();
        assert_eq!(app.input_text, "");
    }

    #[test]
    fn test_send_empty_message_noop() {
        let mut app = TuiApp::new(EventCollector::new()).unwrap();
        let (chat_id, _rx) = setup_chat_with_session(&mut app, "Test");

        // 1. Empty string
        app.input_text = "".to_string();
        app.send_message();
        let chat = app.chat_manager.chats.get(&chat_id).unwrap();
        assert!(chat.messages.is_empty());

        // 2. Whitespace only
        app.input_text = "   ".to_string();
        app.send_message();
        let chat = app.chat_manager.chats.get(&chat_id).unwrap();
        assert!(chat.messages.is_empty());

        assert_eq!(app.input_text, "   ");
    }

    #[test]
    fn test_send_message_adds_to_history() {
        let mut app = TuiApp::new(EventCollector::new()).unwrap();
        let (chat_id, _rx) = setup_chat_with_session(&mut app, "Test");

        app.input_text = "Message 1".to_string();

        // Use a wrapper to check for result
        if let Some(selected_index) = app.chat_list_state.selected() {
            if let Some(cid) = app.chat_ids.get(selected_index) {
                let res = app.chat_manager.send_message(*cid, app.input_text.clone());
                assert!(res.is_ok(), "send_message failed: {:?}", res.err());
            } else {
                panic!("chat_id not found in app.chat_ids");
            }
        } else {
            panic!("no chat selected");
        }

        app.input_text.clear();

        let chat = app.chat_manager.chats.get(&chat_id).unwrap();
        assert_eq!(
            chat.messages.len(),
            1,
            "History length should be 1 after sending"
        );
        match &chat.messages[0].content {
            crate::types::MessageContent::Text { text } => assert_eq!(text, "Message 1"),
            _ => panic!("Wrong message content"),
        }

        // Send second message
        app.input_text = "Message 2".to_string();
        app.send_message();

        let chat = app.chat_manager.chats.get(&chat_id).unwrap();
        assert_eq!(chat.messages.len(), 2);
        match &chat.messages[1].content {
            crate::types::MessageContent::Text { text } => assert_eq!(text, "Message 2"),
            _ => panic!("Wrong message content"),
        }
    }

    #[test]
    fn test_send_to_nonexistent_chat() {
        let mut app = TuiApp::new(EventCollector::new()).unwrap();
        // Force state: empty chat_ids but selected index 0
        app.chat_ids = vec![];
        app.chat_list_state.select(Some(0));
        app.input_text = "Crash?".to_string();

        // Should not panic
        app.send_message();

        // Input should NOT be cleared because send failed
        assert_eq!(app.input_text, "Crash?");
    }

    #[test]
    fn test_scroll_overflow() {
        let mut app = TuiApp::new(EventCollector::new()).unwrap();
        app.message_scroll = u16::MAX;

        app.scroll_down();
        assert_eq!(app.message_scroll, u16::MAX);

        app.message_scroll = 0;
        app.scroll_up();
        assert_eq!(app.message_scroll, 0);
    }

    #[test]
    fn test_unicode_input() {
        let mut app = TuiApp::new(EventCollector::new()).unwrap();
        let (chat_id, _rx) = setup_chat_with_session(&mut app, "Unicode");

        let emoji_msg = "Hello 👋 🌍";
        app.input_text = emoji_msg.to_string();

        app.send_message();

        let chat = app.chat_manager.chats.get(&chat_id).unwrap();
        match &chat.messages[0].content {
            crate::types::MessageContent::Text { text } => assert_eq!(text, emoji_msg),
            _ => panic!("Wrong content"),
        }
    }

    #[test]
    fn test_very_long_input() {
        let mut app = TuiApp::new(EventCollector::new()).unwrap();
        let (chat_id, _rx) = setup_chat_with_session(&mut app, "Long");

        let long_msg = "a".repeat(1000);
        app.input_text = long_msg.clone();

        app.send_message();

        let chat = app.chat_manager.chats.get(&chat_id).unwrap();
        match &chat.messages[0].content {
            crate::types::MessageContent::Text { text } => assert_eq!(text, &long_msg),
            _ => panic!("Wrong content"),
        }
    }

    #[test]
    fn test_parse_host_command_with_port() {
        let parsed = TuiApp::parse_command(":host 7777").unwrap();
        assert_eq!(parsed, TuiCommand::Host(Some(7777)));
    }

    #[test]
    fn test_parse_connect_command_default_port() {
        let parsed = TuiApp::parse_command(":connect 10.0.0.1").unwrap();
        assert_eq!(
            parsed,
            TuiCommand::Connect {
                host: "10.0.0.1".to_string(),
                port: crate::PORT_DEFAULT,
            }
        );
    }

    #[test]
    fn test_parse_host_relay_command() {
        let parsed = TuiApp::parse_command(":host-relay relay.example.com:23456 token123").unwrap();
        assert_eq!(
            parsed,
            TuiCommand::HostRelay {
                relay: "relay.example.com:23456".to_string(),
                token: Some("token123".to_string()),
            }
        );
    }

    #[test]
    fn test_parse_connect_relay_command() {
        let parsed =
            TuiApp::parse_command(":connect-relay relay.example.com:23456 token123").unwrap();
        assert_eq!(
            parsed,
            TuiCommand::ConnectRelay {
                relay: "relay.example.com:23456".to_string(),
                token: "token123".to_string(),
            }
        );
    }

    #[test]
    fn test_parse_rename_requires_title() {
        assert!(TuiApp::parse_command(":rename").is_err());
    }

    #[test]
    fn test_parse_diagnostics_command() {
        let parsed = TuiApp::parse_command(":diagnostics").unwrap();
        assert_eq!(parsed, TuiCommand::Diagnostics);
    }

    #[test]
    fn test_enter_command_mode_from_empty_input() {
        let mut app = TuiApp::new(EventCollector::new()).unwrap();
        app.focus = TuiFocus::Input;
        app.input_text.clear();

        app.handle_key_event(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));

        assert_eq!(app.mode, TuiMode::Command);
    }

    #[test]
    fn test_ctrl_j_inserts_newline_in_input() {
        let mut app = TuiApp::new(EventCollector::new()).unwrap();
        app.focus = TuiFocus::Input;
        app.input_text = "hello".to_string();

        app.handle_key_event(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));

        assert_eq!(app.input_text, "hello\n");
    }

    #[tokio::test]
    async fn test_quit_command_sets_exit_flag() {
        let mut app = TuiApp::new(EventCollector::new()).unwrap();
        app.execute_command(TuiCommand::Quit).await;
        assert!(app.should_quit);
    }
}
