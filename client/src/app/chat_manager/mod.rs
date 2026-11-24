//! `ChatManager`: app state (chats, contacts, sessions, transfers, toasts) and
//! the orchestration around it. Split by concern into [`connect`], [`contacts`],
//! [`events`], [`files`], [`invites`] and [`text`]; this file holds the state
//! and small accessors.

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
use rsa::PrivateKey as RsaPrivateKey;
use text::IncomingTextMessage;

/// Session handle for communication with the network task.
#[derive(Clone)]
pub struct SessionHandle {
    /// Unbounded control lane: text, typing, `FileCancel`.
    pub from_app_tx: mpsc::UnboundedSender<ProtocolMessage>,
    /// Bounded bulk lane (`FileMeta`/`FileChunk`/`FileEnd`), so a large send is
    /// paced by the network instead of buffered whole.
    pub file_tx: mpsc::Sender<ProtocolMessage>,
}

/// Consecutive failed binds before the manager gives up on hosting (auto-rehost
/// is a timer and would otherwise retry a taken port forever).
pub(crate) const MAX_CONSECUTIVE_REHOST_FAILURES: u32 = 3;

/// Capacity (in chunks) of the bounded outgoing file-data lane.
pub(crate) const FILE_LANE_CAPACITY: usize = 8;

impl SessionHandle {
    /// Test-support constructor whose bulk file lane is inert (receiver dropped).
    pub fn for_test_control(from_app_tx: mpsc::UnboundedSender<ProtocolMessage>) -> Self {
        let (file_tx, _file_rx) = mpsc::channel(1);
        Self {
            from_app_tx,
            file_tx,
        }
    }
}

/// Live control handle for an in-flight outgoing transfer, streamed by a
/// background task so a large send never holds the manager lock.
pub(crate) struct OutgoingTransfer {
    /// Session the file is streaming on (where `FileSendComplete` arrives).
    pub session_id: Uuid,
    /// Set to request the streaming task stop; it then emits `FileCancel`.
    pub cancel: Arc<std::sync::atomic::AtomicBool>,
    /// Bytes handed to the transport so far (mirrored into `active_transfers`).
    pub progress: Arc<std::sync::atomic::AtomicU64>,
    /// Set by the streaming task on a local I/O error so the poll loop marks
    /// the transfer `Failed`.
    pub failed: Arc<std::sync::atomic::AtomicBool>,
}

/// An outgoing file whose final frame has not reached the wire yet, resolved
/// FIFO per session by `SessionEvent::FileSendComplete`.
pub(crate) struct PendingFileSend {
    pub filename: String,
    pub transfer_id: Uuid,
    pub chat_id: Uuid,
    pub message_id: Uuid,
}

/// A TOFU verification the UI must resolve before a session becomes `Ready`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingFingerprint {
    /// 64-char hex fingerprint of the peer's identity key.
    pub fingerprint: String,
    /// Peer display name / address for the prompt heading.
    pub peer_name: String,
    /// Short authentication string (empty on a legacy path with no SAS).
    pub sas: String,
    /// Session id the accept/reject decision must be routed to.
    pub session_id: Uuid,
    /// Why this prompt is not an ordinary first use, if it is not. A UI must
    /// lead with it: the SAS still decides, but a user who was told "this is
    /// not the key you expected" compares it very differently from one who
    /// was told nothing.
    pub warning: Option<FingerprintWarning>,
}

/// What makes a TOFU prompt a warning rather than a first-use question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FingerprintWarning {
    /// This conversation already trusted a different key.
    KeyChanged {
        /// The fingerprint the conversation stored.
        previous: String,
    },
    /// The peer was dialled as a contact whose invite named a different key.
    ///
    /// The invite's fingerprint is only ever the *expected* value — an invite
    /// is not verification — but a different key answering where it should
    /// have been is exactly what an interposed attacker looks like.
    InviteMismatch {
        contact_id: Uuid,
        contact_name: String,
        /// The fingerprint the contact's invite named.
        expected: String,
    },
}

