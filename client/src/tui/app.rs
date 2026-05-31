//! TUI application state and behavior.
//!
//! `TuiApp` owns the `ChatManager` directly (the loop is single-threaded, so no
//! `Arc<Mutex>` is needed) plus all view state: focus, mode, the editable input
//! and command fields, the active overlay, scroll/unread/typing state, and the
//! identity. Keys are routed by `handle_key_event`; commands run through
//! `execute_command`; `tick` performs per-frame housekeeping (session events,
//! autosave, auto-rehost, unread/typing bookkeeping).

use crate::app::chat_manager::ChatManager;
use crate::identity::Identity;
use crate::tui::command::{parse_command, parse_setting_bool, settings_keys, COMMANDS};
use crate::tui::input::EditableField;
use crate::tui::overlays::{PasswordMode, TuiOverlay};
use crate::types::{Config, Theme, ToastLevel};
use anyhow::Result;
use egui_tracing::tracing::EventCollector;
use ratatui::widgets::ListState;
use ratatui_crossterm::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use uuid::Uuid;

pub use crate::tui::command::TuiCommand;

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

const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(30);
const REHOST_DEBOUNCE: Duration = Duration::from_millis(1500);
const TYPING_IDLE: Duration = Duration::from_secs(4);

pub struct TuiApp {
    pub chat_manager: ChatManager,
    pub chat_list_state: ListState,
    pub chat_ids: Vec<Uuid>,
    pub input_field: EditableField,
    pub command_field: EditableField,
    pub password_field: EditableField,
    pub message_scroll: u16,
    pub stick_to_bottom: bool,
    pub identity_name: String,
    pub event_collector: EventCollector,
    pub focus: TuiFocus,
    pub mode: TuiMode,
    pub status_line: String,
    pub should_quit: bool,
    pub overlay: TuiOverlay,
    pub overlay_scroll: u16,
    pub contact_ids: Vec<Uuid>,
    pub contacts_list_state: ListState,
    pub settings_list_state: ListState,
    /// Render metrics captured by the message view, used for scroll clamping.
    pub msg_view_height: u16,
    pub msg_view_width: u16,
    identity: Identity,
    identity_path: PathBuf,
    history_path: PathBuf,
    pending_command: Option<TuiCommand>,
    /// Per-chat count of messages already seen (for unread detection).
    seen_counts: HashMap<Uuid, usize>,
    unread: HashSet<Uuid>,
    last_save: Option<Instant>,
    last_rehost: Option<Instant>,
    typing_active: bool,
    last_typing_activity: Option<Instant>,
    /// Indices into `command::COMMANDS` matching the command word being typed.
    pub command_suggestions: Vec<usize>,
    /// Highlighted entry within `command_suggestions`.
    pub suggestion_index: usize,
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
        let identity_path = history_path.with_file_name("identity.json");

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
            input_field: EditableField::new(true),
            command_field: EditableField::new(false),
            password_field: EditableField::new(false).masked(true),
            message_scroll: 0,
            stick_to_bottom: true,
            identity_name: identity.name.clone(),
            event_collector,
            focus: TuiFocus::Input,
            mode: TuiMode::Normal,
            status_line: String::new(),
            should_quit: false,
            overlay: TuiOverlay::None,
            overlay_scroll: 0,
            contact_ids: Vec::new(),
            contacts_list_state: ListState::default(),
            settings_list_state: ListState::default(),
            msg_view_height: 0,
            msg_view_width: 0,
            identity,
            identity_path,
            history_path,
            pending_command: None,
            seen_counts: HashMap::new(),
            unread: HashSet::new(),
            last_save: None,
            last_rehost: None,
            typing_active: false,
            last_typing_activity: None,
            command_suggestions: Vec::new(),
            suggestion_index: 0,
        };
        app.sync_chat_ids();
        app.refresh_status_line();
        Ok(app)
    }

    /// At startup, greet the user with a password overlay when appropriate:
    /// unlock a locked identity, or set a password on a new/unencrypted one.
    /// Called by `run()` (kept out of `new()` so tests start overlay-free).
    pub fn prompt_auth_if_needed(&mut self) {
        if self.identity.is_locked() {
            self.overlay = TuiOverlay::Password {
                mode: PasswordMode::Unlock,
            };
        } else if !self.identity.is_encrypted() {
            self.overlay = TuiOverlay::Password {
                mode: PasswordMode::Set,
            };
        }
    }

    /// Compatibility shim so external callers/tests keep using `TuiApp::parse_command`.
    pub fn parse_command(raw: &str) -> std::result::Result<TuiCommand, String> {
        parse_command(raw)
    }

    pub fn identity_fingerprint(&self) -> String {
        self.identity.fingerprint.clone()
    }

    pub fn is_unlocked(&self) -> bool {
        self.identity.private_key().is_ok()
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

    fn sync_contact_ids(&mut self) {
        let mut contacts: Vec<_> = self.chat_manager.contacts.values().collect();
        contacts.sort_by_key(|c| c.created_at);
        self.contact_ids = contacts.into_iter().map(|c| c.id).collect();
        if self.contact_ids.is_empty() {
            self.contacts_list_state.select(None);
        } else if self
            .contacts_list_state
            .selected()
            .is_none_or(|i| i >= self.contact_ids.len())
        {
            self.contacts_list_state.select(Some(0));
        }
    }

    pub fn tick(&mut self) {
        self.chat_manager.poll_session_events();
        self.chat_manager.cleanup_expired_toasts();

        // Surface a pending fingerprint verification as an overlay.
        if !self.overlay.is_open() {
            if let Some((fingerprint, peer_name, chat_id)) =
                self.chat_manager.fingerprint_verification_request.take()
            {
                self.overlay = TuiOverlay::FingerprintVerify {
                    fingerprint,
                    peer_name,
                    chat_id,
                };
            }
        }

        self.sync_chat_ids();
        self.update_unread();
        self.maybe_send_typing_stop();
        self.autosave();
        self.auto_rehost();
        self.refresh_status_line();
    }

    fn update_unread(&mut self) {
        let selected = self.selected_chat_id();
        for chat in self.chat_manager.chats.values() {
            let count = chat.messages.len();
            let seen = self.seen_counts.get(&chat.id).copied().unwrap_or(0);
            if Some(chat.id) == selected {
                self.seen_counts.insert(chat.id, count);
                self.unread.remove(&chat.id);
            } else if count > seen {
                self.unread.insert(chat.id);
            }
        }
        self.seen_counts
            .retain(|id, _| self.chat_manager.chats.contains_key(id));
        self.unread
            .retain(|id| self.chat_manager.chats.contains_key(id));
    }

    pub fn is_unread(&self, chat_id: &Uuid) -> bool {
        self.unread.contains(chat_id)
    }

    fn autosave(&mut self) {
        if self.chat_manager.history_key.is_none() || self.chat_manager.chats.is_empty() {
            return;
        }
        let due = self
            .last_save
            .map(|t| t.elapsed() >= AUTOSAVE_INTERVAL)
            .unwrap_or(true);
        if due {
            self.save_history_now();
            self.last_save = Some(Instant::now());
        }
    }

    fn save_history_now(&mut self) {
        if self.chat_manager.history_key.is_none() {
            return;
        }
        if let Err(e) = self.chat_manager.save_history(&self.history_path) {
            tracing::warn!("TUI history save failed: {}", e);
        }
    }

    fn auto_rehost(&mut self) {
        if !self.chat_manager.config.auto_host_on_startup {
            return;
        }
        if !self.chat_manager.check_rehost_needed() {
            return;
        }
        let due = self
            .last_rehost
            .map(|t| t.elapsed() >= REHOST_DEBOUNCE)
            .unwrap_or(true);
        if !due {
            return;
        }
        if !self.is_unlocked() {
            return;
        }
        self.last_rehost = Some(Instant::now());
        let port = self.chat_manager.config.listen_port;
        self.pending_command = Some(TuiCommand::Host(Some(port)));
        tracing::info!(port, "Auto-rehost scheduled");
    }

    fn refresh_status_line(&mut self) {
        if self.identity.is_locked() {
            self.status_line = "🔒 Identity locked — :unlock <password> to continue".to_string();
            return;
        }

        let sessions = self.chat_manager.sessions_len();
        let chat_label = match self.selected_chat_id() {
            Some(id) => {
                let title = self
                    .chat_manager
                    .get_chat(id)
                    .map(|c| c.title.clone())
                    .unwrap_or_else(|| "—".to_string());
                let state = if self.chat_manager.is_connected(&id) {
                    "connected"
                } else if self
                    .chat_manager
                    .get_chat(id)
                    .map(|c| c.is_host_placeholder)
                    .unwrap_or(false)
                {
                    "hosting"
                } else {
                    "offline"
                };
                format!("{} · {}", title, state)
            }
            None => "no chat selected".to_string(),
        };

        let hint = match self.mode {
            TuiMode::Command => {
                if self.command_suggestions.is_empty() {
                    "Enter run · Esc cancel · ↑/↓ history".to_string()
                } else {
                    "Tab complete · ↑/↓ pick · Enter run · Esc cancel".to_string()
                }
            }
            TuiMode::Normal => match self.focus {
                TuiFocus::Input => "Enter send · Ctrl+J newline · : commands".to_string(),
                _ => "Tab focus · : commands · :help".to_string(),
            },
        };

        self.status_line = format!("{}  │  sessions:{}  │  {}", chat_label, sessions, hint);
    }

    pub fn take_pending_command(&mut self) -> Option<TuiCommand> {
        self.pending_command.take()
    }

    pub fn enter_command_mode(&mut self) {
        self.mode = TuiMode::Command;
        self.command_field.clear();
        self.update_command_suggestions();
    }

    /// Recompute the command autocomplete matches from the current buffer.
    ///
    /// Suggestions appear only while typing the first word (the command name)
    /// and only after at least one character, so pressing ↑ on an empty `:`
    /// prompt still recalls history.
    fn update_command_suggestions(&mut self) {
        let text = self.command_field.text();
        let token = text.trim_start();
        if token.is_empty() || token.contains(char::is_whitespace) {
            self.command_suggestions.clear();
            self.suggestion_index = 0;
            return;
        }
        let prefix = token.to_ascii_lowercase();
        self.command_suggestions = COMMANDS
            .iter()
            .enumerate()
            .filter(|(_, (name, _, _))| name.starts_with(&prefix))
            .map(|(i, _)| i)
            .collect();
        if self.suggestion_index >= self.command_suggestions.len() {
            self.suggestion_index = 0;
        }
    }

    /// Replace the command word with the highlighted suggestion (+trailing space).
    fn accept_suggestion(&mut self) {
        if let Some(&cmd_idx) = self.command_suggestions.get(self.suggestion_index) {
            let name = COMMANDS[cmd_idx].0;
            self.command_field.set_text(&format!("{} ", name));
            self.update_command_suggestions();
        }
    }

    pub fn cancel_command_mode(&mut self) {
        self.mode = TuiMode::Normal;
        self.command_field.clear();
        self.command_suggestions.clear();
        self.suggestion_index = 0;
    }

    pub fn submit_command(&mut self) {
        let raw = self.command_field.text();
        self.command_field.push_history(&raw);
        match parse_command(&raw) {
            Ok(cmd) => self.pending_command = Some(cmd),
            Err(e) => self.toast(ToastLevel::Error, e),
        }
        self.mode = TuiMode::Normal;
        self.command_field.clear();
        self.command_suggestions.clear();
        self.suggestion_index = 0;
    }

    pub fn cycle_focus(&mut self) {
        self.focus = match self.focus {
            TuiFocus::ChatList => TuiFocus::MessageView,
            TuiFocus::MessageView => TuiFocus::Input,
            TuiFocus::Input => TuiFocus::ChatList,
        };
    }

    fn toast(&mut self, level: ToastLevel, msg: String) {
        self.chat_manager.add_toast(level, msg);
    }

    // ---------------------------------------------------------------- keys

    pub fn handle_key_event(&mut self, key: KeyEvent) {
        if self.overlay.is_open() {
            self.handle_overlay_key(key);
            return;
        }
        if self.mode == TuiMode::Command {
            self.handle_command_key(key);
            return;
        }
        self.handle_normal_key(key);
    }

    fn handle_command_key(&mut self, key: KeyEvent) {
        let has_suggestions = !self.command_suggestions.is_empty();
        match key.code {
            KeyCode::Esc => self.cancel_command_mode(),
            KeyCode::Enter => self.submit_command(),
            KeyCode::Tab => {
                if has_suggestions {
                    self.accept_suggestion();
                }
            }
            // When the suggestion menu is showing, ↑/↓ move the highlight;
            // otherwise they recall command history.
            KeyCode::Up => {
                if has_suggestions {
                    let n = self.command_suggestions.len();
                    self.suggestion_index = (self.suggestion_index + n - 1) % n;
                } else {
                    self.command_field.history_prev();
                    self.update_command_suggestions();
                }
            }
            KeyCode::Down => {
                if has_suggestions {
                    let n = self.command_suggestions.len();
                    self.suggestion_index = (self.suggestion_index + 1) % n;
                } else {
                    self.command_field.history_next();
                    self.update_command_suggestions();
                }
            }
            _ => {
                if self.command_field.handle_edit_key(&key) {
                    self.update_command_suggestions();
                }
            }
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        if ctrl && key.code == KeyCode::Char('l') {
            self.copy_logs();
            return;
        }
        if key.code == KeyCode::Tab {
            self.cycle_focus();
            return;
        }
        if ctrl && key.code == KeyCode::Char('j') {
            if self.focus == TuiFocus::Input {
                self.input_field.newline();
                self.note_input_activity();
            }
            return;
        }

        if self.focus == TuiFocus::Input {
            match key.code {
                KeyCode::Char(':') if self.input_field.is_empty() => self.enter_command_mode(),
                KeyCode::Enter => self.send_message(),
                KeyCode::Esc => self.focus = TuiFocus::ChatList,
                _ => {
                    if self.input_field.handle_edit_key(&key) {
                        self.note_input_activity();
                    }
                }
            }
            return;
        }

        // ChatList / MessageView focus.
        match key.code {
            KeyCode::Char(':') => self.enter_command_mode(),
            KeyCode::Char('q') => self.overlay = TuiOverlay::ConfirmQuit,
            KeyCode::Char('?') => {
                self.overlay = TuiOverlay::Help;
                self.overlay_scroll = 0;
            }
            KeyCode::Esc => self.focus = TuiFocus::ChatList,
            KeyCode::Down => match self.focus {
                TuiFocus::ChatList => self.next_chat(),
                _ => self.scroll_down(),
            },
            KeyCode::Up => match self.focus {
                TuiFocus::ChatList => self.previous_chat(),
                _ => self.scroll_up(),
            },
            KeyCode::PageDown => self.scroll_down(),
            KeyCode::PageUp => self.scroll_up(),
            _ => {}
        }
    }

    fn handle_overlay_key(&mut self, key: KeyEvent) {
        match self.overlay.clone() {
            TuiOverlay::Password { mode } => self.handle_password_key(key, mode),
            TuiOverlay::FingerprintVerify {
                chat_id,
                fingerprint,
                ..
            } => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    self.resolve_fingerprint(chat_id, &fingerprint, true)
                }
                KeyCode::Char('n') | KeyCode::Esc => {
                    self.resolve_fingerprint(chat_id, &fingerprint, false)
                }
                _ => {}
            },
            TuiOverlay::ConfirmQuit => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    self.save_history_now();
                    self.should_quit = true;
                }
                _ => self.overlay = TuiOverlay::None,
            },
            TuiOverlay::ConfirmClearHistory => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    let path = self.history_path.clone();
                    self.chat_manager.clear_history(&path);
                    self.toast(ToastLevel::Info, "History cleared".to_string());
                    self.overlay = TuiOverlay::None;
                }
                _ => self.overlay = TuiOverlay::None,
            },
            TuiOverlay::Contacts => match key.code {
                KeyCode::Esc => self.overlay = TuiOverlay::None,
                KeyCode::Down => self.contacts_select_next(),
                KeyCode::Up => self.contacts_select_prev(),
                KeyCode::Enter => self.connect_selected_contact(),
                _ => {}
            },
            TuiOverlay::Settings => match key.code {
                KeyCode::Esc => self.overlay = TuiOverlay::None,
                KeyCode::Down => self.settings_select(1),
                KeyCode::Up => self.settings_select(-1),
                KeyCode::Enter | KeyCode::Char(' ') => self.toggle_selected_setting(),
                _ => {}
            },
            TuiOverlay::Help => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => self.overlay = TuiOverlay::None,
                KeyCode::Down => self.overlay_scroll = self.overlay_scroll.saturating_add(1),
                KeyCode::Up => self.overlay_scroll = self.overlay_scroll.saturating_sub(1),
                KeyCode::PageDown => self.overlay_scroll = self.overlay_scroll.saturating_add(10),
                KeyCode::PageUp => self.overlay_scroll = self.overlay_scroll.saturating_sub(10),
                _ => {}
            },
            // Plain informational overlays: any of Esc/Enter/q closes.
            TuiOverlay::Invite { .. } | TuiOverlay::Identity | TuiOverlay::Transfers => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                    self.overlay = TuiOverlay::None;
                }
            }
            TuiOverlay::None => {}
        }
    }

    fn handle_password_key(&mut self, key: KeyEvent, mode: PasswordMode) {
        match key.code {
            KeyCode::Esc => {
                // Allow dismissing the "set password" prompt; unlocking is required
                // for a locked identity, so closing it just leaves the app locked.
                self.password_field.clear();
                self.overlay = TuiOverlay::None;
            }
            KeyCode::Enter => {
                let pw = self.password_field.text();
                self.password_field.clear();
                match mode {
                    PasswordMode::Unlock => self.try_unlock(&pw),
                    PasswordMode::Set => self.try_set_password(&pw),
                }
            }
            _ => {
                self.password_field.handle_edit_key(&key);
            }
        }
    }

    // ---------------------------------------------------------------- scrolling

    pub fn next_chat(&mut self) {
        if self.chat_ids.is_empty() {
            return;
        }
        let i = match self.chat_list_state.selected() {
            Some(i) if i + 1 < self.chat_ids.len() => i + 1,
            Some(_) => 0,
            None => 0,
        };
        self.chat_list_state.select(Some(i));
        self.message_scroll = 0;
        self.stick_to_bottom = true;
    }

    pub fn previous_chat(&mut self) {
        if self.chat_ids.is_empty() {
            return;
        }
        let i = match self.chat_list_state.selected() {
            Some(0) => self.chat_ids.len() - 1,
            Some(i) => i - 1,
            None => 0,
        };
        self.chat_list_state.select(Some(i));
        self.message_scroll = 0;
        self.stick_to_bottom = true;
    }

    pub fn scroll_up(&mut self) {
        self.message_scroll = self.message_scroll.saturating_sub(1);
        self.stick_to_bottom = false;
    }

    pub fn scroll_down(&mut self) {
        self.message_scroll = self.message_scroll.saturating_add(1);
        // Re-stick is decided during render once the content height is known.
    }

    // ---------------------------------------------------------------- typing

    fn note_input_activity(&mut self) {
        self.last_typing_activity = Some(Instant::now());
        if self.input_field.is_empty() {
            self.stop_typing();
            return;
        }
        if !self.typing_active {
            if let Some(chat_id) = self.selected_chat_id() {
                let _ = self.chat_manager.send_typing_start(chat_id);
                self.typing_active = true;
            }
        }
    }

    fn stop_typing(&mut self) {
        if self.typing_active {
            if let Some(chat_id) = self.selected_chat_id() {
                let _ = self.chat_manager.send_typing_stop(chat_id);
            }
            self.typing_active = false;
        }
    }

    fn maybe_send_typing_stop(&mut self) {
        if self.typing_active {
            let idle = self
                .last_typing_activity
                .map(|t| t.elapsed() >= TYPING_IDLE)
                .unwrap_or(true);
            if idle {
                self.stop_typing();
            }
        }
    }

    pub fn send_message(&mut self) {
        let Some(chat_id) = self.selected_chat_id() else {
            return;
        };
        let text = self.input_field.text();
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        match self.chat_manager.send_message(chat_id, trimmed) {
            Ok(()) => {
                self.input_field.clear();
                self.stop_typing();
                self.stick_to_bottom = true;
            }
            Err(e) => {
                tracing::error!("Failed to send message: {}", e);
                self.toast(ToastLevel::Error, format!("Send failed: {}", e));
            }
        }
    }

    // ---------------------------------------------------------------- commands

    fn require_unlocked(&mut self) -> bool {
        if self.is_unlocked() {
            true
        } else {
            self.toast(
                ToastLevel::Warning,
                "Identity is locked. Unlock with :unlock <password>.".to_string(),
            );
            self.overlay = TuiOverlay::Password {
                mode: PasswordMode::Unlock,
            };
            false
        }
    }

    fn private_key(&mut self) -> Option<rsa::RsaPrivateKey> {
        match self.identity.private_key() {
            Ok(k) => Some(k),
            Err(e) => {
                self.toast(ToastLevel::Error, format!("Cannot access key: {}", e));
                None
            }
        }
    }

    fn select_chat(&mut self, chat_id: Uuid) {
        self.sync_chat_ids();
        if let Some(idx) = self.chat_ids.iter().position(|id| *id == chat_id) {
            self.chat_list_state.select(Some(idx));
        }
        self.stick_to_bottom = true;
        self.message_scroll = 0;
    }

    /// Resolve a contact target given as a 1-based index or a (case-insensitive) name.
    fn resolve_contact(&self, target: &str) -> Option<Uuid> {
        if let Ok(idx) = target.parse::<usize>() {
            if idx >= 1 {
                return self.contact_ids.get(idx - 1).copied();
            }
        }
        let lower = target.to_ascii_lowercase();
        self.chat_manager
            .contacts
            .values()
            .find(|c| c.name.to_ascii_lowercase() == lower)
            .map(|c| c.id)
    }

    pub async fn execute_command(&mut self, cmd: TuiCommand) {
        match cmd {
            TuiCommand::Host(port_override) => {
                if !self.require_unlocked() {
                    return;
                }
                let port = port_override.unwrap_or(self.chat_manager.config.listen_port);
                let Some(privkey) = self.private_key() else {
                    return;
                };
                match self.chat_manager.start_host(port, privkey).await {
                    Ok(_) => self.toast(ToastLevel::Success, format!("Hosting on :{}", port)),
                    Err(e) => self.toast(ToastLevel::Error, format!("Failed to host: {}", e)),
                }
            }
            TuiCommand::Connect { host, port } => {
                if !self.require_unlocked() {
                    return;
                }
                let Some(privkey) = self.private_key() else {
                    return;
                };
                match self
                    .chat_manager
                    .connect_to_host(&host, port, None, privkey)
                    .await
                {
                    Ok(chat_id) => {
                        self.toast(
                            ToastLevel::Info,
                            format!("Connecting to {}:{}…", host, port),
                        );
                        self.select_chat(chat_id);
                    }
                    Err(e) => self.toast(ToastLevel::Error, format!("Connect failed: {}", e)),
                }
            }
            TuiCommand::HostRelay { relay, token } => {
                if !self.require_unlocked() {
                    return;
                }
                let Some(privkey) = self.private_key() else {
                    return;
                };
                match self
                    .chat_manager
                    .start_host_via_relay(&relay, token, privkey)
                    .await
                {
                    Ok((chat_id, relay_token)) => {
                        if let Ok(link) = self.identity.generate_signed_invite_link_with_route(
                            None,
                            Some(relay.clone()),
                            Some(relay_token.clone()),
                        ) {
                            Self::copy_to_clipboard(&link);
                            self.overlay = TuiOverlay::Invite { link };
                        }
                        self.toast(
                            ToastLevel::Success,
                            format!("Relay host ready (token {})", relay_token),
                        );
                        self.select_chat(chat_id);
                    }
                    Err(e) => self.toast(ToastLevel::Error, format!("Relay host failed: {}", e)),
                }
            }
            TuiCommand::ConnectRelay { relay, token } => {
                if !self.require_unlocked() {
                    return;
                }
                let Some(privkey) = self.private_key() else {
                    return;
                };
                match self
                    .chat_manager
                    .connect_via_relay(&relay, &token, None, privkey)
                    .await
                {
                    Ok(chat_id) => {
                        self.toast(ToastLevel::Info, format!("Connecting via relay {}…", relay));
                        self.select_chat(chat_id);
                    }
                    Err(e) => self.toast(ToastLevel::Error, format!("Relay connect failed: {}", e)),
                }
            }
            TuiCommand::Disconnect => self.disconnect_selected(),
            TuiCommand::StopHost => {
                self.chat_manager.stop_hosting();
            }

            TuiCommand::Contacts => {
                self.sync_contact_ids();
                self.overlay = TuiOverlay::Contacts;
            }
            TuiCommand::ContactAdd {
                name,
                address,
                fingerprint,
            } => {
                if let Err(e) = ChatManager::parse_address(&address) {
                    self.toast(ToastLevel::Error, format!("Bad address: {}", e));
                    return;
                }
                self.chat_manager
                    .add_contact(name.clone(), Some(address), fingerprint, None);
                self.toast(ToastLevel::Success, format!("Contact '{}' added", name));
                self.save_history_now();
            }
            TuiCommand::ContactConnect(target) => {
                if !self.require_unlocked() {
                    return;
                }
                self.sync_contact_ids();
                let Some(contact_id) = self.resolve_contact(&target) else {
                    self.toast(ToastLevel::Error, format!("No contact '{}'", target));
                    return;
                };
                let Some(privkey) = self.private_key() else {
                    return;
                };
                match self
                    .chat_manager
                    .connect_to_contact(contact_id, None, &privkey)
                    .await
                {
                    Ok(chat_id) => {
                        self.toast(ToastLevel::Info, "Connecting to contact…".to_string());
                        self.overlay = TuiOverlay::None;
                        self.select_chat(chat_id);
                    }
                    Err(e) => self.toast(ToastLevel::Error, format!("Connect failed: {}", e)),
                }
            }
            TuiCommand::ContactRemove(target) => {
                self.sync_contact_ids();
                match self.resolve_contact(&target) {
                    Some(id) => {
                        self.chat_manager.remove_contact(id);
                        self.sync_contact_ids();
                        self.toast(ToastLevel::Info, "Contact removed".to_string());
                        self.save_history_now();
                    }
                    None => self.toast(ToastLevel::Error, format!("No contact '{}'", target)),
                }
            }
            TuiCommand::ContactRename { target, new_name } => {
                self.sync_contact_ids();
                match self.resolve_contact(&target) {
                    Some(id) => {
                        if let Some(c) = self.chat_manager.contacts.get_mut(&id) {
                            c.name = new_name.chars().take(50).collect();
                        }
                        self.toast(ToastLevel::Success, "Contact renamed".to_string());
                        self.save_history_now();
                    }
                    None => self.toast(ToastLevel::Error, format!("No contact '{}'", target)),
                }
            }

            TuiCommand::Invite(addr) => match self.identity.generate_signed_invite_link(addr) {
                Ok(link) => {
                    Self::copy_to_clipboard(&link);
                    self.overlay = TuiOverlay::Invite { link };
                    self.toast(ToastLevel::Success, "Invite link copied".to_string());
                }
                Err(e) => self.toast(ToastLevel::Error, format!("Invite failed: {}", e)),
            },
            TuiCommand::InviteRelay(relay) => {
                self.pending_command = Some(TuiCommand::HostRelay { relay, token: None });
            }
            TuiCommand::Import(link) => match self.chat_manager.parse_invite_link(&link) {
                Ok(contact) => {
                    let name = contact.name.clone();
                    self.chat_manager.import_contact(contact);
                    self.sync_contact_ids();
                    self.toast(
                        ToastLevel::Success,
                        format!("Imported '{}'. Use :contact-connect {}", name, name),
                    );
                    self.save_history_now();
                }
                Err(e) => self.toast(ToastLevel::Error, format!("Import failed: {}", e)),
            },

            TuiCommand::Send(path) => {
                let Some(chat_id) = self.selected_chat_id() else {
                    self.toast(ToastLevel::Warning, "No chat selected".to_string());
                    return;
                };
                match self
                    .chat_manager
                    .send_file(chat_id, PathBuf::from(path))
                    .await
                {
                    Ok(()) => {}
                    Err(e) => self.toast(ToastLevel::Error, format!("Send file failed: {}", e)),
                }
            }
            TuiCommand::Transfers => self.overlay = TuiOverlay::Transfers,

            TuiCommand::Rename(title) => match self.selected_chat_id() {
                Some(id) => {
                    if let Err(e) = self.chat_manager.rename_chat(id, title) {
                        self.toast(ToastLevel::Error, format!("Rename failed: {}", e));
                    } else {
                        self.toast(ToastLevel::Success, "Chat renamed".to_string());
                        self.save_history_now();
                    }
                }
                None => self.toast(ToastLevel::Warning, "No chat selected".to_string()),
            },
            TuiCommand::DeleteChat => self.disconnect_selected(),
            TuiCommand::ClearHistory => self.overlay = TuiOverlay::ConfirmClearHistory,

            TuiCommand::Identity => {
                Self::copy_to_clipboard(&self.identity.fingerprint);
                self.overlay = TuiOverlay::Identity;
            }
            TuiCommand::Verify(accept) => self.verify_pending(accept),
            TuiCommand::Unlock(pw) => match pw {
                Some(pw) => self.try_unlock(&pw),
                None => {
                    self.overlay = TuiOverlay::Password {
                        mode: PasswordMode::Unlock,
                    }
                }
            },
            TuiCommand::SetPassword(pw) => self.try_set_password(&pw),

            TuiCommand::Settings => {
                self.settings_list_state.select(Some(0));
                self.overlay = TuiOverlay::Settings;
            }
            TuiCommand::Set { key, value } => self.apply_setting(&key, &value),

            TuiCommand::Diagnostics => self.export_diagnostics_bundle(),
            TuiCommand::Logs => self.copy_logs(),
            TuiCommand::Help(_) => {
                self.overlay = TuiOverlay::Help;
                self.overlay_scroll = 0;
            }
            TuiCommand::Quit => self.overlay = TuiOverlay::ConfirmQuit,
            TuiCommand::ForceQuit => self.should_quit = true,
        }
        self.refresh_status_line();
    }

    fn disconnect_selected(&mut self) {
        match self.selected_chat_id() {
            Some(chat_id) => {
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
                self.save_history_now();
            }
            None => self.toast(ToastLevel::Warning, "No chat selected".to_string()),
        }
    }

    // ---------------------------------------------------------------- fingerprint

    fn verify_pending(&mut self, accept: bool) {
        if let TuiOverlay::FingerprintVerify {
            chat_id,
            fingerprint,
            ..
        } = self.overlay.clone()
        {
            self.resolve_fingerprint(chat_id, &fingerprint, accept);
        } else {
            self.toast(
                ToastLevel::Info,
                "No fingerprint verification pending".to_string(),
            );
        }
    }

    fn resolve_fingerprint(&mut self, chat_id: Uuid, fingerprint: &str, accept: bool) {
        if accept {
            let _ = self.chat_manager.confirm_fingerprint(chat_id, true);
            if let Some(chat) = self.chat_manager.chats.get_mut(&chat_id) {
                chat.peer_fingerprint = Some(fingerprint.to_string());
            }
            self.toast(ToastLevel::Success, "Fingerprint accepted".to_string());
        } else {
            let _ = self.chat_manager.confirm_fingerprint(chat_id, false);
            self.chat_manager.delete_chat(chat_id);
            self.toast(ToastLevel::Warning, "Fingerprint rejected".to_string());
        }
        self.overlay = TuiOverlay::None;
        self.save_history_now();
    }

    // ---------------------------------------------------------------- password

    fn try_unlock(&mut self, password: &str) {
        match self.identity.decrypt(password) {
            Ok(()) => {
                if let Ok(key) = self.identity.history_key() {
                    self.chat_manager.set_history_key(key);
                    let _ = self
                        .chat_manager
                        .load_history_auto(&self.history_path, &key);
                }
                self.overlay = TuiOverlay::None;
                self.toast(ToastLevel::Success, "Identity unlocked".to_string());
            }
            Err(_) => self.toast(ToastLevel::Error, "Wrong password".to_string()),
        }
    }

    fn try_set_password(&mut self, password: &str) {
        if password.len() < 6 {
            self.toast(
                ToastLevel::Error,
                "Password must be at least 6 characters".to_string(),
            );
            return;
        }
        // Migrate a plaintext key, or rotate an existing password.
        let result = if self.identity.is_encrypted() {
            // Rotate: decrypt is needed first; if locked we cannot rotate here.
            if self.identity.is_locked() {
                Err(anyhow::anyhow!("Unlock before changing the password"))
            } else {
                self.identity.encrypt(password)
            }
        } else {
            self.identity.migrate_legacy_plaintext(password)
        };

        match result.and_then(|()| self.identity.save(&self.identity_path)) {
            Ok(()) => {
                // Re-derive key requires the plaintext; encrypt() cleared it, so reload.
                if let Ok(mut reloaded) = Identity::load(&self.identity_path) {
                    if reloaded.decrypt(password).is_ok() {
                        if let Ok(key) = reloaded.history_key() {
                            self.chat_manager.set_history_key(key);
                            let _ = self
                                .chat_manager
                                .load_history_auto(&self.history_path, &key);
                        }
                        self.identity = reloaded;
                    }
                }
                self.overlay = TuiOverlay::None;
                self.toast(ToastLevel::Success, "Password set".to_string());
            }
            Err(e) => self.toast(ToastLevel::Error, format!("Failed to set password: {}", e)),
        }
    }

    // ---------------------------------------------------------------- contacts overlay

    fn contacts_select_next(&mut self) {
        if self.contact_ids.is_empty() {
            return;
        }
        let i = self
            .contacts_list_state
            .selected()
            .map(|i| (i + 1) % self.contact_ids.len())
            .unwrap_or(0);
        self.contacts_list_state.select(Some(i));
    }

    fn contacts_select_prev(&mut self) {
        if self.contact_ids.is_empty() {
            return;
        }
        let i = match self.contacts_list_state.selected() {
            Some(0) | None => self.contact_ids.len() - 1,
            Some(i) => i - 1,
        };
        self.contacts_list_state.select(Some(i));
    }

    fn connect_selected_contact(&mut self) {
        if let Some(idx) = self.contacts_list_state.selected() {
            if let Some(&id) = self.contact_ids.get(idx) {
                // Resolve to a 1-based index string for the shared command path.
                self.pending_command = Some(TuiCommand::ContactConnect((idx + 1).to_string()));
                let _ = id;
            }
        }
    }

    // ---------------------------------------------------------------- settings overlay

    fn settings_select(&mut self, delta: i32) {
        let len = 8i32; // number of settings rows rendered
        let cur = self.settings_list_state.selected().unwrap_or(0) as i32;
        let next = (cur + delta).rem_euclid(len) as usize;
        self.settings_list_state.select(Some(next));
    }

    fn toggle_selected_setting(&mut self) {
        let idx = self.settings_list_state.selected().unwrap_or(0);
        let cfg = &mut self.chat_manager.config;
        match idx {
            // 0 download-dir, 1 listen-port are text-only (use :set)
            2 => cfg.enable_notifications = !cfg.enable_notifications,
            3 => cfg.enable_typing_indicators = !cfg.enable_typing_indicators,
            4 => cfg.auto_accept_files = !cfg.auto_accept_files,
            5 => cfg.auto_host_on_startup = !cfg.auto_host_on_startup,
            6 => cfg.enable_mdns = !cfg.enable_mdns,
            7 => {
                cfg.theme = match cfg.theme {
                    Theme::Light => Theme::Dark,
                    Theme::Dark => Theme::Midnight,
                    Theme::Midnight => Theme::Forest,
                    Theme::Forest => Theme::Light,
                }
            }
            _ => {
                self.toast(
                    ToastLevel::Info,
                    "Use :set download-dir <path> / :set listen-port <n>".to_string(),
                );
                return;
            }
        }
        self.save_history_now();
    }

    fn apply_setting(&mut self, key: &str, value: &str) {
        let cfg = &mut self.chat_manager.config;
        let result: std::result::Result<String, String> = match key {
            "download-dir" => {
                cfg.download_dir = PathBuf::from(value);
                Ok(format!("download-dir = {}", value))
            }
            "listen-port" => match value.parse::<u16>() {
                Ok(p) => {
                    cfg.listen_port = p;
                    Ok(format!("listen-port = {}", p))
                }
                Err(_) => Err("listen-port must be a number".to_string()),
            },
            "notifications" => parse_setting_bool(value).map(|b| {
                cfg.enable_notifications = b;
                format!("notifications = {}", b)
            }),
            "typing" => parse_setting_bool(value).map(|b| {
                cfg.enable_typing_indicators = b;
                format!("typing = {}", b)
            }),
            "auto-accept" => parse_setting_bool(value).map(|b| {
                cfg.auto_accept_files = b;
                format!("auto-accept = {}", b)
            }),
            "auto-host" => parse_setting_bool(value).map(|b| {
                cfg.auto_host_on_startup = b;
                format!("auto-host = {}", b)
            }),
            "mdns" => parse_setting_bool(value).map(|b| {
                cfg.enable_mdns = b;
                format!("mdns = {}", b)
            }),
            "theme" => match value.to_ascii_lowercase().as_str() {
                "light" => {
                    cfg.theme = Theme::Light;
                    Ok("theme = Light".to_string())
                }
                "dark" => {
                    cfg.theme = Theme::Dark;
                    Ok("theme = Dark".to_string())
                }
                "midnight" => {
                    cfg.theme = Theme::Midnight;
                    Ok("theme = Midnight".to_string())
                }
                "forest" => {
                    cfg.theme = Theme::Forest;
                    Ok("theme = Forest".to_string())
                }
                _ => Err("theme: light|dark|midnight|forest".to_string()),
            },
            _ => Err(format!(
                "Unknown setting '{}'. Keys: {}",
                key,
                settings_keys().join(", ")
            )),
        };
        match result {
            Ok(msg) => {
                self.toast(ToastLevel::Success, msg);
                self.save_history_now();
            }
            Err(e) => self.toast(ToastLevel::Error, e),
        }
    }

    // ---------------------------------------------------------------- misc

    fn copy_to_clipboard(text: &str) {
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            let _ = clipboard.set_text(text.to_string());
        }
    }

    pub fn copy_logs(&mut self) {
        let log_text = crate::support::format_event_logs(&self.event_collector);
        match arboard::Clipboard::new() {
            Ok(mut clipboard) => match clipboard.set_text(log_text) {
                Ok(()) => self.toast(ToastLevel::Success, "Logs copied to clipboard".to_string()),
                Err(e) => self.toast(ToastLevel::Error, format!("Failed to copy logs: {}", e)),
            },
            Err(e) => self.toast(ToastLevel::Error, format!("Clipboard error: {}", e)),
        }
        self.refresh_status_line();
    }

    pub fn export_diagnostics_bundle(&mut self) {
        let report = crate::support::DiagnosticsReport {
            generated_at_utc: chrono::Utc::now().to_rfc3339(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            history_path: self.history_path.display().to_string(),
            identity_path: self.identity_path.display().to_string(),
            history_exists: self.history_path.exists(),
            identity_exists: self.identity_path.exists(),
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
            Ok(bundle_dir) => self.toast(
                ToastLevel::Success,
                format!("Diagnostics exported to {}", bundle_dir.display()),
            ),
            Err(e) => self.toast(ToastLevel::Error, format!("Diagnostics failed: {}", e)),
        }
        self.refresh_status_line();
    }

    /// Save history one last time (called on graceful shutdown).
    pub fn shutdown_save(&mut self) {
        self.save_history_now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::overlays::TuiOverlay;
    use ratatui_crossterm::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tokio::sync::mpsc;

    fn app() -> TuiApp {
        TuiApp::new(EventCollector::new()).unwrap()
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn fingerprint_accept_confirms_and_stores() {
        let mut app = app();
        let chat_id = Uuid::new_v4();
        app.chat_manager
            .create_local_chat_for_test(chat_id, "Peer".into());
        let (tx, mut rx) = mpsc::unbounded_channel();
        app.chat_manager
            .add_fingerprint_confirm_sender_for_test(chat_id, tx);
        app.overlay = TuiOverlay::FingerprintVerify {
            fingerprint: "deadbeef".into(),
            peer_name: "Peer".into(),
            chat_id,
        };

        // Pressing 'y' accepts.
        app.handle_key_event(key('y'));

        assert!(rx.try_recv().unwrap());
        assert_eq!(app.overlay, TuiOverlay::None);
        assert_eq!(
            app.chat_manager.get_chat(chat_id).unwrap().peer_fingerprint,
            Some("deadbeef".to_string())
        );
    }

    #[test]
    fn fingerprint_reject_declines_and_removes_chat() {
        let mut app = app();
        let chat_id = Uuid::new_v4();
        app.chat_manager
            .create_local_chat_for_test(chat_id, "Peer".into());
        let (tx, mut rx) = mpsc::unbounded_channel();
        app.chat_manager
            .add_fingerprint_confirm_sender_for_test(chat_id, tx);
        app.overlay = TuiOverlay::FingerprintVerify {
            fingerprint: "deadbeef".into(),
            peer_name: "Peer".into(),
            chat_id,
        };

        app.handle_key_event(key('n'));

        assert!(!rx.try_recv().unwrap());
        assert_eq!(app.overlay, TuiOverlay::None);
        assert!(app.chat_manager.get_chat(chat_id).is_none());
    }

    #[tokio::test]
    async fn set_command_toggles_config() {
        let mut app = app();
        assert!(app.chat_manager.config.enable_notifications);
        app.execute_command(TuiCommand::Set {
            key: "notifications".into(),
            value: "off".into(),
        })
        .await;
        assert!(!app.chat_manager.config.enable_notifications);

        app.execute_command(TuiCommand::Set {
            key: "listen-port".into(),
            value: "4242".into(),
        })
        .await;
        assert_eq!(app.chat_manager.config.listen_port, 4242);
    }

    #[tokio::test]
    async fn contacts_overlay_opens_and_closes() {
        let mut app = app();
        app.execute_command(TuiCommand::Contacts).await;
        assert_eq!(app.overlay, TuiOverlay::Contacts);
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.overlay, TuiOverlay::None);
    }

    #[tokio::test]
    async fn contact_add_then_resolve_by_index_and_name() {
        let mut app = app();
        app.execute_command(TuiCommand::ContactAdd {
            name: "Alice".into(),
            address: "127.0.0.1:9000".into(),
            fingerprint: None,
        })
        .await;
        app.sync_contact_ids();
        assert_eq!(app.resolve_contact("1"), app.resolve_contact("alice"));
        assert!(app.resolve_contact("1").is_some());
        assert!(app.resolve_contact("nobody").is_none());
    }

    #[test]
    fn unread_marks_unselected_chats() {
        let mut app = app();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        app.chat_manager.create_local_chat_for_test(a, "A".into());
        app.chat_manager.create_local_chat_for_test(b, "B".into());
        app.sync_chat_ids();
        // Select A.
        let a_idx = app.chat_ids.iter().position(|id| *id == a).unwrap();
        app.chat_list_state.select(Some(a_idx));
        app.tick();
        // A message arrives in B (not selected).
        if let Some(chat) = app.chat_manager.get_chat_mut(b) {
            chat.messages.push(crate::types::Message {
                id: Uuid::new_v4(),
                from_me: false,
                content: crate::types::MessageContent::Text { text: "hi".into() },
                timestamp: chrono::Utc::now(),
            });
        }
        app.update_unread();
        assert!(app.is_unread(&b));
        assert!(!app.is_unread(&a));
    }

    #[test]
    fn command_autocomplete_matches_and_completes() {
        let mut app = app();
        app.enter_command_mode();
        // Empty buffer => no suggestions (so Up recalls history instead).
        assert!(app.command_suggestions.is_empty());

        app.handle_key_event(key('c'));
        app.handle_key_event(key('o'));
        assert!(!app.command_suggestions.is_empty());
        for &i in &app.command_suggestions {
            assert!(COMMANDS[i].0.starts_with("co"));
        }

        // Tab completes to the highlighted command + trailing space, hiding the menu.
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        let text = app.command_field.text();
        assert!(text.ends_with(' '), "completed text was {:?}", text);
        assert!(app.command_suggestions.is_empty());
    }

    #[test]
    fn command_autocomplete_arrows_cycle_selection() {
        let mut app = app();
        app.enter_command_mode();
        app.handle_key_event(key('c'));
        let n = app.command_suggestions.len();
        assert!(n >= 2);
        assert_eq!(app.suggestion_index, 0);
        app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.suggestion_index, 1);
        app.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.suggestion_index, 0);
        // Wrap upwards.
        app.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.suggestion_index, n - 1);
    }

    #[tokio::test]
    async fn quit_command_requires_confirmation() {
        let mut app = app();
        app.execute_command(TuiCommand::Quit).await;
        assert_eq!(app.overlay, TuiOverlay::ConfirmQuit);
        assert!(!app.should_quit);
        app.handle_key_event(key('y'));
        assert!(app.should_quit);
    }
}
