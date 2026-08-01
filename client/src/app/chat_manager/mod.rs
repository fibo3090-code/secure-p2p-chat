//! Chat management and application state orchestration.
//!
//! Provides the `ChatManager` which coordinates:
//! - Contacts and chats lifecycle (create, rename, trust)
//! - Network sessions and event handling (`SessionEvent`)
//! - Message routing and typing indicators
//! - File transfer state and toasts/notifications
//! - Invite link generation and parsing (including QR codes)
//!
//! The implementation is split by concern: [`connect`] (sessions), [`contacts`],
//! [`events`] (session-event pump), [`files`] (transfers), [`invites`], and
//! [`text`] (messaging). This file holds the state, constructor, and small
//! accessors.

use anyhow::{anyhow, bail, Result};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::core::ProtocolMessage;
use crate::network::{
    generate_relay_token, run_client_session_multi, run_client_session_via_relay, run_host_session,
    run_host_session_via_relay,
};
use crate::transfer::IncomingFileSync;
use crate::types::*;
use rsa::RsaPrivateKey;
use text::IncomingTextMessage;

/// Session handle for communication with network task
#[derive(Clone)]
pub struct SessionHandle {
    /// Unbounded control lane: text, typing, and file-transfer control frames
    /// (`FileMeta` is sent on the file lane; `FileCancel` on this one).
    pub from_app_tx: mpsc::UnboundedSender<ProtocolMessage>,
    /// Bounded bulk-data lane for outgoing `FileChunk`/`FileMeta`/`FileEnd`
    /// frames, so a large send is paced by the network instead of buffering the
    /// whole file in the outbound queue.
    pub file_tx: mpsc::Sender<ProtocolMessage>,
}

/// Capacity (in chunks) of the bounded outgoing file-data lane. At
/// `FILE_CHUNK_SIZE` (64 KiB) this caps in-flight outbound file data at a few
/// hundred KiB regardless of file size, providing real backpressure.
pub(crate) const FILE_LANE_CAPACITY: usize = 8;

impl SessionHandle {
    /// Test-support constructor for a handle whose bulk file lane is inert (its
    /// receiver is dropped). For tests that exercise control-lane messaging and
    /// never drive `send_file`.
    pub fn for_test_control(from_app_tx: mpsc::UnboundedSender<ProtocolMessage>) -> Self {
        let (file_tx, _file_rx) = mpsc::channel(1);
        Self {
            from_app_tx,
            file_tx,
        }
    }
}

/// Live control handle for an in-flight outgoing file transfer. The chunk
/// streaming runs in a background task; this lets the manager observe progress
/// and request cancellation without holding the manager lock for the whole
/// (potentially multi-gigabyte) send.
pub(crate) struct OutgoingTransfer {
    /// Session the file is streaming on (where `FileSendComplete` will arrive).
    pub session_id: Uuid,
    /// Set to request the streaming task stop; it then emits `FileCancel`.
    pub cancel: Arc<std::sync::atomic::AtomicBool>,
    /// Bytes handed to the transport so far (mirrored into `active_transfers`).
    pub progress: Arc<std::sync::atomic::AtomicU64>,
    /// Set by the streaming task on a local I/O error (open/read) so the poll
    /// loop can mark the transfer `Failed` and drop it instead of leaving a
    /// stuck row and a leaked handle.
    pub failed: Arc<std::sync::atomic::AtomicBool>,
}

/// A TOFU verification the UI must resolve before a session becomes `Ready`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingFingerprint {
    /// 64-char hex fingerprint of the peer's identity key.
    pub fingerprint: String,
    /// Peer display name / address for the prompt heading.
    pub peer_name: String,
    /// Short authentication string both peers read aloud to compare (empty
    /// for a legacy/mismatch path that carries no session SAS).
    pub sas: String,
    /// Session id the accept/reject decision must be routed to.
    pub session_id: Uuid,
}