/// The fingerprint a session is expected to present, because the user dialled
/// it as a contact that carries one. Session-only: never persisted, never
/// trusted on its own.
#[derive(Clone, Debug)]
pub(super) struct ExpectedPeer {
    pub(super) contact_id: Uuid,
    pub(super) contact_name: String,
    pub(super) fingerprint: String,
}

/// Main chat manager - orchestrates sessions, messages, and file transfers
#[derive(Default)]
pub struct ChatManager {
    pub chats: HashMap<Uuid, Chat>,
    pub contacts: HashMap<Uuid, Contact>,
    /// contact_id -> one-to-one chat id.
    pub contact_to_chat: HashMap<Uuid, Uuid>,
    sessions: HashMap<Uuid, SessionHandle>,
    /// The task driving each session, aborted on teardown: dropping the handle
    /// alone never wakes a host parked in `accept()` (it keeps the port bound).
    session_tasks: HashMap<Uuid, tokio::task::JoinHandle<()>>,
    session_events: HashMap<Uuid, Arc<Mutex<mpsc::UnboundedReceiver<SessionEvent>>>>,
    /// Confirms fingerprint verification with the running session task.
    fingerprint_confirm_senders: HashMap<Uuid, mpsc::UnboundedSender<bool>>,
    /// incoming_chat_id -> session id (the host placeholder the peer connected
    /// through).
    chat_id_mapping: HashMap<Uuid, Uuid>,
    active_transfers: HashMap<Uuid, FileTransferState>,
    /// Outgoing files whose final frame has not hit the wire yet, FIFO per session.
    pending_file_sends: HashMap<Uuid, VecDeque<PendingFileSend>>,
    /// Outgoing texts awaiting `TextSendComplete`, FIFO per session:
    /// `(chat_id, message_id)`.
    pending_text_sends: HashMap<Uuid, VecDeque<(Uuid, Uuid)>>,
    /// Sent messages awaiting the peer's receipt:
    /// `(session_id, wire seq)` → `(chat_id, message_id)`.
    awaiting_ack: HashMap<(Uuid, u64), (Uuid, Uuid)>,
    /// Live handles for in-flight outgoing transfers, keyed by transfer id.
    outgoing_transfers: HashMap<Uuid, OutgoingTransfer>,
    /// Open spool files for incoming transfers, keyed by transfer id.
    incoming_files: HashMap<Uuid, IncomingFileSync>,
    /// Held incoming offers whose `FileEnd` already arrived (value: its wire
    /// seq, for the receipt), until accepted or declined.
    pending_file_end: HashMap<Uuid, u64>,
    /// chat id -> the incoming transfer its wire is streaming into (set by
    /// `FileMeta`, cleared by `FileEnd`/`FileCancel`). `FileChunk` carries no
    /// transfer id, so this — never "the active transfer" — routes chunks.
    incoming_streams: HashMap<Uuid, Uuid>,
    incoming_text_messages: HashMap<(Uuid, Uuid), IncomingTextMessage>,
    pub toasts: Vec<Toast>,
    pub config: Config,
    /// TOFU prompts waiting on the user, oldest first. A queue, since a host can
    /// have several peers mid-handshake. Removed only by `confirm_fingerprint`
    /// or when the session dies — never by reading.
    fingerprint_verification_requests: VecDeque<PendingFingerprint>,
    /// The fingerprint each outgoing session to a contact should present,
    /// keyed by session id (see [`ExpectedPeer`]). Set by
    /// `connect_to_contact`, dropped by `teardown_session`. A different key
    /// answering takes the warning path in `handle_tofu_verification`; a
    /// matching one still gets the ordinary SAS prompt.
    pub(super) expected_fingerprints: HashMap<Uuid, ExpectedPeer>,
    pub history_key: Option<[u8; 32]>,
    /// Why the history file could not be loaded. While set, nothing may be
    /// written over it (the empty state is only the failed load's). Cleared by
    /// `set_aside_unreadable_history` or a later successful load.
    pub(crate) history_load_error: Option<String>,
    /// Set by `delete_all_data`; from then on every save is refused so a late
    /// flush cannot recreate the history.
    pub(crate) data_wiped: bool,
    /// Shared with every snapshot so a background write cannot outlive a wipe.
    pub(crate) persist_gate: crate::app::persistence::PersistGate,
    /// The app intends to be hosting (drives auto-rehost).
    pub is_hosting: bool,
    /// Consecutive failed binds (see [`MAX_CONSECUTIVE_REHOST_FAILURES`]).
    rehost_failures: u32,
    /// The port the live listener actually bound (may differ from
    /// `config.listen_port`); what "share this address" must use.
    pub hosting_port: Option<u16>,
    /// Optional P2P connection password (required when hosting, supplied when
    /// connecting), verified inside the tunnel.
    pub connection_password: Option<String>,
    /// While locked there is no auto-rehost, so no new peer joins.
    pub conversation_locked: bool,
    /// External `host:port` from UPnP, preferred over the LAN address in invites.
    pub external_address: Option<String>,
    /// In-flight UPnP mapping attempt, resolved by `poll_session_events`.
    pending_upnp:
        Option<tokio::sync::oneshot::Receiver<anyhow::Result<crate::network::nat::MappedAddress>>>,
    /// Dropping this cancels the port-mapping renewal task (which unmaps).
    upnp_cancel: Option<tokio::sync::oneshot::Sender<()>>,
    /// What the user can see, pushed down by the UI shell (the manager owns no
    /// window), so notifications skip the conversation on screen.
    presence: UiPresence,
    /// This device's fingerprint, pushed down by the shell, so importing the
    /// user's own invite is refused.
    my_fingerprint: Option<String>,
}

