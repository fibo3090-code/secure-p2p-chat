//! TUI application state: `TuiApp` owns the `ChatManager` directly (the loop is
//! single-threaded) plus all view state. Keys go through `handle_key_event`,
//! commands through `execute_command`, per-frame housekeeping through `tick`.

mod cmd_connect;
mod cmd_contacts;
mod cmd_misc;
mod cmd_party;
pub(crate) use cmd_party::PartyThread;

use crate::app::chat_manager::ChatManager;
use crate::app::party_manager::{PartyDial, PartyJoinOutcome, PartyManager};
use crate::identity::Identity;
use crate::logbuf::LogBuffer;
use crate::tui::command::{
    command_help, parse_command, parse_setting_bool, settings_keys, COMMANDS,
};
use crate::tui::input::EditableField;
use crate::tui::overlays::{PasswordMode, TuiOverlay};
use crate::types::{Config, Theme, ToastLevel};
use anyhow::Result;
use ratatui::widgets::ListState;
use ratatui_crossterm::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::oneshot::{self, error::TryRecvError};
use uuid::Uuid;

pub use crate::tui::command::TuiCommand;

/// A community join held at the verification step. The credentials stay here,
/// off the wire, until the user accepts `fingerprint` — and the
/// credential-bearing connection must then present exactly that key.
struct PendingPartyJoin {
    address: String,
    username: String,
    password: Option<String>,
    fingerprint: String,
}

/// A community connect running off the UI loop (TUI-08). `tick` drains it.
struct PartyDialJob {
    address: String,
    username: String,
    password: Option<String>,
    rx: oneshot::Receiver<anyhow::Result<PartyDial>>,
}

/// A pending Party file download and the file's display name.
type PendingPartyDownload = (oneshot::Receiver<Result<Vec<u8>, String>>, String);

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
/// How long after the last keypress the user still counts as present (see
/// `report_presence`).
const PRESENCE_IDLE: Duration = Duration::from_secs(45);

/// The boolean `:set` keys, in the order of Settings rows 2..=7.
const BOOL_SETTINGS: [&str; 6] = [
    "notifications",
    "typing",
    "auto-accept",
    "auto-host",
    "mdns",
    "upnp",
];
const THEMES: [Theme; 5] = [
    Theme::Light,
    Theme::Dark,
    Theme::Midnight,
    Theme::Forest,
    Theme::Rose,
];

fn bool_setting<'a>(cfg: &'a mut Config, key: &str) -> Option<&'a mut bool> {
    Some(match key {
        "notifications" => &mut cfg.enable_notifications,
        "typing" => &mut cfg.enable_typing_indicators,
        "auto-accept" => &mut cfg.auto_accept_files,
        "auto-host" => &mut cfg.auto_host_on_startup,
        "mdns" => &mut cfg.enable_mdns,
        "upnp" => &mut cfg.enable_upnp,
        _ => return None,
    })
}

pub struct TuiApp {
    pub chat_manager: ChatManager,
    pub party_manager: PartyManager,
    /// The community most recently joined: the target of the `:party-*` commands.
    current_party: Option<Uuid>,
    /// Shown but not yet accepted; `:party-trust` confirms it.
    pending_party_join: Option<PendingPartyJoin>,
    pub chat_list_state: ListState,
    pub chat_ids: Vec<Uuid>,
    pub input_field: EditableField,
    pub command_field: EditableField,
    pub password_field: EditableField,
    pub message_scroll: u16,
    pub stick_to_bottom: bool,
    pub identity_name: String,
    pub logs: LogBuffer,
    pub focus: TuiFocus,
    pub mode: TuiMode,
    pub status_line: String,
    pub should_quit: bool,
    pub overlay: TuiOverlay,
    pub overlay_scroll: u16,
    /// Selected row in the Transfers overlay (index into the id-sorted list).
    pub transfer_sel: usize,
    /// Selected community in the Party pane, then its thread (channels, then DMs).
    pub party_sel: usize,
    pub party_thread_sel: usize,
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
    /// A fingerprint prompt put away with Esc: still queued (the peer is
    /// waiting) and answerable with `:verify`, just not re-opened over typing.
    deferred_fingerprint: Option<Uuid>,
    /// The one community connect/trust in flight; a second is refused.
    party_dial: Option<PartyDialJob>,
    pending_party_downloads: Vec<PendingPartyDownload>,
    /// Finished downloads being written to disk off the UI thread.
    pending_party_saves: Vec<oneshot::Receiver<crate::tui::downloads::SaveOutcome>>,
    last_save: Option<Instant>,
    /// `persisted_signature` at the last save; autosave skips an unchanged
    /// state (PERF-03).
    saved_signature: Option<u64>,
    /// Set while a background autosave writes, so two never overlap.
    save_in_flight: Arc<AtomicBool>,
    /// Set by a failed background autosave so the next tick writes again.
    autosave_failed: Arc<AtomicBool>,
    last_rehost: Option<Instant>,
    typing_active: bool,
    last_typing_activity: Option<Instant>,
    /// Last keypress: stands in for window focus (see `report_presence`).
    last_interaction: Option<Instant>,
    /// Indices into `command::COMMANDS` matching the command word being typed.
    pub command_suggestions: Vec<usize>,
    pub suggestion_index: usize,
    /// Optional command name to show in the help overlay.
    pub help_topic: Option<String>,
}