/// Main chat manager - orchestrates sessions, messages, and file transfers
pub struct ChatManager {
    pub chats: HashMap<Uuid, Chat>,
    pub contacts: HashMap<Uuid, Contact>,
    /// Map contact_id -> one-to-one chat id (if any). Used to find session/chat for a contact.
    pub contact_to_chat: HashMap<Uuid, Uuid>,
    sessions: HashMap<Uuid, SessionHandle>,
    session_events: HashMap<Uuid, Arc<Mutex<mpsc::UnboundedReceiver<SessionEvent>>>>,
    /// Channels used to confirm fingerprint verification with the running session task
    fingerprint_confirm_senders: HashMap<Uuid, mpsc::UnboundedSender<bool>>,
    /// Map incoming_chat_id -> parent_session_chat_id. Used when host accepts a connection with a different chat_id than the placeholder.
    /// When a message is sent to incoming_chat_id, it's forwarded to the parent_session_chat_id's session.
    chat_id_mapping: HashMap<Uuid, Uuid>,
    active_transfers: HashMap<Uuid, FileTransferState>,
    /// Outgoing files queued on a session but whose final frame has not hit the
    /// wire yet (FIFO per session; resolved by `SessionEvent::FileSendComplete`).
    /// Each entry is `(filename, transfer_id, chat_id, message_id)` so the
    /// completion can map back to its tracked transfer AND register the file
    /// message for a delivery receipt.
    pending_file_sends: HashMap<Uuid, VecDeque<(String, Uuid, Uuid, Uuid)>>,
    /// Outgoing text messages queued on a session but whose frame has not hit
    /// the wire yet (FIFO per session; resolved by `TextSendComplete`).
    /// Entries are `(chat_id, message_id)`.
    pending_text_sends: HashMap<Uuid, VecDeque<(Uuid, Uuid)>>,
    /// Sent messages waiting for the peer's delivery receipt, keyed by
    /// `(session_id, wire seq)` → `(chat_id, message_id)`.
    awaiting_ack: HashMap<(Uuid, u64), (Uuid, Uuid)>,
    /// Live handles for in-flight outgoing transfers, keyed by transfer id.
    /// The streaming task reports bytes sent via `progress` and stops when
    /// `cancel` is set; the poll loop mirrors both into `active_transfers`.
    outgoing_transfers: HashMap<Uuid, OutgoingTransfer>,
    #[allow(dead_code)] // Reserved for future file transfer implementation
    incoming_files: HashMap<Uuid, IncomingFileSync>,
    /// Incoming transfers whose `FileEnd` arrived while still awaiting the
    /// user's acceptance: the spooled file is complete and held (with the
    /// FileEnd's wire seq, for the delivery receipt) until the user accepts
    /// (finalize) or declines (delete).
    pending_file_end: HashMap<Uuid, u64>,
    incoming_text_messages: HashMap<(Uuid, Uuid), IncomingTextMessage>,
    pub toasts: Vec<Toast>,
    pub config: Config,
    pub fingerprint_verification_request: Option<PendingFingerprint>,
    pub history_key: Option<[u8; 32]>,
    /// Tracks if the application intends to be hosting.
    /// Used for auto-rehosting if the placeholder connection is consumed.
    pub is_hosting: bool,
    /// The port the live listener actually bound (which can differ from
    /// `config.listen_port` when the user typed another one in the Host
    /// dialog). This is what "share this address" displays must use.
    pub hosting_port: Option<u16>,
    /// Optional P2P connection password: required from peers when hosting, and
    /// supplied to the host when connecting. Verified inside the encrypted tunnel.
    pub connection_password: Option<String>,
    /// When true, the conversation is locked: no auto-rehost, so no new peer joins.
    pub conversation_locked: bool,
    /// External `host:port` discovered via UPnP when hosting (None until a
    /// mapping succeeds). Preferred over the LAN address in generated invites.
    pub external_address: Option<String>,
    /// In-flight UPnP mapping attempt, resolved by `poll_session_events`.
    pending_upnp:
        Option<tokio::sync::oneshot::Receiver<anyhow::Result<crate::network::nat::MappedAddress>>>,
    /// Dropping this cancels the background port-mapping renewal task, which
    /// then unmaps the router port. `Some` while a UPnP/NAT-PMP mapping is
    /// being maintained for the current hosting session.
    upnp_cancel: Option<tokio::sync::oneshot::Sender<()>>,
    /// What the user can currently see, pushed down by the UI shell.
    /// `ChatManager` is deliberately UI-agnostic and owns no window handle, so
    /// a front-end that knows about focus must report it here — otherwise
    /// "notify when a message arrives in the background" fires for the
    /// conversation the user is reading right now.
    presence: UiPresence,
}