/// What the user is currently looking at (see [`ChatManager::set_ui_presence`]).
/// The default is "unfocused": a front-end that never reports presence keeps
/// notifying, since a missing notification is worse than an extra one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UiPresence {
    /// The app window has OS focus and is visible.
    pub focused: bool,
    /// The conversation currently on screen, if any.
    pub active_chat: Option<Uuid>,
}

impl ChatManager {
    pub fn new(config: Config) -> Self {
        let mut manager = Self::default();
        manager.config = config;
        manager
    }

    /// Tell the manager which identity this device runs as.
    pub fn set_my_fingerprint(&mut self, fingerprint: impl Into<String>) {
        self.my_fingerprint = Some(fingerprint.into());
    }

    /// Report what the user can see (focus / open conversation changes).
    pub fn set_ui_presence(&mut self, focused: bool, active_chat: Option<Uuid>) {
        self.presence = UiPresence {
            focused,
            active_chat,
        };
    }

    pub fn ui_presence(&self) -> UiPresence {
        self.presence
    }

    /// Whether an arriving message in `chat_id` warrants a desktop notification:
    /// enabled, and not for the conversation on screen.
    pub fn should_notify_for(&self, chat_id: Uuid) -> bool {
        if !self.config.enable_notifications {
            return false;
        }
        let id = self.resolve_display_chat_id(chat_id);
        !(self.presence.focused && self.presence.active_chat == Some(id))
    }

    /// Mark every message in a conversation as seen (persisted on the `Chat`).
    pub fn mark_chat_read(&mut self, chat_id: Uuid) {
        // Accept either the incoming chat id or the session id.
        let id = self.resolve_display_chat_id(chat_id);
        if let Some(chat) = self.chats.get_mut(&id) {
            chat.mark_read();
        }
    }

    pub fn unread_count(&self, chat_id: Uuid) -> usize {
        self.chats
            .get(&self.resolve_display_chat_id(chat_id))
            .map(|c| c.unread_count())
            .unwrap_or(0)
    }

    /// Total unseen messages across every conversation.
    pub fn total_unread(&self) -> usize {
        self.chats.values().map(|c| c.unread_count()).sum()
    }

    /// Map a session id back to the chat the UI displays. An id that already
    /// names a chat wins, so UI callers may pass either end of the mapping.
    pub fn resolve_display_chat_id(&self, chat_id: Uuid) -> Uuid {
        if self.chats.contains_key(&chat_id) {
            return chat_id;
        }
        self.chat_for_session(chat_id)
    }

