//! TUI application state and behavior.
//!
//! `TuiApp` owns the `ChatManager` directly (the loop is single-threaded, so no
//! `Arc<Mutex>` is needed) plus all view state: focus, mode, the editable input
//! and command fields, the active overlay, scroll/unread/typing state, and the
//! identity. Keys are routed by `handle_key_event`; commands run through
//! `execute_command`; `tick` performs per-frame housekeeping (session events,
//! autosave, auto-rehost, unread/typing bookkeeping).

mod cmd_connect;
mod cmd_contacts;
mod cmd_misc;
mod cmd_party;
pub(crate) use cmd_party::PartyThread;

use crate::app::chat_manager::ChatManager;
use crate::app::party_manager::{PartyDial, PartyJoinOutcome, PartyManager};

/// A community join held at the verification step: the address and credentials
/// the user typed, kept locally until they confirm the server's identity. They
/// are not on the wire — that is the whole point of holding them.
struct PendingPartyJoin {
    address: String,
    username: String,
    password: Option<String>,
    /// The server fingerprint the user was shown and is about to accept. The
    /// trust step requires the credential-bearing connection to present
    /// exactly this key.
    fingerprint: String,
}

/// A community connect running off the UI loop (TUI-08). `tick` drains it.
struct PartyDialJob {
    address: String,
    username: String,
    password: Option<String>,
    rx: tokio::sync::oneshot::Receiver<anyhow::Result<crate::app::party_manager::PartyDial>>,
}
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
use std::time::{Duration, Instant};
use uuid::Uuid;

pub use crate::tui::command::TuiCommand;

/// A pending Party file download: the byte receiver plus the file's display
/// name (used for the saved filename and progress toasts).
type PendingPartyDownload = (
    tokio::sync::oneshot::Receiver<Result<Vec<u8>, String>>,
    String,
);

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
/// How long after the last keypress the terminal still counts as "the user is
/// here", for the desktop-notification focus gate. See `report_presence`.
const PRESENCE_IDLE: Duration = Duration::from_secs(45);

pub struct TuiApp {
    pub chat_manager: ChatManager,
    /// Connections to Party servers (Phase 1 Party tab, command-driven).
    pub party_manager: PartyManager,
    /// The server most recently joined, used as the target for `:party-post`.
    current_party: Option<Uuid>,
    /// A community server whose identity the user has been shown but not yet
    /// accepted. Its credentials are held here — deliberately not sent — until
    /// `:party-trust` confirms the fingerprint.
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
    /// Selected community in the Party pane.
    pub party_sel: usize,
    /// Selected thread (a channel, then the DMs) in the selected community.
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
    /// A fingerprint prompt the user put away with Esc. It stays queued (the
    /// peer is still waiting) and `:verify` still answers it; the overlay is
    /// just not re-opened over the user's typing.
    deferred_fingerprint: Option<Uuid>,
    /// A community connect/trust in flight. One at a time: a second
    /// `:party-connect` while one runs is refused rather than queued.
    party_dial: Option<PartyDialJob>,
    /// In-flight Party file downloads. `tick` polls them and writes finished
    /// downloads into the download directory.
    pending_party_downloads: Vec<PendingPartyDownload>,
    /// Finished downloads being written to disk off the UI thread.
    pending_party_saves: Vec<tokio::sync::oneshot::Receiver<crate::tui::downloads::SaveOutcome>>,
    last_save: Option<Instant>,
    /// `persisted_signature` at the last successful save: autosave skips the
    /// write when it has not moved (PERF-03).
    saved_signature: Option<u64>,
    /// Set while a background autosave is writing, so two never overlap and
    /// the save on exit can wait for the last one.
    save_in_flight: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Set by a background autosave whose write failed; the next tick clears
    /// `saved_signature` so the state is written again.
    autosave_failed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    last_rehost: Option<Instant>,
    typing_active: bool,
    last_typing_activity: Option<Instant>,
    /// Last keypress of any kind. Stands in for "the window has focus", which a
    /// terminal cannot report portably — see `report_presence`.
    last_interaction: Option<Instant>,
    /// Indices into `command::COMMANDS` matching the command word being typed.
    pub command_suggestions: Vec<usize>,
    /// Highlighted entry within `command_suggestions`.
    pub suggestion_index: usize,
    /// Optional command name to show in the help overlay.
    pub help_topic: Option<String>,
}