/// What the user is currently looking at. Reported by the UI shell via
/// [`ChatManager::set_ui_presence`]; used to decide whether an arriving message
/// is "in the background" and therefore worth a desktop notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiPresence {
    /// The app window has OS focus and is visible.
    pub focused: bool,
    /// The conversation currently on screen, if any.
    pub active_chat: Option<Uuid>,
}

impl Default for UiPresence {
    /// A front-end that never reports presence behaves like an unfocused shell:
    /// every message notifies. That is the safe default — missing a
    /// notification is worse than one extra, and the alternative (assume
    /// focused) would silently disable notifications for such a front-end.
    fn default() -> Self {
        Self {
            focused: false,
            active_chat: None,
        }
    }
}

impl ChatManager {
    pub fn new(config: Config) -> Self {
        Self {
            chats: HashMap::new(),
            contacts: HashMap::new(),
            contact_to_chat: HashMap::new(),
            sessions: HashMap::new(),
            session_events: HashMap::new(),
            chat_id_mapping: HashMap::new(),
            active_transfers: HashMap::new(),
            pending_file_sends: HashMap::new(),
            pending_text_sends: HashMap::new(),
            awaiting_ack: HashMap::new(),
            outgoing_transfers: HashMap::new(),
            incoming_files: HashMap::new(),
            pending_file_end: HashMap::new(),
            incoming_text_messages: HashMap::new(),
            toasts: Vec::new(),
            config,
            fingerprint_verification_request: None,
            fingerprint_confirm_senders: HashMap::new(),
            history_key: None,
            is_hosting: false,
            hosting_port: None,
            connection_password: None,
            conversation_locked: false,
            external_address: None,
            pending_upnp: None,
            upnp_cancel: None,
            presence: UiPresence::default(),
        }
    }

    /// Report what the user can see. Call this from the UI shell whenever the
    /// window gains/loses focus or the open conversation changes.
    pub fn set_ui_presence(&mut self, focused: bool, active_chat: Option<Uuid>) {
        self.presence = UiPresence {
            focused,
            active_chat,
        };
    }

    /// The last reported UI presence.
    pub fn ui_presence(&self) -> UiPresence {
        self.presence
    }

    /// Whether an arriving message in `chat_id` warrants a desktop
    /// notification: notifications must be enabled, and the message must not be
    /// landing in the conversation the user is looking at right now.
    ///
    /// Public so the gate itself is testable without firing an OS popup.
    pub fn should_notify_for(&self, chat_id: Uuid) -> bool {
        if !self.config.enable_notifications {
            return false;
        }
        let id = self.resolve_display_chat_id(chat_id);
        !(self.presence.focused && self.presence.active_chat == Some(id))
    }

    /// Mark every message currently in a conversation as seen. The read mark
    /// lives on the `Chat` and is persisted with the encrypted history, so
    /// unread badges survive a restart.
    pub fn mark_chat_read(&mut self, chat_id: Uuid) {
        // Incoming connections are tracked under the *incoming* chat id; accept
        // either that or the session id so callers do not have to know which.
        let id = self.resolve_display_chat_id(chat_id);
        if let Some(chat) = self.chats.get_mut(&id) {
            chat.mark_read();
        }
    }

    /// Unseen messages from the peer in a conversation.
    pub fn unread_count(&self, chat_id: Uuid) -> usize {
        self.chats
            .get(&self.resolve_display_chat_id(chat_id))
            .map(|c| c.unread_count())
            .unwrap_or(0)
    }