    /// The session serving a conversation (host-side incoming chats map to
    /// their placeholder's session via `chat_id_mapping`).
    pub(super) fn session_for_chat(&self, chat_id: Uuid) -> Uuid {
        self.chat_id_mapping
            .get(&chat_id)
            .copied()
            .unwrap_or(chat_id)
    }

    /// The conversation a session's events belong to.
    pub(super) fn chat_for_session(&self, session_id: Uuid) -> Uuid {
        self.chat_id_mapping
            .iter()
            .find(|(_, &sid)| sid == session_id)
            .map(|(&chat_id, _)| chat_id)
            .unwrap_or(session_id)
    }

    /// Queue a control frame on the session serving `chat_id` (its seq is a
    /// placeholder the session loop stamps). `false` when there is no session.
    pub(super) fn send_control(&self, chat_id: Uuid, frame: ProtocolMessage) -> bool {
        self.sessions
            .get(&self.session_for_chat(chat_id))
            .is_some_and(|s| s.from_app_tx.send(frame).is_ok())
    }

    /// The TOFU prompt to answer next. Peeks — never consumes, so a polling UI
    /// cannot swallow a prompt and `confirm_fingerprint` can still persist it.
    pub fn pending_fingerprint(&self) -> Option<&PendingFingerprint> {
        self.fingerprint_verification_requests.front()
    }

    pub fn pending_fingerprint_count(&self) -> usize {
        self.fingerprint_verification_requests.len()
    }

    /// Queue a TOFU prompt; a newer one for the same session replaces the older.
    pub(super) fn queue_fingerprint_request(&mut self, request: PendingFingerprint) {
        if let Some(slot) = self
            .fingerprint_verification_requests
            .iter_mut()
            .find(|p| p.session_id == request.session_id)
        {
            *slot = request;
            return;
        }
        self.fingerprint_verification_requests.push_back(request);
    }

    /// Remove (and return) the queued prompt for a session.
    pub(super) fn take_fingerprint_request(
        &mut self,
        session_id: Uuid,
    ) -> Option<PendingFingerprint> {
        let idx = self
            .fingerprint_verification_requests
            .iter()
            .position(|p| p.session_id == session_id)?;
        self.fingerprint_verification_requests.remove(idx)
    }

    /// Drop queued prompts of a dead session so they cannot block the queue.
    pub(super) fn drop_fingerprint_requests_for_session(&mut self, session_id: Uuid) {
        self.fingerprint_verification_requests
            .retain(|p| p.session_id != session_id);
    }

    /// Set the optional P2P connection password (before `start_host` /
    /// `connect_to_host`).
    pub fn set_connection_password(&mut self, password: Option<String>) {
        self.connection_password = password.filter(|p| !p.is_empty());
    }

    pub fn has_connection_password(&self) -> bool {
        self.connection_password.is_some()
    }

    /// Lock or unlock the conversation (locked: no auto-rehost).
    pub fn set_conversation_locked(&mut self, locked: bool) {
        self.conversation_locked = locked;
    }

    pub fn is_conversation_locked(&self) -> bool {
        self.conversation_locked
    }

    /// Whether the caller should spawn `start_host` again (we want to host but
    /// no placeholder exists).
    pub fn check_rehost_needed(&self) -> bool {
        if !self.is_hosting || self.conversation_locked {
            return false;
        }
        // Only a direct listener (`hosting_port`) is auto-rehosted: a dead relay
        // rendezvous must not be silently replaced by a TCP listener.
        if self.hosting_port.is_none() {
            return false;
        }
        !self.chats.values().any(|c| c.is_host_placeholder)
    }

    /// Stop hosting (user action). Drops the UPnP mapping and aborts the
    /// listener task, which otherwise holds the port.
    pub fn stop_hosting(&mut self) {
        self.is_hosting = false;
        self.rehost_failures = 0;
        self.hosting_port = None;
        self.upnp_cancel = None;
        self.pending_upnp = None;
        self.external_address = None;
        if let Some(id) = self.host_placeholder_id() {
            self.add_toast(ToastLevel::Info, "Stopped hosting".to_string());
            self.remove_host_placeholder(id);
        }
    }