impl TuiApp {
    pub fn new(logs: LogBuffer) -> Result<Self> {
        let mut chat_manager = ChatManager::new(Config::default());
        let proj_dirs = directories::ProjectDirs::from("com", "chat-p2p", "EncryptedMessenger");
        let (history_path, identity) = if let Some(ref dirs) = proj_dirs {
            let data_dir = dirs.data_dir();
            std::fs::create_dir_all(data_dir).ok();
            let (identity, _) = Identity::get_or_create(data_dir, "TUI_User")?;
            (data_dir.join("history.json.enc"), identity)
        } else {
            let identity = Identity::new_with_plaintext("TUI_User".to_string())?;
            (PathBuf::from("history.json.enc"), identity)
        };
        let identity_path = history_path.with_file_name("identity.json");
        // Lets the manager refuse the user's own invite link as a contact.
        chat_manager.set_my_fingerprint(identity.fingerprint.clone());

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
            party_manager: PartyManager::new(),
            current_party: None,
            pending_party_join: None,
            chat_list_state: ListState::default(),
            chat_ids: Vec::new(),
            input_field: EditableField::new(true),
            command_field: EditableField::new(false),
            password_field: EditableField::new(false).masked(true),
            message_scroll: 0,
            stick_to_bottom: true,
            identity_name: identity.name.clone(),
            logs,
            focus: TuiFocus::Input,
            mode: TuiMode::Normal,
            status_line: String::new(),
            should_quit: false,
            overlay: TuiOverlay::None,
            overlay_scroll: 0,
            transfer_sel: 0,
            party_sel: 0,
            party_thread_sel: 0,
            contact_ids: Vec::new(),
            contacts_list_state: ListState::default(),
            settings_list_state: ListState::default(),
            msg_view_height: 0,
            msg_view_width: 0,
            identity,
            identity_path,
            history_path,
            pending_command: None,
            deferred_fingerprint: None,
            pending_party_downloads: Vec::new(),
            pending_party_saves: Vec::new(),
            party_dial: None,
            last_save: None,
            saved_signature: None,
            save_in_flight: Default::default(),
            autosave_failed: Default::default(),
            last_rehost: None,
            typing_active: false,
            last_typing_activity: None,
            last_interaction: None,
            command_suggestions: Vec::new(),
            suggestion_index: 0,
            help_topic: None,
        };
        app.sync_chat_ids();
        app.refresh_status_line();
        Ok(app)
    }

    /// The community the Party commands act on; toasts when there is none.
    fn require_party(&mut self) -> Option<Uuid> {
        if self.current_party.is_none() {
            self.toast(
                ToastLevel::Error,
                "No community joined. Use :party-connect <host> <username>",
            );
        }
        self.current_party
    }

    /// Hand finished Party downloads to a background save (sanitized,
    /// non-clobbering name — see `tui::downloads`) and toast finished saves.
    fn poll_party_downloads(&mut self) {
        let mut done = Vec::new();
        self.pending_party_downloads
            .retain_mut(|(rx, name)| match rx.try_recv() {
                Err(TryRecvError::Empty) => true,
                outcome => {
                    let outcome = outcome.unwrap_or_else(|_| Err("connection closed".into()));
                    done.push((outcome, std::mem::take(name)));
                    false
                }
            });
        let download_dir = self.chat_manager.config.download_dir.clone();
        for (outcome, name) in done {
            match outcome {
                // Another member chose the name: sanitize it (the server does too).
                Ok(data) => self
                    .pending_party_saves
                    .push(crate::tui::downloads::spawn_save(
                        download_dir.clone(),
                        crate::util::sanitize_filename(&name),
                        data,
                    )),
                Err(reason) => self.toast(
                    ToastLevel::Error,
                    format!("Download of '{name}' failed: {reason}"),
                ),
            }
        }

        let mut saved = Vec::new();
        self.pending_party_saves
            .retain_mut(|rx| match rx.try_recv() {
                Err(TryRecvError::Empty) => true,
                outcome => {
                    saved.push(outcome.ok());
                    false
                }
            });
        for outcome in saved {
            match outcome {
                Some((_, Ok(path))) => {
                    self.toast(ToastLevel::Success, format!("Saved '{}'", path.display()))
                }
                Some((name, Err(e))) => {
                    self.toast(ToastLevel::Error, format!("Saving '{name}' failed: {e}"))
                }
                None => self.toast(
                    ToastLevel::Error,
                    "Saving a download failed: the writer stopped",
                ),
            }
        }

        self.poll_party_dial();
    }

    /// The startup password overlay: unlock a locked identity, or set a
    /// password on an unencrypted one. Kept out of `new()` so tests start
    /// overlay-free.
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

    /// Kept so external callers and tests can use `TuiApp::parse_command`.
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

        // Follow a selected chat that was folded into another conversation (a
        // reconnect to a known peer) rather than whatever now sits at its index.
        if let Some(selected) =
            selected_chat_id.map(|id| self.chat_manager.resolve_display_chat_id(id))
        {
            if let Some(new_idx) = self.chat_ids.iter().position(|id| *id == selected) {
                self.chat_list_state.select(Some(new_idx));
                return;
            }
        }

        if self
            .chat_list_state
            .selected()
            .is_none_or(|idx| idx >= self.chat_ids.len())
        {
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
        self.party_manager.poll_events();
        self.poll_party_downloads();
        self.chat_manager.cleanup_expired_toasts();

        // Show the oldest pending verification. Peeked, never taken: it stays
        // queued until answered (`confirm_fingerprint` removes it), so closing
        // the overlay cannot discard a prompt a peer is blocked on.
        if !self.overlay.is_open() {
            if let Some(pending) = self
                .chat_manager
                .pending_fingerprint()
                .filter(|p| Some(p.session_id) != self.deferred_fingerprint)
                .cloned()
            {
                self.overlay = TuiOverlay::FingerprintVerify {
                    fingerprint: pending.fingerprint,
                    peer_name: pending.peer_name,
                    sas: pending.sas,
                    chat_id: pending.session_id,
                };
            }
        }

        self.sync_chat_ids();
        self.update_unread();
        self.report_presence();
        self.maybe_send_typing_stop();
        self.autosave();
        self.auto_rehost();
        self.refresh_status_line();
    }

    /// Read state is persisted (`Chat::read_count`), so what arrived while the
    /// app was closed is still unread on the next launch.
    fn update_unread(&mut self) {
        if let Some(selected) = self.selected_chat_id() {
            self.chat_manager.mark_chat_read(selected);
        }
    }

    pub fn is_unread(&self, chat_id: &Uuid) -> bool {
        self.chat_manager
            .get_chat(*chat_id)
            .is_some_and(|c| c.unread_count() > 0)
    }

    /// Tell `ChatManager` what the user can see, so no notification fires for
    /// the conversation on screen. A terminal cannot report focus portably, so
    /// "focused" means a keypress within [`PRESENCE_IDLE`]; idle falls back to
    /// notifying, the safer direction to fail.
    fn report_presence(&mut self) {
        let active = self
            .last_interaction
            .is_some_and(|t| t.elapsed() < PRESENCE_IDLE);
        self.chat_manager
            .set_ui_presence(active, self.selected_chat_id());
    }

    /// Periodic save. Skips an unchanged state and encrypts + writes on the
    /// blocking pool; only the snapshot is taken on the UI loop (PERF-03).
    fn autosave(&mut self) {
        if self.autosave_failed.swap(false, Ordering::SeqCst) {
            self.saved_signature = None;
        }
        if self.chat_manager.history_key.is_none() || self.chat_manager.chats.is_empty() {
            return;
        }
        let due = self
            .last_save
            .is_none_or(|t| t.elapsed() >= AUTOSAVE_INTERVAL);
        if !due || self.save_in_flight.load(Ordering::SeqCst) {
            return;
        }
        self.last_save = Some(Instant::now());
        let sig = self.chat_manager.persisted_signature();
        if self.saved_signature == Some(sig) {
            return;
        }
        let (history, key) = match self.chat_manager.history_snapshot() {
            Ok(snapshot) => snapshot,
            Err(e) => {
                tracing::warn!("TUI history save skipped: {}", e);
                return;
            }
        };
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            // No runtime (unit tests): save inline.
            match history.save_encrypted(&self.history_path, &key) {
                Ok(()) => self.saved_signature = Some(sig),
                Err(e) => tracing::warn!("TUI history save failed: {}", e),
            }
            return;
        };
        self.save_in_flight.store(true, Ordering::SeqCst);
        let in_flight = self.save_in_flight.clone();
        let path = self.history_path.clone();
        // Optimistic; a failed write sets `failed` so the next tick retries.
        self.saved_signature = Some(sig);
        let failed = Arc::new(AtomicBool::new(false));
        self.autosave_failed = failed.clone();
        handle.spawn_blocking(move || {
            if let Err(e) = history.save_encrypted(&path, &key) {
                tracing::warn!("TUI history save failed: {}", e);
                failed.store(true, Ordering::SeqCst);
            }
            in_flight.store(false, Ordering::SeqCst);
        });
    }

    fn save_history_now(&mut self) {
        if self.chat_manager.history_key.is_none() {
            return;
        }
        // Let a background autosave finish first so it cannot land over this one.
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.save_in_flight.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        match self.chat_manager.save_history(&self.history_path) {
            Ok(()) => self.saved_signature = Some(self.chat_manager.persisted_signature()),
            Err(e) => tracing::warn!("TUI history save failed: {}", e),
        }
    }

    /// Not gated on `auto_host_on_startup` (that is about launch);
    /// `check_rehost_needed` already requires hosting and an unlocked
    /// conversation, so `:stophost` and `:lock` stop it.
    fn auto_rehost(&mut self) {
        if !self.chat_manager.check_rehost_needed()
            || self
                .last_rehost
                .is_some_and(|t| t.elapsed() < REHOST_DEBOUNCE)
            || !self.is_unlocked()
        {
            return;
        }
        self.last_rehost = Some(Instant::now());
        // Rebind the port in use, not the default: moving would invalidate the
        // address the user already shared.
        let port = self
            .chat_manager
            .hosting_port
            .unwrap_or(self.chat_manager.config.listen_port);
        self.pending_command = Some(TuiCommand::Host(Some(port)));
        tracing::info!(port, "Auto-rehost scheduled");
    }

    fn refresh_status_line(&mut self) {
        if self.identity.is_locked() {
            self.status_line = "🔒 Identity locked — :unlock <password> to continue".to_string();
            return;
        }
        // Saving is blocked while this holds, so it outranks everything else.
        if self.chat_manager.history_load_error().is_some() {
            self.status_line = "⚠ History unreadable — nothing is being saved. \
                 Restore the file, or :history-reset to keep it aside and start fresh"
                .to_string();
            return;
        }

        let chat_label = match self.selected_chat_id() {
            Some(id) => {
                let chat = self.chat_manager.get_chat(id);
                let title = chat.map_or("—", |c| c.title.as_str());
                let state = if self.chat_manager.is_connected(&id) {
                    "connected"
                } else if chat.is_some_and(|c| c.is_host_placeholder) {
                    "hosting"
                } else {
                    "offline"
                };
                format!("{} · {}", title, state)
            }
            None => "no chat selected".to_string(),
        };

        let hint = match (self.mode, self.focus) {
            (TuiMode::Command, _) if self.command_suggestions.is_empty() => {
                "Enter run · Esc cancel · ↑/↓ history"
            }
            (TuiMode::Command, _) => "Tab complete · ↑/↓ pick · Enter run · Esc cancel",
            (TuiMode::Normal, TuiFocus::Input) => "Enter send · Ctrl+J newline · : commands",
            (TuiMode::Normal, _) => "Tab focus · : commands · :help",
        };

        self.status_line = format!(
            "{}  │  sessions:{}  │  {}",
            chat_label,
            self.chat_manager.sessions_len(),
            hint
        );
    }

    pub fn take_pending_command(&mut self) -> Option<TuiCommand> {
        self.pending_command.take()
    }

    pub fn enter_command_mode(&mut self) {
        self.mode = TuiMode::Command;
        self.command_field.clear();
        self.update_command_suggestions();
    }

    /// Recompute autocomplete matches. Only while typing a non-empty first
    /// word, so ↑ on an empty `:` prompt still recalls history.
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
            self.command_field
                .set_text(&format!("{} ", COMMANDS[cmd_idx].0));
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
        self.cancel_command_mode();
    }

    pub fn cycle_focus(&mut self) {
        self.focus = match self.focus {
            TuiFocus::ChatList => TuiFocus::MessageView,
            TuiFocus::MessageView => TuiFocus::Input,
            TuiFocus::Input => TuiFocus::ChatList,
        };
    }

    fn toast(&mut self, level: ToastLevel, msg: impl Into<String>) {
        self.chat_manager.add_toast(level, msg.into());
    }

    // ---------------------------------------------------------------- keys

    /// A bracketed paste is text, never keys: overlays ignore it (it cannot
    /// answer a prompt), and in command mode it is joined onto one line so it
    /// cannot submit the command.
    pub fn handle_paste(&mut self, text: &str) {
        self.last_interaction = Some(Instant::now());
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        if self.overlay.is_open() {
            return;
        }
        if self.mode == TuiMode::Command {
            self.command_field.insert_str(&text.replace('\n', " "));
            self.update_command_suggestions();
            return;
        }
        self.focus = TuiFocus::Input;
        self.input_field.insert_str(&text);
        self.note_input_activity();
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) {
        self.last_interaction = Some(Instant::now());
        if self.overlay.is_open() {
            self.handle_overlay_key(key);
        } else if self.mode == TuiMode::Command {
            self.handle_command_key(key);
        } else {
            self.handle_normal_key(key);
        }
    }

    fn handle_command_key(&mut self, key: KeyEvent) {
        let n = self.command_suggestions.len();
        match key.code {
            KeyCode::Esc => self.cancel_command_mode(),
            KeyCode::Enter => self.submit_command(),
            KeyCode::Tab if n > 0 => self.accept_suggestion(),
            // ↑/↓ move the suggestion highlight when the menu shows, else
            // recall command history.
            KeyCode::Up if n > 0 => self.suggestion_index = (self.suggestion_index + n - 1) % n,
            KeyCode::Down if n > 0 => self.suggestion_index = (self.suggestion_index + 1) % n,
            KeyCode::Up => {
                self.command_field.history_prev();
                self.update_command_suggestions();
            }
            KeyCode::Down => {
                self.command_field.history_next();
                self.update_command_suggestions();
            }
            KeyCode::Tab => {}
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

        let in_list = self.focus == TuiFocus::ChatList;
        match key.code {
            KeyCode::Char(':') => self.enter_command_mode(),
            KeyCode::Char('q') => self.overlay = TuiOverlay::ConfirmQuit,
            KeyCode::Char('?') => self.show_help(None),
            KeyCode::Esc => self.focus = TuiFocus::ChatList,
            KeyCode::Down if in_list => self.next_chat(),
            KeyCode::Up if in_list => self.previous_chat(),
            KeyCode::Down | KeyCode::PageDown => self.scroll_down(),
            KeyCode::Up | KeyCode::PageUp => self.scroll_up(),
            _ => {}
        }
    }

    fn handle_overlay_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let close_key = matches!(key.code, KeyCode::Esc | KeyCode::Char('q'));
        match self.overlay.clone() {
            TuiOverlay::Password { mode } => self.handle_password_key(key, mode),
            // Opens on its own, under whatever the user is typing, so plain
            // keys answer nothing: `y`/Enter used to trust an unverified key
            // mid-sentence (audit TUI-01). Esc only puts it away (still queued).
            TuiOverlay::FingerprintVerify { chat_id, .. } => match key.code {
                KeyCode::Char('y') if ctrl => self.resolve_fingerprint(chat_id, true),
                KeyCode::Char('n') if ctrl => self.resolve_fingerprint(chat_id, false),
                KeyCode::Esc => {
                    self.deferred_fingerprint = Some(chat_id);
                    self.overlay = TuiOverlay::None;
                    self.toast(
                        ToastLevel::Warning,
                        "Verification still pending — :verify accept|reject when ready",
                    );
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
            TuiOverlay::ConfirmClearHistory => {
                self.overlay = TuiOverlay::None;
                if matches!(key.code, KeyCode::Char('y') | KeyCode::Enter) {
                    let path = self.history_path.clone();
                    self.chat_manager.clear_history(&path);
                    self.toast(ToastLevel::Info, "History cleared");
                }
            }
            TuiOverlay::ConfirmWipe => {
                self.overlay = TuiOverlay::None;
                if key.code == KeyCode::Char('y') && ctrl {
                    self.wipe_now();
                } else {
                    self.toast(ToastLevel::Info, "Nothing was deleted");
                }
            }
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
            TuiOverlay::Help => {
                let s = &mut self.overlay_scroll;
                match key.code {
                    _ if close_key => self.overlay = TuiOverlay::None,
                    KeyCode::Down => *s = s.saturating_add(1),
                    KeyCode::Up => *s = s.saturating_sub(1),
                    KeyCode::PageDown => *s = s.saturating_add(10),
                    KeyCode::PageUp => *s = s.saturating_sub(10),
                    _ => {}
                }
            }
            TuiOverlay::Invite { .. } | TuiOverlay::Identity => {
                if close_key || key.code == KeyCode::Enter {
                    self.overlay = TuiOverlay::None;
                }
            }
            TuiOverlay::Transfers => {
                let count = self.chat_manager.active_transfers_sorted().len();
                match key.code {
                    _ if close_key => self.overlay = TuiOverlay::None,
                    KeyCode::Down | KeyCode::Char('j') if count > 0 => {
                        self.transfer_sel = (self.transfer_sel + 1).min(count - 1);
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.transfer_sel = self.transfer_sel.saturating_sub(1);
                    }
                    KeyCode::Char('c') => self.cancel_selected_transfer(),
                    _ => {}
                }
            }
            // ←/→ switch community; ↑/↓ pick the thread shown (and posted to).
            TuiOverlay::Party => {
                let count = self.party_manager.server_ids().len();
                match key.code {
                    _ if close_key => self.overlay = TuiOverlay::None,
                    KeyCode::Right | KeyCode::Char('l') if count > 0 => {
                        self.party_sel = (self.party_sel + 1).min(count - 1);
                        self.party_thread_sel = 0;
                        self.sync_current_party();
                    }
                    KeyCode::Left | KeyCode::Char('h') => {
                        self.party_sel = self.party_sel.saturating_sub(1);
                        self.party_thread_sel = 0;
                        self.sync_current_party();
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        let n = self.party_threads().len();
                        if n > 0 {
                            self.party_thread_sel = (self.party_thread_sel + 1).min(n - 1);
                        }
                        self.open_selected_party_thread();
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.party_thread_sel = self.party_thread_sel.saturating_sub(1);
                        self.open_selected_party_thread();
                    }
                    _ => {}
                }
            }
            TuiOverlay::None => {}
        }
    }

    fn handle_password_key(&mut self, key: KeyEvent, mode: PasswordMode) {
        match key.code {
            // The first-run prompt cannot be dismissed: no identity or history
            // is saved without a password, so dismissing it meant a new
            // identity every launch (audit TUI-04 / JOURNEY-02). Ctrl+Q quits.
            KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.password_field.clear();
                self.should_quit = true;
            }
            KeyCode::Esc if mode == PasswordMode::Set && !self.identity.is_encrypted() => {
                self.toast(
                    ToastLevel::Warning,
                    "A password is needed before anything can be saved (Ctrl+Q quits)",
                );
            }
            // Unlocking can be put off; nothing is saved while locked.
            KeyCode::Esc => {
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
        if let Some(i) = self.chat_list_state.selected() {
            self.select_chat_index(if i + 1 < self.chat_ids.len() {
                i + 1
            } else {
                0
            });
        } else {
            self.select_chat_index(0);
        }
    }

    pub fn previous_chat(&mut self) {
        let i = match self.chat_list_state.selected() {
            Some(0) => self.chat_ids.len().wrapping_sub(1),
            Some(i) => i - 1,
            None => 0,
        };
        self.select_chat_index(i);
    }

    fn select_chat_index(&mut self, i: usize) {
        if self.chat_ids.is_empty() {
            return;
        }
        self.chat_list_state.select(Some(i));
        self.message_scroll = 0;
        self.stick_to_bottom = true;
    }

    pub fn scroll_up(&mut self) {
        self.message_scroll = self.message_scroll.saturating_sub(1);
        self.stick_to_bottom = false;
    }

    pub fn scroll_down(&mut self) {
        // Re-stick is decided during render once the content height is known.
        self.message_scroll = self.message_scroll.saturating_add(1);
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
        if self
            .last_typing_activity
            .is_none_or(|t| t.elapsed() >= TYPING_IDLE)
        {
            self.stop_typing();
        }
    }

    pub fn send_message(&mut self) {
        let Some(chat_id) = self.selected_chat_id() else {
            return;
        };
        let trimmed = self.input_field.text().trim().to_string();
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
            return true;
        }
        self.toast(
            ToastLevel::Warning,
            "Identity is locked. Unlock with :unlock <password>.",
        );
        self.overlay = TuiOverlay::Password {
            mode: PasswordMode::Unlock,
        };
        false
    }

    /// The private key if unlocked; otherwise prompts for the password (or
    /// toasts why the key is unreadable) and returns `None`.
    fn unlocked_private_key(&mut self) -> Option<rsa::RsaPrivateKey> {
        if !self.require_unlocked() {
            return None;
        }
        self.private_key()
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

    /// Resolve a contact given as a 1-based index or a case-insensitive name.
    fn resolve_contact(&self, target: &str) -> Option<Uuid> {
        if let Ok(idx @ 1..) = target.parse::<usize>() {
            return self.contact_ids.get(idx - 1).copied();
        }
        let lower = target.to_ascii_lowercase();
        self.chat_manager
            .contacts
            .values()
            .find(|c| c.name.to_ascii_lowercase() == lower)
            .map(|c| c.id)
    }

    pub async fn execute_command(&mut self, cmd: TuiCommand) {
        use TuiCommand as C;
        match cmd {
            C::Host(port_override) => self.cmd_host(port_override).await,
            C::Connect { host, port } => self.cmd_connect(host, port).await,
            C::HostRelay { relay, token } => self.cmd_host_relay(relay, token).await,
            C::ConnectRelay { relay, token } => self.cmd_connect_relay(relay, token).await,
            C::ConnectionPassword(pw) => self.cmd_connection_password(pw),
            C::Lock(on) => self.cmd_lock(on),
            C::Disconnect | C::DeleteChat => self.disconnect_selected(),
            C::StopHost => self.chat_manager.stop_hosting(),

            C::Contacts => {
                self.sync_contact_ids();
                self.overlay = TuiOverlay::Contacts;
            }
            C::ContactAdd {
                name,
                address,
                fingerprint,
            } => self.cmd_contact_add(name, address, fingerprint),
            C::ContactConnect(target) => self.cmd_contact_connect(target).await,
            C::ContactRemove(target) => self.cmd_contact_remove(target),
            C::ContactRename { target, new_name } => self.cmd_contact_rename(target, new_name),
            C::Invite(addr) => self.cmd_invite(addr),
            C::InviteRelay(relay) => {
                self.pending_command = Some(C::HostRelay { relay, token: None });
            }
            C::Import(link) => self.cmd_import(link),
            C::Send(path) => self.cmd_send(path).await,
            C::PartyConnect {
                address,
                username,
                password,
            } => self.cmd_party_connect(address, username, password).await,
            C::PartyTrust => self.cmd_party_trust().await,
            C::PartyPost(text) => self.cmd_party_post(text),
            C::PartyDm { target, text } => self.cmd_party_dm(target, text),
            C::PartyCreateChannel(name) => self.cmd_party_create_channel(name),
            C::PartySendFile(path) => self.cmd_party_send_file(path),
            C::PartyDownload(query) => self.cmd_party_download(query),
            C::PartyStatus => self.cmd_party_status(),
            C::PartyPane => self.cmd_party_pane(),
            C::PartyFiles => self.cmd_party_files(),
            C::PartyAudit => self.cmd_party_audit(),
            C::PartyDeleteFile(query) => self.cmd_party_delete_file(query),
            C::PartyRole { username, role } => self.cmd_party_role(username, role),
            C::PartyChannelAccess {
                channel,
                kind,
                members,
            } => self.cmd_party_channel_access(channel, kind, members),
            C::PartyDeleteChannel(channel) => self.cmd_party_delete_channel(channel),
            C::Transfers => self.overlay = TuiOverlay::Transfers,
            C::AcceptFile => self.cmd_resolve_file_offer(true),
            C::DeclineFile => self.cmd_resolve_file_offer(false),
            C::Name(name) => self.cmd_name(name),
            C::Backup(dest) => self.cmd_backup(dest),
            C::Rename(title) => self.cmd_rename(title),
            C::ClearHistory => self.overlay = TuiOverlay::ConfirmClearHistory,
            C::HistoryReset => self.cmd_history_reset(),
            C::Wipe { password } => self.cmd_wipe(&password),

            C::Identity => {
                Self::copy_to_clipboard(&self.identity.fingerprint);
                self.overlay = TuiOverlay::Identity;
            }
            C::Verify(accept) => self.verify_pending(accept),
            C::Unlock(pw) => self.cmd_unlock(pw),
            C::SetPassword(pw) => self.try_set_password(&pw),
            C::ChangePassword { current, new } => self.try_change_password(&current, &new),

            C::Settings => {
                self.settings_list_state.select(Some(0));
                self.overlay = TuiOverlay::Settings;
            }
            C::Set { key, value } => self.apply_setting(&key, &value),

            C::Diagnostics => self.export_diagnostics_bundle(),
            C::Logs => self.copy_logs(),
            C::Help(topic) => self.show_help(topic),
            C::Quit => self.overlay = TuiOverlay::ConfirmQuit,
            C::ForceQuit => self.should_quit = true,
        }
        self.refresh_status_line();
    }

    fn disconnect_selected(&mut self) {
        let Some(chat_id) = self.selected_chat_id() else {
            self.toast(ToastLevel::Warning, "No chat selected");
            return;
        };
        let is_placeholder = self
            .chat_manager
            .get_chat(chat_id)
            .is_some_and(|c| c.is_host_placeholder);
        if is_placeholder {
            self.chat_manager.stop_hosting();
        } else {
            self.chat_manager.delete_chat(chat_id);
        }
        self.save_history_now();
    }

    fn show_help(&mut self, topic: Option<String>) {
        let topic = topic.map(|t| t.trim().trim_start_matches(':').to_ascii_lowercase());
        if let Some(name) = topic.as_ref().filter(|n| command_help(n).is_none()) {
            self.toast(
                ToastLevel::Error,
                format!("No help for '{}'. Try :help", name),
            );
            return;
        }
        self.help_topic = topic;
        self.overlay = TuiOverlay::Help;
        self.overlay_scroll = 0;
    }

    /// Cancel the highlighted transfer in the Transfers overlay if it is still
    /// in flight (id-sorted, so the index matches the render).
    fn cancel_selected_transfer(&mut self) {
        use crate::types::TransferStatus::{InProgress, Pending};
        let transfers = self.chat_manager.active_transfers_sorted();
        if let Some(t) = transfers.get(self.transfer_sel) {
            if matches!(t.status, Pending | InProgress) {
                self.chat_manager.cancel_transfer(t.id);
            }
        }
    }

    // ---------------------------------------------------------------- fingerprint

    fn verify_pending(&mut self, accept: bool) {
        // The head of the queue, whether or not its overlay is showing.
        let target = match self.overlay {
            TuiOverlay::FingerprintVerify { chat_id, .. } => Some(chat_id),
            _ => self
                .chat_manager
                .pending_fingerprint()
                .map(|p| p.session_id),
        };
        match target {
            Some(chat_id) => self.resolve_fingerprint(chat_id, accept),
            None => self.toast(ToastLevel::Info, "No fingerprint verification pending"),
        }
    }

    fn resolve_fingerprint(&mut self, chat_id: Uuid, accept: bool) {
        if self.deferred_fingerprint == Some(chat_id) {
            self.deferred_fingerprint = None;
        }
        // `confirm_fingerprint` stores the fingerprint on the right
        // conversation itself (on the host, `chat_id` names the session).
        if accept {
            let _ = self.chat_manager.confirm_fingerprint(chat_id, true);
            self.toast(ToastLevel::Success, "Fingerprint accepted");
        } else {
            let _ = self.chat_manager.reject_fingerprint(chat_id);
            self.toast(ToastLevel::Warning, "Fingerprint rejected");
        }
        self.overlay = TuiOverlay::None;
        self.save_history_now();
    }

    // ---------------------------------------------------------------- password

    fn try_unlock(&mut self, password: &str) {
        if self.identity.decrypt(password).is_err() {
            self.toast(ToastLevel::Error, "Wrong password");
            return;
        }
        if let Ok(key) = self.identity.history_key() {
            self.chat_manager.set_history_key(key);
            let _ = self
                .chat_manager
                .load_history_auto(&self.history_path, &key);
        }
        self.overlay = TuiOverlay::None;
        self.toast(ToastLevel::Success, "Identity unlocked");
    }

    /// Mirrors the floor `Identity::encrypt` enforces, for a clearer message.
    fn password_too_short(&mut self, password: &str) -> bool {
        let short = password.chars().count() < crate::MIN_PASSWORD_LEN;
        if short {
            self.toast(
                ToastLevel::Error,
                format!(
                    "Password must be at least {} characters",
                    crate::MIN_PASSWORD_LEN
                ),
            );
        }
        short
    }

    fn try_set_password(&mut self, password: &str) {
        if self.password_too_short(password) {
            return;
        }
        // Only for an identity with no password: re-encrypting a protected
        // one here skipped the current-password check (audit TUI-03).
        if self.identity.is_encrypted() {
            self.toast(
                ToastLevel::Error,
                "This identity already has a password — use :change-password <current> <new>",
            );
            return;
        }
        let result = self.identity.migrate_legacy_plaintext(password);
        self.finish_password_update(result, password, "Password set");
    }

    fn try_change_password(&mut self, current: &str, new: &str) {
        if self.password_too_short(new) {
            return;
        }
        let result = self.identity.change_password(current, new);
        self.finish_password_update(result, new, "Password changed");
    }

    /// Save the re-wrapped identity and re-derive the history key from it;
    /// success is reported only if every step worked.
    fn finish_password_update(&mut self, result: anyhow::Result<()>, password: &str, done: &str) {
        let outcome = result
            .and_then(|()| self.identity.save(&self.identity_path))
            .and_then(|()| {
                // encrypt() cleared the plaintext the key derives from: reload.
                let mut reloaded = Identity::load(&self.identity_path)?;
                reloaded.decrypt(password)?;
                let key = reloaded.history_key()?;
                Ok((reloaded, key))
            });
        match outcome {
            Ok((reloaded, key)) => {
                self.chat_manager.set_history_key(key);
                let _ = self
                    .chat_manager
                    .load_history_auto(&self.history_path, &key);
                self.identity = reloaded;
                self.overlay = TuiOverlay::None;
                self.toast(ToastLevel::Success, done);
            }
            Err(e) => self.toast(ToastLevel::Error, format!("Password not updated: {e}")),
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
            .map_or(0, |i| (i + 1) % self.contact_ids.len());
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
        let selected = self.contacts_list_state.selected();
        if let Some(idx) = selected.filter(|&i| i < self.contact_ids.len()) {
            // A 1-based index string, for the shared command path.
            self.pending_command = Some(TuiCommand::ContactConnect((idx + 1).to_string()));
        }
    }

    // ---------------------------------------------------------------- settings overlay

    fn settings_select(&mut self, delta: i32) {
        let len = 9i32; // number of settings rows rendered
        let cur = self.settings_list_state.selected().unwrap_or(0) as i32;
        let next = (cur + delta).rem_euclid(len) as usize;
        self.settings_list_state.select(Some(next));
    }

    fn toggle_selected_setting(&mut self) {
        let idx = self.settings_list_state.selected().unwrap_or(0);
        let cfg = &mut self.chat_manager.config;
        match idx {
            // Rows 0 (download-dir) and 1 (listen-port) are text: use :set.
            2..=7 => {
                let b = bool_setting(cfg, BOOL_SETTINGS[idx - 2]).expect("a bool setting");
                *b = !*b;
            }
            8 => {
                let i = THEMES.iter().position(|t| *t == cfg.theme).unwrap_or(0);
                cfg.theme = THEMES[(i + 1) % THEMES.len()];
            }
            _ => {
                self.toast(
                    ToastLevel::Info,
                    "Use :set download-dir <path> / :set listen-port <n>",
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
            "theme" => {
                let lower = value.to_ascii_lowercase();
                match THEMES
                    .iter()
                    .find(|t| format!("{t:?}").to_ascii_lowercase() == lower)
                {
                    Some(&theme) => {
                        cfg.theme = theme;
                        Ok(format!("theme = {theme:?}"))
                    }
                    None => Err("theme: light|dark|midnight|forest|rose".to_string()),
                }
            }
            _ => match bool_setting(cfg, key) {
                Some(slot) => parse_setting_bool(value).map(|b| {
                    *slot = b;
                    format!("{key} = {b}")
                }),
                None => Err(format!(
                    "Unknown setting '{}'. Keys: {}",
                    key,
                    settings_keys().join(", ")
                )),
            },
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
        let log_text = crate::support::format_event_logs(&self.logs);
        match arboard::Clipboard::new() {
            Ok(mut clipboard) => match clipboard.set_text(log_text) {
                Ok(()) => self.toast(ToastLevel::Success, "Logs copied to clipboard"),
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
        let logs = crate::support::format_event_logs(&self.logs);
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
    use tokio::sync::mpsc;

    fn app() -> TuiApp {
        TuiApp::new(LogBuffer::new()).unwrap()
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn code(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    /// An app showing a fingerprint prompt for a fresh chat, optionally also
    /// queued in the manager (the overlay normally opens from the queue).
    fn prompting_app(queued: bool) -> (TuiApp, Uuid, mpsc::UnboundedReceiver<bool>) {
        let mut app = app();
        let chat_id = Uuid::new_v4();
        app.chat_manager
            .create_local_chat_for_test(chat_id, "Peer".into());
        let (tx, rx) = mpsc::unbounded_channel();
        app.chat_manager
            .add_fingerprint_confirm_sender_for_test(chat_id, tx);
        if queued {
            app.chat_manager
                .queue_fingerprint_prompt_for_test(crate::app::PendingFingerprint {
                    fingerprint: "deadbeef".into(),
                    peer_name: "Peer".into(),
                    sas: String::new(),
                    session_id: chat_id,
                });
        }
        app.overlay = TuiOverlay::FingerprintVerify {
            fingerprint: "deadbeef".into(),
            peer_name: "Peer".into(),
            sas: String::new(),
            chat_id,
        };
        (app, chat_id, rx)
    }

    #[test]
    fn fingerprint_accept_confirms_and_stores() {
        let (mut app, chat_id, mut rx) = prompting_app(true);
        app.handle_key_event(ctrl('y'));
        assert!(rx.try_recv().unwrap());
        assert_eq!(app.overlay, TuiOverlay::None);
        assert_eq!(
            app.chat_manager.get_chat(chat_id).unwrap().peer_fingerprint,
            Some("deadbeef".to_string())
        );
    }

    #[test]
    fn fingerprint_reject_declines_and_removes_chat() {
        let (mut app, chat_id, mut rx) = prompting_app(false);
        app.handle_key_event(ctrl('n'));
        assert!(!rx.try_recv().unwrap());
        assert_eq!(app.overlay, TuiOverlay::None);
        assert!(app.chat_manager.get_chat(chat_id).is_none());
    }

    /// TUI-01: plain keys must not answer the prompt — "hey" + Enter used to
    /// trust the peer.
    #[test]
    fn typing_into_the_fingerprint_prompt_does_not_answer_it() {
        let (mut app, chat_id, mut rx) = prompting_app(true);
        for c in ['h', 'e', 'y', 'n'] {
            app.handle_key_event(key(c));
        }
        app.handle_key_event(code(KeyCode::Enter));
        assert!(rx.try_recv().is_err(), "a plain key answered the prompt");
        assert!(matches!(app.overlay, TuiOverlay::FingerprintVerify { .. }));

        // Esc puts it away unanswered; it stays queued and is not re-opened.
        app.handle_key_event(code(KeyCode::Esc));
        assert_eq!(app.overlay, TuiOverlay::None);
        assert!(rx.try_recv().is_err());
        assert!(app.chat_manager.pending_fingerprint().is_some());
        app.tick();
        assert_eq!(app.overlay, TuiOverlay::None);
        assert!(app.chat_manager.get_chat(chat_id).is_some());
    }

    /// TUI-03: a pasted newline is not Enter.
    #[test]
    fn a_paste_is_text_never_keystrokes() {
        let mut app = app();
        app.handle_paste(":clear-history\ny\n");
        assert_eq!(app.overlay, TuiOverlay::None);
        assert_eq!(app.mode, TuiMode::Normal);
        assert_eq!(app.input_field.text(), ":clear-history\ny\n");

        // In command mode a multi-line paste becomes one line, not executed.
        let mut app = self::app();
        app.enter_command_mode();
        app.handle_paste("quit\nnow");
        assert_eq!(app.mode, TuiMode::Command);
        assert!(!app.should_quit);
        assert_eq!(app.command_field.text(), "quit now");
    }

    /// TUI-04 / JOURNEY-02: the first-run password prompt cannot be dismissed.
    #[test]
    fn the_first_run_password_prompt_cannot_be_dismissed() {
        let mut app = app();
        assert!(!app.identity.is_encrypted());
        let set = TuiOverlay::Password {
            mode: PasswordMode::Set,
        };
        app.overlay = set.clone();
        app.handle_key_event(code(KeyCode::Esc));
        assert_eq!(app.overlay, set);
        assert!(!app.should_quit);
        app.handle_key_event(ctrl('q'));
        assert!(app.should_quit);
    }

    /// JOURNEY-03: `:wipe` checks the password, asks once more (Ctrl+Y only),
    /// then deletes everything and quits.
    #[tokio::test]
    async fn wipe_checks_the_password_confirms_then_deletes_everything() {
        let dir = tempfile::tempdir().unwrap();
        let password = format!("pw-{}", Uuid::new_v4());
        let wrong = format!("pw-{}", Uuid::new_v4());
        let mut app = app();
        app.history_path = dir.path().join("history.json.enc");
        app.identity_path = dir.path().join("identity.json");
        let mut identity = Identity::new_with_plaintext("T".into()).unwrap();
        identity.encrypt(&password).unwrap();
        identity.save(&app.identity_path).unwrap();
        app.identity = identity;
        std::fs::write(&app.history_path, b"h").unwrap();
        let wipe = |password: &String| TuiCommand::Wipe {
            password: password.clone(),
        };

        app.execute_command(wipe(&wrong)).await;
        assert_eq!(app.overlay, TuiOverlay::None);
        assert!(app.identity_path.exists());

        app.execute_command(wipe(&password)).await;
        assert_eq!(app.overlay, TuiOverlay::ConfirmWipe);
        app.handle_key_event(key('y')); // plain y cancels
        assert_eq!(app.overlay, TuiOverlay::None);
        assert!(app.identity_path.exists() && !app.should_quit);

        app.execute_command(wipe(&password)).await;
        app.handle_key_event(ctrl('y'));
        assert!(app.should_quit);
        assert!(!app.identity_path.exists());
        assert!(!app.history_path.exists());
        app.shutdown_save();
        assert!(
            !app.history_path.exists(),
            "the exit save must not recreate it"
        );
    }

    /// PERF-03: an unchanged history is not written again.
    #[test]
    fn autosave_skips_an_unchanged_history() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app();
        app.history_path = dir.path().join("history.json.enc");
        // `TuiApp::new` read the machine's real data dir; ignore what it found.
        app.chat_manager.history_load_error = None;
        let key: [u8; 32] = rand::random();
        app.chat_manager.set_history_key(key);
        app.chat_manager
            .create_local_chat_for_test(Uuid::new_v4(), "A".into());

        app.last_save = None;
        app.autosave();
        assert!(app.history_path.exists(), "the first save is written");
        std::fs::remove_file(&app.history_path).unwrap();

        app.last_save = None;
        app.autosave();
        assert!(
            !app.history_path.exists(),
            "an unchanged history was rewritten"
        );

        app.chat_manager
            .create_local_chat_for_test(Uuid::new_v4(), "B".into());
        app.last_save = None;
        app.autosave();
        assert!(app.history_path.exists());
    }

    #[tokio::test]
    async fn set_command_toggles_config() {
        let mut app = app();
        let set = |key: &str, value: &str| TuiCommand::Set {
            key: key.into(),
            value: value.into(),
        };
        assert!(app.chat_manager.config.enable_notifications);
        app.execute_command(set("notifications", "off")).await;
        assert!(!app.chat_manager.config.enable_notifications);
        app.execute_command(set("listen-port", "4242")).await;
        assert_eq!(app.chat_manager.config.listen_port, 4242);
    }

    #[tokio::test]
    async fn contacts_overlay_opens_and_closes() {
        let mut app = app();
        app.execute_command(TuiCommand::Contacts).await;
        assert_eq!(app.overlay, TuiOverlay::Contacts);
        app.handle_key_event(code(KeyCode::Esc));
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
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        app.chat_manager.create_local_chat_for_test(a, "A".into());
        app.chat_manager.create_local_chat_for_test(b, "B".into());
        app.sync_chat_ids();
        let a_idx = app.chat_ids.iter().position(|id| *id == a).unwrap();
        app.chat_list_state.select(Some(a_idx));
        app.tick();
        // A message arrives in B (not selected).
        app.chat_manager
            .get_chat_mut(b)
            .unwrap()
            .messages
            .push(crate::types::Message {
                id: Uuid::new_v4(),
                from_me: false,
                content: crate::types::MessageContent::Text { text: "hi".into() },
                timestamp: chrono::Utc::now(),
                delivered: false,
            });
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

        // Tab completes the highlighted command + a space, hiding the menu.
        app.handle_key_event(code(KeyCode::Tab));
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
        app.handle_key_event(code(KeyCode::Down));
        assert_eq!(app.suggestion_index, 1);
        app.handle_key_event(code(KeyCode::Up));
        assert_eq!(app.suggestion_index, 0);
        app.handle_key_event(code(KeyCode::Up)); // wraps
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