    /// Total unseen messages across every conversation (rail badge / tray).
    pub fn total_unread(&self) -> usize {
        self.chats.values().map(|c| c.unread_count()).sum()
    }

    /// Map a session id back to the chat the UI actually displays. For an
    /// incoming connection the host stores messages under the client's chat id,
    /// so the session id alone would resolve to nothing.
    fn resolve_display_chat_id(&self, chat_id: Uuid) -> Uuid {
        if self.chats.contains_key(&chat_id) {
            return chat_id;
        }
        self.chat_id_mapping
            .iter()
            .find(|(_, &session_id)| session_id == chat_id)
            .map(|(&incoming_id, _)| incoming_id)
            .unwrap_or(chat_id)
    }

    /// Set the optional P2P connection password. When hosting, peers must present
    /// it; when connecting, it is the password supplied to the host. Set this
    /// before calling `start_host` / `connect_to_host`.
    pub fn set_connection_password(&mut self, password: Option<String>) {
        self.connection_password = password.filter(|p| !p.is_empty());
    }

    /// Whether a connection password is currently configured.
    pub fn has_connection_password(&self) -> bool {
        self.connection_password.is_some()
    }

    /// Lock or unlock the conversation. While locked, the host does not auto-rehost,
    /// so no new peer can connect (the caller typically also stops hosting).
    pub fn set_conversation_locked(&mut self, locked: bool) {
        self.conversation_locked = locked;
    }

    pub fn is_conversation_locked(&self) -> bool {
        self.conversation_locked
    }

    /// Check if we need to re-host (i.e. we want to be hosting, but no placeholder host exists).
    /// Returns true if the caller should spawn a task to call `start_host`.
    pub fn check_rehost_needed(&self) -> bool {
        if !self.is_hosting || self.conversation_locked {
            return false;
        }
        // If we want to host, but no chat is a placeholder host, we need to restart.
        !self.chats.values().any(|c| c.is_host_placeholder)
    }

    /// Stop hosting (user action).
    pub fn stop_hosting(&mut self) {
        self.is_hosting = false;
        self.hosting_port = None;
        // Cancel the UPnP renewal task (it unmaps the router port on drop) and
        // forget the external address so invites revert to the LAN address.
        self.upnp_cancel = None;
        self.pending_upnp = None;
        self.external_address = None;
        // Remove placeholder if exists
        if let Some(id) = self
            .chats
            .values()
            .find(|c| c.is_host_placeholder)
            .map(|c| c.id)
        {
            self.toasts.push(Toast {
                id: Uuid::new_v4(),
                level: ToastLevel::Info,
                message: "Stopped hosting".to_string(),
                created_at: std::time::Instant::now(),
                duration: Duration::from_secs(3),
            });
            self.chats.remove(&id);
            self.sessions.remove(&id);
            self.session_events.remove(&id);
            self.fingerprint_confirm_senders.remove(&id);
        }
    }

    /// Provide the history encryption key after the identity is unlocked.
    pub fn set_history_key(&mut self, key: [u8; 32]) {
        self.history_key = Some(key);
    }

    /// Helper to create a local chat for testing purposes without network sessions
    pub fn create_local_chat_for_test(&mut self, id: Uuid, title: String) {
        let chat = Chat {
            id,
            title,
            kind: ChatKind::Dm,
            transport: Transport::Direct,
            peer_fingerprint: None,
            participants: Vec::new(),
            messages: Vec::new(),
            created_at: chrono::Utc::now(),
            peer_typing: false,
            typing_since: None,
            send_seq: 0,
            recv_seq: 0,
            is_host_placeholder: false,
            read_count: 0,
        };
        self.chats.insert(id, chat);
    }

    /// Helper to inject a mock session for testing
    pub fn add_session_for_test(&mut self, chat_id: Uuid, handle: SessionHandle) {
        self.sessions.insert(chat_id, handle);
    }