    pub fn set_history_key(&mut self, key: [u8; 32]) {
        self.history_key = Some(key);
    }

    /// Test helper: a local chat without a network session.
    pub fn create_local_chat_for_test(&mut self, id: Uuid, title: String) {
        self.chats
            .insert(id, Chat::new(id, title, ChatKind::Dm, Transport::Direct));
    }

    /// Test helper: inject a mock session.
    pub fn add_session_for_test(&mut self, chat_id: Uuid, handle: SessionHandle) {
        self.sessions.insert(chat_id, handle);
    }

    /// Test helper: inject a fingerprint-confirmation channel.
    pub fn add_fingerprint_confirm_sender_for_test(
        &mut self,
        chat_id: Uuid,
        tx: mpsc::UnboundedSender<bool>,
    ) {
        self.fingerprint_confirm_senders.insert(chat_id, tx);
    }

    /// Test helper: queue a TOFU prompt the way a handshake would.
    pub fn queue_fingerprint_prompt_for_test(&mut self, prompt: PendingFingerprint) {
        self.queue_fingerprint_request(prompt);
    }

    /// Rename a conversation (marks the title custom so `Connected` keeps it).
    pub fn rename_chat(&mut self, chat_id: Uuid, new_title: String) -> Result<()> {
        // Truncate by chars so multi-byte titles never split.
        let title: String = new_title.trim().chars().take(50).collect();
        if title.is_empty() {
            bail!("A conversation name cannot be empty");
        }

        if let Some(chat) = self.chats.get_mut(&chat_id) {
            chat.title = title;
            chat.title_is_custom = true;
            Ok(())
        } else {
            tracing::error!("Chat not found for rename: {}", chat_id);
            Err(anyhow::anyhow!("Chat not found"))
        }
    }

    pub fn add_toast(&mut self, level: ToastLevel, message: String) {
        self.toasts.push(Toast {
            id: Uuid::new_v4(),
            level,
            message,
            created_at: std::time::Instant::now(),
            duration: Duration::from_secs(4),
        });
    }

    pub fn cleanup_expired_toasts(&mut self) {
        let now = std::time::Instant::now();
        self.toasts
            .retain(|toast| now.duration_since(toast.created_at) < toast.duration);
    }

    /// Desktop notification for a message in `chat_id`, titled with the
    /// conversation name; suppressed for the conversation on screen.
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

    pub fn show_notification(&self, title: &str, body: &str) {
        if !self.config.enable_notifications {
            return;
        }

        let _ = notify_rust::Notification::new()
            .summary(title)
            .body(body)
            .icon("mail-message-new")
            .timeout(notify_rust::Timeout::Milliseconds(5000))
            .show();
    }

    pub fn get_chat(&self, chat_id: Uuid) -> Option<&Chat> {
        self.chats.get(&chat_id)
    }

    pub fn get_chat_mut(&mut self, chat_id: Uuid) -> Option<&mut Chat> {
        self.chats.get_mut(&chat_id)
    }

    pub fn sessions_len(&self) -> usize {
        self.sessions.len()
    }

    pub fn is_connected(&self, chat_id: &Uuid) -> bool {
        self.sessions.contains_key(&self.session_for_chat(*chat_id))
    }

    pub fn chat_ids(&self) -> Vec<Uuid> {
        self.chats.keys().copied().collect()
    }

    /// Snapshot of all tracked file transfers.
    pub fn active_transfers_snapshot(&self) -> Vec<FileTransferState> {
        self.active_transfers.values().cloned().collect()
    }

    /// Best-effort delivery receipt for the peer's frame `acked_seq`.
    pub(crate) fn send_ack_for_chat(&self, chat_id: Uuid, acked_seq: u64) {
        self.send_control(chat_id, ProtocolMessage::Ack { acked_seq, seq: 0 });
    }