impl TuiApp {
    pub fn new(logs: LogBuffer) -> Result<Self> {
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
        // So the manager can recognise this device's own invite link and refuse
        // to add the user to their own contacts.
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

    /// The community the Party commands act on, complaining if there is none.
    /// Every governance command needs it, and "nothing happened" is a worse
    /// answer than being told to join one first.
    fn require_party(&mut self) -> Option<Uuid> {
        match self.current_party {
            Some(id) => Some(id),
            None => {
                self.toast(
                    ToastLevel::Error,
                    "No community joined. Use :party-connect <host> <username>".to_string(),
                );
                None
            }
        }
    }

    /// Drain finished Party file downloads: hand the bytes to a background
    /// save (sanitized, non-clobbering name — see `tui::downloads`), then toast
    /// each save as it completes.
    fn poll_party_downloads(&mut self) {
        use tokio::sync::oneshot::error::TryRecvError;

        let download_dir = self.chat_manager.config.download_dir.clone();
        let mut i = 0;
        while i < self.pending_party_downloads.len() {
            let outcome = match self.pending_party_downloads[i].0.try_recv() {
                Err(TryRecvError::Empty) => {
                    i += 1;
                    continue;
                }
                Ok(result) => result,
                Err(TryRecvError::Closed) => Err("connection closed".to_string()),
            };
            let (_, name) = self.pending_party_downloads.remove(i);
            match outcome {
                Ok(data) => {
                    // The name was chosen by another member: sanitize it
                    // (defense in depth — the server sanitizes too).
                    let safe = crate::util::sanitize_filename(&name);
                    self.pending_party_saves
                        .push(crate::tui::downloads::spawn_save(
                            download_dir.clone(),
                            safe,
                            data,
                        ));
                }
                Err(reason) => self.toast(
                    ToastLevel::Error,
                    format!("Download of '{name}' failed: {reason}"),
                ),
            }
        }

        let mut i = 0;
        while i < self.pending_party_saves.len() {
            let saved = match self.pending_party_saves[i].try_recv() {
                Err(TryRecvError::Empty) => {
                    i += 1;
                    continue;
                }
                Ok(saved) => Some(saved),
                Err(TryRecvError::Closed) => None,
            };
            self.pending_party_saves.remove(i);
            match saved {
                Some((_, Ok(path))) => {
                    self.toast(ToastLevel::Success, format!("Saved '{}'", path.display()))
                }
                Some((name, Err(e))) => {
                    self.toast(ToastLevel::Error, format!("Saving '{name}' failed: {e}"))
                }
                None => self.toast(
                    ToastLevel::Error,
                    "Saving a download failed: the writer stopped".to_string(),
                ),
            }
        }

        self.poll_party_dial();
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

        // Follow a selected chat that was folded into another conversation (a
        // reconnect to a known peer), rather than landing on whichever chat now
        // sits at its old position.
        if let Some(selected) =
            selected_chat_id.map(|id| self.chat_manager.resolve_display_chat_id(id))
        {
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
        self.party_manager.poll_events();
        self.poll_party_downloads();
        self.chat_manager.cleanup_expired_toasts();

        // Surface the oldest pending fingerprint verification as an overlay.
        // Peeked, not taken: the prompt stays queued until the user answers it
        // (`confirm_fingerprint` removes it), so closing the overlay or a redraw
        // can't quietly discard a verification a peer is still blocked on. Any
        // further peers waiting behind it get their turn on a later tick.
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

    /// Reading a conversation is a *persisted* fact (`Chat::read_count`, saved
    /// inside the encrypted history), not per-session bookkeeping — so a message
    /// that arrived while the app was closed is still unread on the next launch.
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

    /// Tell `ChatManager` what the user can see, so a desktop notification does
    /// not fire for the conversation they are reading right now.
    ///
    /// A terminal cannot reliably report window focus — crossterm's focus
    /// events need terminal support that is far from universal, and enabling
    /// them can emit stray escape sequences on terminals that lack it. So
    /// "focused" is approximated by *recent interaction*: if the user pressed a
    /// key within [`PRESENCE_IDLE`], they are at the terminal. Once they go
    /// idle, presence drops and every conversation notifies again — which is
    /// the correct direction to fail, because the cost of a missed notification
    /// is higher than the cost of an extra one.
    fn report_presence(&mut self) {
        let active = self
            .last_interaction
            .is_some_and(|t| t.elapsed() < PRESENCE_IDLE);
        self.chat_manager
            .set_ui_presence(active, self.selected_chat_id());
    }

    /// Periodic save. It used to rewrite (clone, serialize, encrypt, fsync)
    /// the whole history every 30 s whether or not anything had changed, on
    /// the UI loop — tens of MB per tick for a long history, and a frozen
    /// interface while it ran (PERF-03). Now it skips an unchanged state and
    /// does the encrypt + write on the blocking pool; only the snapshot is
    /// taken here.
    fn autosave(&mut self) {
        use std::sync::atomic::Ordering;
        if self.autosave_failed.swap(false, Ordering::SeqCst) {
            self.saved_signature = None;
        }
        if self.chat_manager.history_key.is_none() || self.chat_manager.chats.is_empty() {
            return;
        }
        let due = self
            .last_save
            .map(|t| t.elapsed() >= AUTOSAVE_INTERVAL)
            .unwrap_or(true);
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
        // Recorded optimistically; a failed write clears it below so the next
        // tick tries again.
        self.saved_signature = Some(sig);
        let failed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
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
        use std::sync::atomic::Ordering;
        if self.chat_manager.history_key.is_none() {
            return;
        }
        // Let a background autosave finish first, so it cannot land after
        // (and over) this one.
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.save_in_flight.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        match self.chat_manager.save_history(&self.history_path) {
            Ok(()) => self.saved_signature = Some(self.chat_manager.persisted_signature()),
            Err(e) => tracing::warn!("TUI history save failed: {}", e),
        }
    }

    fn auto_rehost(&mut self) {
        // Deliberately NOT gated on `auto_host_on_startup`: that setting decides
        // whether hosting begins at launch, not whether hosting the user asked
        // for survives its first peer. `check_rehost_needed()` already requires
        // `is_hosting` and an unlocked conversation, so `:stophost` and the lock
        // remain the way to stop.
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
        // Rebind the port actually in use, not the settings default — the user
        // may have hosted on another one, and rehosting elsewhere silently
        // invalidates the address they already shared.
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
        // Saving is blocked while this holds, so it outranks everything else
        // on the status line until the user deals with it.
        if self.chat_manager.history_load_error().is_some() {
            self.status_line = "⚠ History unreadable — nothing is being saved. \
                 Restore the file, or :history-reset to keep it aside and start fresh"
                .to_string();
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

    /// A bracketed paste: inserted as text into whatever is being edited, never
    /// replayed as keys. A paste cannot answer a prompt or confirm a dialog
    /// (overlays ignore it), and in command mode it stays on one line so it
    /// cannot submit the command or start another.
    pub fn handle_paste(&mut self, text: &str) {
        self.last_interaction = Some(Instant::now());
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        if self.overlay.is_open() {
            return;
        }
        if self.mode == TuiMode::Command {
            let line = text.split('\n').collect::<Vec<_>>().join(" ");
            self.command_field.insert_str(&line);
            self.update_command_suggestions();
            return;
        }
        self.focus = TuiFocus::Input;
        self.input_field.insert_str(&text);
        self.note_input_activity();
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) {
        // Any keystroke means the user is at the terminal; `report_presence`
        // uses this to gate desktop notifications.
        self.last_interaction = Some(Instant::now());
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
            KeyCode::Char('?') => self.show_help(None),
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
            // This overlay opens on its own, under whatever the user is typing.
            // Plain keys therefore answer nothing: `y`/Enter used to accept, so
            // typing "hey" + Enter while a peer connected trusted an unverified
            // key (audit TUI-01). Answering takes a chord nobody types by
            // accident; Esc only puts the prompt away (it stays queued).
            TuiOverlay::FingerprintVerify { chat_id, .. } => {
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                match key.code {
                    KeyCode::Char('y') if ctrl => self.resolve_fingerprint(chat_id, true),
                    KeyCode::Char('n') if ctrl => self.resolve_fingerprint(chat_id, false),
                    KeyCode::Esc => {
                        self.deferred_fingerprint = Some(chat_id);
                        self.overlay = TuiOverlay::None;
                        self.toast(
                            ToastLevel::Warning,
                            "Verification still pending — :verify accept|reject when ready"
                                .to_string(),
                        );
                    }
                    _ => {}
                }
            }
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
            TuiOverlay::ConfirmWipe => {
                if key.code == KeyCode::Char('y') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    self.overlay = TuiOverlay::None;
                    self.wipe_now();
                } else {
                    self.overlay = TuiOverlay::None;
                    self.toast(ToastLevel::Info, "Nothing was deleted".to_string());
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
            TuiOverlay::Help => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => self.overlay = TuiOverlay::None,
                KeyCode::Down => self.overlay_scroll = self.overlay_scroll.saturating_add(1),
                KeyCode::Up => self.overlay_scroll = self.overlay_scroll.saturating_sub(1),
                KeyCode::PageDown => self.overlay_scroll = self.overlay_scroll.saturating_add(10),
                KeyCode::PageUp => self.overlay_scroll = self.overlay_scroll.saturating_sub(10),
                _ => {}
            },
            // Plain informational overlays: any of Esc/Enter/q closes.
            TuiOverlay::Invite { .. } | TuiOverlay::Identity => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                    self.overlay = TuiOverlay::None;
                }
            }
            // Transfers overlay supports selecting and cancelling a transfer.
            TuiOverlay::Transfers => {
                let count = self.chat_manager.active_transfers_sorted().len();
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => self.overlay = TuiOverlay::None,
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
            // Communities pane: switch between the servers you have joined.
            // ←/→ switch community, ↑/↓ pick the thread whose messages are
            // shown (and that :party-post writes to).
            TuiOverlay::Party => {
                let count = self.party_manager.server_ids().len();
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => self.overlay = TuiOverlay::None,
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
            // The first-run prompt cannot be dismissed. `Identity::save`
            // refuses a plaintext key, so an identity without a password is
            // never written: dismissing the prompt meant a brand-new identity
            // on every launch, and the old history overwritten by the next
            // one's autosave (audit TUI-04 / JOURNEY-02). Ctrl+Q quits instead.
            KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.password_field.clear();
                self.should_quit = true;
            }
            KeyCode::Esc if mode == PasswordMode::Set && !self.identity.is_encrypted() => {
                self.toast(
                    ToastLevel::Warning,
                    "A password is needed before anything can be saved (Ctrl+Q quits)".to_string(),
                );
            }
            KeyCode::Esc => {
                // Unlocking can be put off: closing the prompt leaves the app
                // locked, and nothing is saved while it is.
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

    /// The private key, if the identity is unlocked. Prompts for the password
    /// (or toasts why the key is unreadable) otherwise, so a command can just
    /// `let Some(key) = self.unlocked_private_key() else { return };`.
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
            TuiCommand::Host(port_override) => self.cmd_host(port_override).await,
            TuiCommand::Connect { host, port } => self.cmd_connect(host, port).await,
            TuiCommand::HostRelay { relay, token } => self.cmd_host_relay(relay, token).await,
            TuiCommand::ConnectRelay { relay, token } => self.cmd_connect_relay(relay, token).await,
            TuiCommand::ConnectionPassword(pw) => self.cmd_connection_password(pw),
            TuiCommand::Lock(on) => self.cmd_lock(on),
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
            } => self.cmd_contact_add(name, address, fingerprint),
            TuiCommand::ContactConnect(target) => self.cmd_contact_connect(target).await,
            TuiCommand::ContactRemove(target) => self.cmd_contact_remove(target),
            TuiCommand::ContactRename { target, new_name } => {
                self.cmd_contact_rename(target, new_name)
            }
            TuiCommand::Invite(addr) => self.cmd_invite(addr),
            TuiCommand::InviteRelay(relay) => {
                self.pending_command = Some(TuiCommand::HostRelay { relay, token: None });
            }
            TuiCommand::Import(link) => self.cmd_import(link),
            TuiCommand::Send(path) => self.cmd_send(path).await,
            TuiCommand::PartyConnect {
                address,
                username,
                password,
            } => self.cmd_party_connect(address, username, password).await,
            TuiCommand::PartyTrust => self.cmd_party_trust().await,
            TuiCommand::PartyPost(text) => self.cmd_party_post(text),
            TuiCommand::PartyDm { target, text } => self.cmd_party_dm(target, text),
            TuiCommand::PartyCreateChannel(name) => self.cmd_party_create_channel(name),
            TuiCommand::PartySendFile(path) => self.cmd_party_send_file(path),
            TuiCommand::PartyDownload(query) => self.cmd_party_download(query),
            TuiCommand::PartyStatus => self.cmd_party_status(),
            TuiCommand::PartyPane => self.cmd_party_pane(),
            TuiCommand::PartyFiles => self.cmd_party_files(),
            TuiCommand::PartyAudit => self.cmd_party_audit(),
            TuiCommand::PartyDeleteFile(query) => self.cmd_party_delete_file(query),
            TuiCommand::PartyRole { username, role } => self.cmd_party_role(username, role),
            TuiCommand::PartyChannelAccess {
                channel,
                kind,
                members,
            } => self.cmd_party_channel_access(channel, kind, members),
            TuiCommand::PartyDeleteChannel(channel) => self.cmd_party_delete_channel(channel),
            TuiCommand::Transfers => self.overlay = TuiOverlay::Transfers,
            TuiCommand::AcceptFile => self.cmd_resolve_file_offer(true),
            TuiCommand::DeclineFile => self.cmd_resolve_file_offer(false),
            TuiCommand::Name(name) => self.cmd_name(name),
            TuiCommand::Backup(dest) => self.cmd_backup(dest),
            TuiCommand::Rename(title) => self.cmd_rename(title),
            TuiCommand::DeleteChat => self.disconnect_selected(),
            TuiCommand::ClearHistory => self.overlay = TuiOverlay::ConfirmClearHistory,
            TuiCommand::HistoryReset => self.cmd_history_reset(),
            TuiCommand::Wipe { password } => self.cmd_wipe(&password),

            TuiCommand::Identity => {
                Self::copy_to_clipboard(&self.identity.fingerprint);
                self.overlay = TuiOverlay::Identity;
            }
            TuiCommand::Verify(accept) => self.verify_pending(accept),
            TuiCommand::Unlock(pw) => self.cmd_unlock(pw),
            TuiCommand::SetPassword(pw) => self.try_set_password(&pw),
            TuiCommand::ChangePassword { current, new } => self.try_change_password(&current, &new),

            TuiCommand::Settings => {
                self.settings_list_state.select(Some(0));
                self.overlay = TuiOverlay::Settings;
            }
            TuiCommand::Set { key, value } => self.apply_setting(&key, &value),

            TuiCommand::Diagnostics => self.export_diagnostics_bundle(),
            TuiCommand::Logs => self.copy_logs(),
            TuiCommand::Help(topic) => self.show_help(topic),
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

    fn show_help(&mut self, topic: Option<String>) {
        let topic = topic.map(|t| t.trim().trim_start_matches(':').to_ascii_lowercase());
        if let Some(ref name) = topic {
            if command_help(name).is_none() {
                self.toast(
                    ToastLevel::Error,
                    format!("No help for '{}'. Try :help", name),
                );
                return;
            }
        }
        self.help_topic = topic;
        self.overlay = TuiOverlay::Help;
        self.overlay_scroll = 0;
    }

    // ---------------------------------------------------------------- transfers

    /// Cancel the transfer highlighted in the Transfers overlay, if it is still
    /// in flight (id-sorted so the index matches the render).
    fn cancel_selected_transfer(&mut self) {
        let transfers = self.chat_manager.active_transfers_sorted();
        let Some(t) = transfers.get(self.transfer_sel) else {
            return;
        };
        if matches!(
            t.status,
            crate::types::TransferStatus::Pending | crate::types::TransferStatus::InProgress
        ) {
            self.chat_manager.cancel_transfer(t.id);
        }
    }

    // ---------------------------------------------------------------- fingerprint

    fn verify_pending(&mut self, accept: bool) {
        // The head of the queue, whether or not its overlay is showing: Esc
        // puts the overlay away without answering.
        let target = match self.overlay.clone() {
            TuiOverlay::FingerprintVerify { chat_id, .. } => Some(chat_id),
            _ => self
                .chat_manager
                .pending_fingerprint()
                .map(|p| p.session_id),
        };
        if let Some(chat_id) = target {
            self.resolve_fingerprint(chat_id, accept);
        } else {
            self.toast(
                ToastLevel::Info,
                "No fingerprint verification pending".to_string(),
            );
        }
    }

    fn resolve_fingerprint(&mut self, chat_id: Uuid, accept: bool) {
        if self.deferred_fingerprint == Some(chat_id) {
            self.deferred_fingerprint = None;
        }
        // `confirm_fingerprint` stores an accepted fingerprint on the right
        // conversation itself (on the host that is not `chat_id`, which names
        // the session).
        if accept {
            let _ = self.chat_manager.confirm_fingerprint(chat_id, true);
            self.toast(ToastLevel::Success, "Fingerprint accepted".to_string());
        } else {
            let _ = self.chat_manager.reject_fingerprint(chat_id);
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
        // Mirrors the floor `Identity::encrypt` enforces, so the user gets a
        // clear message here instead of a generic failure from the core.
        if password.chars().count() < crate::MIN_PASSWORD_LEN {
            self.toast(
                ToastLevel::Error,
                format!(
                    "Password must be at least {} characters",
                    crate::MIN_PASSWORD_LEN
                ),
            );
            return;
        }
        // Setting a password is for an identity that has none. Re-encrypting
        // an already-protected identity here skipped the current-password
        // check entirely — anyone at an unlocked terminal could replace the
        // password (audit TUI-03). That is `:change-password`'s job.
        if self.identity.is_encrypted() {
            self.toast(
                ToastLevel::Error,
                "This identity already has a password — use :change-password <current> <new>"
                    .to_string(),
            );
            return;
        }
        let result = self.identity.migrate_legacy_plaintext(password);
        self.finish_password_update(result, password, "Password set");
    }

    fn try_change_password(&mut self, current: &str, new: &str) {
        if new.chars().count() < crate::MIN_PASSWORD_LEN {
            self.toast(
                ToastLevel::Error,
                format!(
                    "Password must be at least {} characters",
                    crate::MIN_PASSWORD_LEN
                ),
            );
            return;
        }
        let result = self.identity.change_password(current, new);
        self.finish_password_update(result, new, "Password changed");
    }

    /// Save the re-wrapped identity and re-derive the history key from it.
    /// Success is reported only when every step worked: "Password set" used
    /// to be shown even when the identity could not be reloaded afterwards.
    fn finish_password_update(&mut self, result: anyhow::Result<()>, password: &str, done: &str) {
        let outcome = result
            .and_then(|()| self.identity.save(&self.identity_path))
            .and_then(|()| {
                // Re-deriving the key needs the plaintext, which encrypt()
                // cleared, so reload and decrypt.
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
                self.toast(ToastLevel::Success, done.to_string());
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
        let len = 9i32; // number of settings rows rendered
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
            7 => cfg.enable_upnp = !cfg.enable_upnp,
            8 => {
                cfg.theme = match cfg.theme {
                    Theme::Light => Theme::Dark,
                    Theme::Dark => Theme::Midnight,
                    Theme::Midnight => Theme::Forest,
                    Theme::Forest => Theme::Rose,
                    Theme::Rose => Theme::Light,
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
            "upnp" => parse_setting_bool(value).map(|b| {
                cfg.enable_upnp = b;
                format!("upnp = {}", b)
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
                "rose" => {
                    cfg.theme = Theme::Rose;
                    Ok("theme = Rose".to_string())
                }
                _ => Err("theme: light|dark|midnight|forest|rose".to_string()),
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
        let log_text = crate::support::format_event_logs(&self.logs);
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
    use crate::tui::overlays::TuiOverlay;
    use ratatui_crossterm::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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

    fn queued_prompt(session_id: Uuid) -> crate::app::PendingFingerprint {
        crate::app::PendingFingerprint {
            fingerprint: "deadbeef".into(),
            peer_name: "Peer".into(),
            sas: String::new(),
            session_id,
        }
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
        // The overlay is always opened from the manager's queued prompt.
        app.chat_manager
            .queue_fingerprint_prompt_for_test(queued_prompt(chat_id));
        app.overlay = TuiOverlay::FingerprintVerify {
            fingerprint: "deadbeef".into(),
            peer_name: "Peer".into(),
            sas: String::new(),
            chat_id,
        };

        // Ctrl+Y accepts.
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
            sas: String::new(),
            chat_id,
        };

        app.handle_key_event(ctrl('n'));

        assert!(!rx.try_recv().unwrap());
        assert_eq!(app.overlay, TuiOverlay::None);
        assert!(app.chat_manager.get_chat(chat_id).is_none());
    }

    /// TUI-01: the prompt opens under whatever the user is typing, so plain
    /// keys must answer nothing — "hey" + Enter used to trust the peer.
    #[test]
    fn typing_into_the_fingerprint_prompt_does_not_answer_it() {
        let mut app = app();
        let chat_id = Uuid::new_v4();
        app.chat_manager
            .create_local_chat_for_test(chat_id, "Peer".into());
        let (tx, mut rx) = mpsc::unbounded_channel();
        app.chat_manager
            .add_fingerprint_confirm_sender_for_test(chat_id, tx);
        app.chat_manager
            .queue_fingerprint_prompt_for_test(queued_prompt(chat_id));
        app.overlay = TuiOverlay::FingerprintVerify {
            fingerprint: "deadbeef".into(),
            peer_name: "Peer".into(),
            sas: String::new(),
            chat_id,
        };

        for c in ['h', 'e', 'y', 'n'] {
            app.handle_key_event(key(c));
        }
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(rx.try_recv().is_err(), "a plain key answered the prompt");
        assert!(matches!(app.overlay, TuiOverlay::FingerprintVerify { .. }));

        // Esc puts the prompt away without answering; it stays queued and
        // the tick does not immediately pop it back up.
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.overlay, TuiOverlay::None);
        assert!(rx.try_recv().is_err());
        assert!(app.chat_manager.pending_fingerprint().is_some());
        app.tick();
        assert_eq!(app.overlay, TuiOverlay::None);
        assert!(app.chat_manager.get_chat(chat_id).is_some());
    }

    /// TUI-03: a pasted newline is not Enter. Pasting a block that happened
    /// to contain ":clear-history\n" used to run it.
    #[test]
    fn a_paste_is_text_never_keystrokes() {
        let mut app = app();
        app.handle_paste(":clear-history\ny\n");
        assert_eq!(app.overlay, TuiOverlay::None);
        assert_eq!(app.mode, TuiMode::Normal);
        assert_eq!(app.input_field.text(), ":clear-history\ny\n");

        // In command mode a multi-line paste collapses to one line and is
        // still not executed.
        let mut app = self::app();
        app.enter_command_mode();
        app.handle_paste("quit\nnow");
        assert_eq!(app.mode, TuiMode::Command);
        assert!(!app.should_quit);
        assert_eq!(app.command_field.text(), "quit now");
    }

    /// TUI-04 / JOURNEY-02: the first-run password prompt cannot be waved
    /// away, because nothing — not the identity, not the history — can be
    /// saved without it.
    #[test]
    fn the_first_run_password_prompt_cannot_be_dismissed() {
        let mut app = app();
        assert!(!app.identity.is_encrypted());
        app.overlay = TuiOverlay::Password {
            mode: PasswordMode::Set,
        };
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(
            app.overlay,
            TuiOverlay::Password {
                mode: PasswordMode::Set
            }
        );
        assert!(!app.should_quit);
        app.handle_key_event(ctrl('q'));
        assert!(app.should_quit);
    }

    /// JOURNEY-03: `:wipe` checks the password, asks once more (Ctrl+Y only),
    /// then deletes everything and quits.
    #[tokio::test]
    async fn wipe_checks_the_password_confirms_then_deletes_everything() {
        let dir = tempfile::tempdir().unwrap();
        // Generated, not written out: the test needs a right and a wrong one.
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

        app.execute_command(TuiCommand::Wipe {
            password: wrong.clone(),
        })
        .await;
        assert_eq!(app.overlay, TuiOverlay::None);
        assert!(app.identity_path.exists());

        app.execute_command(TuiCommand::Wipe {
            password: password.clone(),
        })
        .await;
        assert_eq!(app.overlay, TuiOverlay::ConfirmWipe);
        app.handle_key_event(key('y')); // plain y cancels
        assert_eq!(app.overlay, TuiOverlay::None);
        assert!(app.identity_path.exists() && !app.should_quit);

        app.execute_command(TuiCommand::Wipe {
            password: password.clone(),
        })
        .await;
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

    /// PERF-03: autosave rewrote the whole history every 30 s even when
    /// nothing had changed. An unchanged state is not written again.
    #[test]
    fn autosave_skips_an_unchanged_history() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app();
        app.history_path = dir.path().join("history.json.enc");
        // `TuiApp::new` reads the machine's real data dir; whatever it found
        // there is not this test's business.
        app.chat_manager.history_load_error = None;
        let mut key = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut key);
        app.chat_manager.set_history_key(key);
        app.chat_manager
            .create_local_chat_for_test(Uuid::new_v4(), "A".into());

        app.last_save = None;
        app.autosave();
        assert!(app.history_path.exists(), "the first save is written");
        std::fs::remove_file(&app.history_path).unwrap();

        // Due again, nothing changed: no write.
        app.last_save = None;
        app.autosave();
        assert!(
            !app.history_path.exists(),
            "an unchanged history was rewritten"
        );

        // A change is written.
        app.chat_manager
            .create_local_chat_for_test(Uuid::new_v4(), "B".into());
        app.last_save = None;
        app.autosave();
        assert!(app.history_path.exists());
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
                delivered: false,
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