    /// Helper to inject a fingerprint-confirmation channel for testing the
    /// verification flow without a live network session.
    pub fn add_fingerprint_confirm_sender_for_test(
        &mut self,
        chat_id: Uuid,
        tx: mpsc::UnboundedSender<bool>,
    ) {
        self.fingerprint_confirm_senders.insert(chat_id, tx);
    }

    /// Rename a conversation/chat
    pub fn rename_chat(&mut self, chat_id: Uuid, new_title: String) -> Result<()> {
        // Truncate by characters (not bytes) so multi-byte/emoji titles never
        // panic on a non-char-boundary slice.
        let title: String = new_title.chars().take(50).collect();

        if let Some(chat) = self.chats.get_mut(&chat_id) {
            chat.title = title;
            Ok(())
        } else {
            tracing::error!("Chat not found for rename: {}", chat_id);
            Err(anyhow::anyhow!("Chat not found"))
        }
    }

    /// Add a toast notification
    pub fn add_toast(&mut self, level: ToastLevel, message: String) {
        self.toasts.push(Toast {
            id: Uuid::new_v4(),
            level,
            message,
            created_at: std::time::Instant::now(),
            duration: Duration::from_secs(4),
        });
    }

    /// Remove expired toasts
    pub fn cleanup_expired_toasts(&mut self) {
        let now = std::time::Instant::now();
        self.toasts
            .retain(|toast| now.duration_since(toast.created_at) < toast.duration);
    }

    /// Desktop notification for a message that arrived in `chat_id`, titled
    /// with the conversation name so the user knows who it is from without
    /// opening the app.
    ///
    /// Suppressed when the user is already looking at that conversation — the
    /// setting promises "notify when a message arrives in the background", and
    /// an OS popup for the thread on screen is the fastest way to make a user
    /// turn notifications off for good.
    pub(super) fn notify_incoming_message(&self, chat_id: Uuid, body: &str) {
        if !self.should_notify_for(chat_id) {
            return;
        }
        let title = self
            .chats
            .get(&chat_id)
            .map(|c| c.title.as_str())
            .filter(|t| !t.is_empty())
            .unwrap_or("New message");
        self.show_notification(title, body);
    }

    /// Show desktop notification
    pub fn show_notification(&self, title: &str, body: &str) {
        if !self.config.enable_notifications {
            return;
        }

        #[cfg(not(target_os = "linux"))]
        {
            use notify_rust::Notification;
            let _ = Notification::new()
                .summary(title)
                .body(body)
                .icon("mail-message-new")
                .timeout(5000)
                .show();
        }

        #[cfg(target_os = "linux")]
        {
            use notify_rust::{Notification, Timeout};
            let _ = Notification::new()
                .summary(title)
                .body(body)
                .icon("mail-message-new")
                .timeout(Timeout::Milliseconds(5000))
                .show();
        }
    }

    /// Get a chat by ID
    pub fn get_chat(&self, chat_id: Uuid) -> Option<&Chat> {
        self.chats.get(&chat_id)
    }

    /// Get a mutable chat by ID
    pub fn get_chat_mut(&mut self, chat_id: Uuid) -> Option<&mut Chat> {
        self.chats.get_mut(&chat_id)
    }

    /// Get the number of active sessions.
    pub fn sessions_len(&self) -> usize {
        self.sessions.len()
    }

    /// Check if a chat has an active session.
    pub fn is_connected(&self, chat_id: &Uuid) -> bool {
        let actual_id = self.chat_id_mapping.get(chat_id).unwrap_or(chat_id);
        self.sessions.contains_key(actual_id)
    }

    /// Get all chat IDs
    pub fn chat_ids(&self) -> Vec<Uuid> {
        self.chats.keys().copied().collect()
    }

    /// Snapshot of all tracked file transfers (for read-only UI display).
    pub fn active_transfers_snapshot(&self) -> Vec<FileTransferState> {
        self.active_transfers.values().cloned().collect()
    }