    /// Transfers sorted by id, so index-based selection (TUI) is stable.
    pub fn active_transfers_sorted(&self) -> Vec<FileTransferState> {
        let mut v: Vec<_> = self.active_transfers.values().cloned().collect();
        v.sort_by_key(|t| t.id);
        v
    }

    /// Stop the task driving a session (a host parked in `accept()` never
    /// notices a dropped handle and keeps the port bound).
    fn abort_session_task(&mut self, session_id: Uuid) {
        if let Some(task) = self.session_tasks.remove(&session_id) {
            task.abort();
        }
    }

    /// Stop a session and forget everything keyed by its id (task, channels,
    /// TOFU prompt, pending sends and receipts). Conversations and
    /// `chat_id_mapping` are left to the caller. Every way a session ends goes
    /// through here.
    pub(super) fn teardown_session(&mut self, session_id: Uuid) {
        self.abort_session_task(session_id);
        self.sessions.remove(&session_id);
        self.session_events.remove(&session_id);
        self.fingerprint_confirm_senders.remove(&session_id);
        self.drop_fingerprint_requests_for_session(session_id);
        self.expected_fingerprints.remove(&session_id);
        self.pending_text_sends.remove(&session_id);
        self.pending_file_sends.remove(&session_id);
        self.awaiting_ack.retain(|(sid, _), _| *sid != session_id);
    }

    pub(super) fn remove_host_placeholder(&mut self, id: Uuid) {
        self.chats.remove(&id);
        self.teardown_session(id);
    }

    pub(super) fn host_placeholder_id(&self) -> Option<Uuid> {
        self.chats
            .values()
            .find(|c| c.is_host_placeholder)
            .map(|c| c.id)
    }

    pub(super) fn register_session_task(
        &mut self,
        session_id: Uuid,
        task: tokio::task::JoinHandle<()>,
    ) {
        if let Some(previous) = self.session_tasks.insert(session_id, task) {
            previous.abort();
        }
    }

    /// Delete a chat and close the session behind it, resolving the session
    /// through `chat_id_mapping` (an incoming chat's session is keyed by the
    /// placeholder, so removing only the chat left the socket open).
    pub fn delete_chat(&mut self, chat_id: Uuid) {
        tracing::info!(chat_id = %chat_id, "Deleting chat");
        let session_id = self.session_for_chat(chat_id);
        // Callers may name either end (the TUI's rejection passes the session
        // id), so incoming conversations served by that session go too.
        let incoming: Vec<Uuid> = self
            .chat_id_mapping
            .iter()
            .filter(|(incoming, session)| **session == session_id && **incoming != chat_id)
            .map(|(incoming, _)| *incoming)
            .collect();

        // In-flight transfers on these conversations have nowhere to land.
        self.fail_incoming_transfers(chat_id, "conversation deleted");
        for id in &incoming {
            self.fail_incoming_transfers(*id, "conversation deleted");
            for transfer_id in self.transfer_ids_for_chat(*id) {
                self.clear_transfer_bookkeeping(transfer_id);
            }
            self.chats.remove(id);
        }
        for transfer_id in self.transfer_ids_for_chat(chat_id) {
            self.clear_transfer_bookkeeping(transfer_id);
        }

        self.chats.remove(&chat_id);
        self.teardown_session(session_id);
        self.chat_id_mapping
            .retain(|incoming, session| *incoming != chat_id && *session != session_id);

        self.add_toast(ToastLevel::Info, "Chat deleted".to_string());
        tracing::debug!(remaining_chats = %self.chats.len(), remaining_sessions = %self.sessions.len(), "Chat deleted");
    }

    /// Clear all chat history and contacts.
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
        self.stop_all_tasks();
        self.sessions.clear();
        self.session_events.clear();
        self.chat_id_mapping.clear();
        self.fingerprint_confirm_senders.clear();
        self.outgoing_transfers.clear();
        self.pending_file_sends.clear();
        self.active_transfers.clear();
        // Clean up spools too: dropping the receivers would strand temp files.
        for (transfer_id, incoming) in self.incoming_files.drain() {
            if let Err(e) = incoming.abort_cleanup() {
                tracing::warn!(%transfer_id, error = %e, "failed to clean up a transfer while clearing data");
            }
        }
        self.pending_file_end.clear();
        self.incoming_streams.clear();
        self.pending_text_sends.clear();
        self.awaiting_ack.clear();
        self.incoming_text_messages.clear();
        self.toasts.clear();
        self.fingerprint_verification_requests.clear();
        self.expected_fingerprints.clear();