    /// Queue a delivery receipt on the session serving `chat_id`, acknowledging
    /// the peer's frame with wire seq `acked_seq`. Best-effort: no session, no
    /// receipt (the peer's message simply stays unmarked on their side). The
    /// frame's own seq is stamped by the session loop like every message.
    pub(crate) fn send_ack_for_chat(&self, chat_id: Uuid, acked_seq: u64) {
        let session_id = *self.chat_id_mapping.get(&chat_id).unwrap_or(&chat_id);
        if let Some(session) = self.sessions.get(&session_id) {
            let _ = session
                .from_app_tx
                .send(ProtocolMessage::Ack { acked_seq, seq: 0 });
        }
    }

    /// Transfers sorted by id, so an index-based selection (the TUI overlay) is
    /// stable across polls even though the backing map is unordered.
    pub fn active_transfers_sorted(&self) -> Vec<FileTransferState> {
        let mut v: Vec<_> = self.active_transfers.values().cloned().collect();
        v.sort_by_key(|t| t.id);
        v
    }

    /// Delete a chat and its associated session
    pub fn delete_chat(&mut self, chat_id: Uuid) {
        tracing::info!(chat_id = %chat_id, "Deleting chat");
        self.chats.remove(&chat_id);
        self.sessions.remove(&chat_id);
        self.session_events.remove(&chat_id);
        self.fingerprint_confirm_senders.remove(&chat_id);
        self.add_toast(ToastLevel::Info, "Chat deleted".to_string());
        tracing::debug!(remaining_chats = %self.chats.len(), remaining_sessions = %self.sessions.len(), "Chat deleted");
    }

    /// Clear all chat history and contacts
    pub fn clear_history(&mut self, history_path: &std::path::Path) {
        tracing::warn!(
            chats = %self.chats.len(),
            contacts = %self.contacts.len(),
            sessions = %self.sessions.len(),
            "Clearing all history and state"
        );
        self.chats.clear();
        self.contacts.clear();
        self.contact_to_chat.clear();
        self.sessions.clear();
        self.session_events.clear();
        self.fingerprint_confirm_senders.clear();
        // Signal any streaming send tasks to stop, then drop all transfer state.
        for handle in self.outgoing_transfers.values() {
            handle
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.outgoing_transfers.clear();
        self.pending_file_sends.clear();
        self.active_transfers.clear();
        self.incoming_files.clear();
        self.pending_file_end.clear();
        self.pending_text_sends.clear();
        self.awaiting_ack.clear();
        self.incoming_text_messages.clear();
        self.toasts.clear();
        self.fingerprint_verification_request = None;

        if !history_path.as_os_str().is_empty() && self.history_key.is_some() {
            let _ = self.save_history(history_path);
            tracing::info!("History cleared and saved");
        } else {
            tracing::info!("History cleared in memory");
        }
    }

    /// Delete all data including identity file. Used for complete data wipe.
    pub fn delete_all_data(
        &mut self,
        data_dir: &std::path::Path,
        history_path: &std::path::Path,
        identity_path: &std::path::Path,
    ) -> Result<()> {
        tracing::warn!("Deleting ALL data including identity");

        // First clear all in-memory state
        self.clear_history(std::path::Path::new(""));

        let history_under_data_dir = history_path.parent() == Some(data_dir);
        let identity_under_data_dir = identity_path.parent() == Some(data_dir);
        if !data_dir.as_os_str().is_empty()
            && history_under_data_dir
            && identity_under_data_dir
            && data_dir.exists()
        {
            std::fs::remove_dir_all(data_dir)?;
            tracing::info!("App data directory deleted: {}", data_dir.display());
            return Ok(());
        }

        if history_path.exists() {
            std::fs::remove_file(history_path)?;
            tracing::info!("History file deleted: {}", history_path.display());
        }

        if identity_path.exists() {
            std::fs::remove_file(identity_path)?;
            tracing::info!("Identity file deleted: {}", identity_path.display());
        }

        Ok(())
    }
}

impl Default for ChatManager {
    fn default() -> Self {
        Self::new(Config::default())
    }
}

mod connect;
mod contacts;
mod events;
mod files;
mod invites;
mod text;

#[cfg(test)]
mod tests;