        if !history_path.as_os_str().is_empty() && self.history_key.is_some() {
            // An unreadable file is moved aside, never cleared over.
            if self.history_load_error.is_some() {
                if let Err(e) = self.set_aside_unreadable_history(history_path) {
                    tracing::error!(error = %e, "could not set the unreadable history aside; not saving");
                    return;
                }
            }
            let _ = self.save_history(history_path);
            tracing::info!("History cleared and saved");
        } else {
            tracing::info!("History cleared in memory");
        }
    }

    /// Delete everything this app keeps (identity, history, saved communities,
    /// diagnostics). Every later save is refused (`data_wiped`) so the closing
    /// flush cannot recreate it. The caller exits after.
    pub fn delete_all_data(
        &mut self,
        data_dir: &std::path::Path,
        history_path: &std::path::Path,
        identity_path: &std::path::Path,
    ) -> Result<()> {
        tracing::warn!("Deleting ALL data including identity");

        self.stop_hosting();
        // In-memory state (this also ends every session).
        self.clear_history(std::path::Path::new(""));
        self.data_wiped = true;
        self.history_key = None;
        // Waits for an in-progress background save and makes later ones refuse.
        let mut gate = self.persist_gate.lock().unwrap_or_else(|e| e.into_inner());
        *gate = true;

        // Remove only what this app writes, and the folder only if then empty:
        // `P2PEM_DATA_DIR` can point anywhere.
        let mut files = vec![history_path.to_path_buf(), identity_path.to_path_buf()];
        let mut dirs = Vec::new();
        for dir in [
            history_path.parent(),
            identity_path.parent(),
            Some(data_dir),
        ]
        .into_iter()
        .flatten()
        .filter(|d| !d.as_os_str().is_empty())
        {
            files.push(dir.join("parties.json"));
            // A legacy plaintext history that was never migrated.
            files.push(dir.join("history.json"));
            dirs.push(dir.join("diagnostics"));
            // Unreadable histories set aside by `set_aside_unreadable_history`.
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if name.starts_with("history.json") && name.contains(".unreadable-") {
                        files.push(entry.path());
                    }
                }
            }
        }
        files.sort();
        files.dedup();
        for file in files.iter().filter(|f| f.is_file()) {
            std::fs::remove_file(file)?;
            tracing::info!("Deleted {}", file.display());
        }
        for dir in dirs.iter().filter(|d| d.is_dir()) {
            std::fs::remove_dir_all(dir)?;
        }
        if !data_dir.as_os_str().is_empty() && data_dir.is_dir() {
            match std::fs::remove_dir(data_dir) {
                Ok(()) => tracing::info!("App data directory deleted: {}", data_dir.display()),
                Err(e) => tracing::warn!(
                    "Left {} in place (it holds files this app did not create): {e}",
                    data_dir.display()
                ),
            }
        }
        Ok(())
    }

    /// Whether `delete_all_data` has run.
    pub fn data_wiped(&self) -> bool {
        self.data_wiped
    }
}

impl ChatManager {
    /// Abort every session task and signal every streaming send to stop.
    fn stop_all_tasks(&mut self) {
        for (_, task) in self.session_tasks.drain() {
            task.abort();
        }
        for handle in self.outgoing_transfers.values() {
            handle
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// Dropping the manager ends its sessions: a dropped `SessionHandle` alone does
/// not stop the message loop, so sockets (and listeners) would outlive it.
impl Drop for ChatManager {
    fn drop(&mut self) {
        self.stop_all_tasks();
    }
}

mod connect;
mod contacts;
mod events;
mod files;
mod invites;
mod text;

#[cfg(test)]
mod adversarial_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
