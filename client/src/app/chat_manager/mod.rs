//! Chat management and application state orchestration.
//!
//! Provides the `ChatManager` which coordinates:
//! - Contacts and chats lifecycle (create, rename, group chats)
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
    pub from_app_tx: mpsc::UnboundedSender<ProtocolMessage>,
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
    /// wire yet (FIFO per chat; resolved by `SessionEvent::FileSendComplete`).
    pending_file_sends: HashMap<Uuid, VecDeque<String>>,
    #[allow(dead_code)] // Reserved for future file transfer implementation
    incoming_files: HashMap<Uuid, IncomingFileSync>,
    /// Incoming transfers whose `FileEnd` arrived while still awaiting the
    /// user's acceptance: the spooled file is complete and held until the
    /// user accepts (finalize) or declines (delete).
    pending_file_end: std::collections::HashSet<Uuid>,
    incoming_text_messages: HashMap<(Uuid, Uuid), IncomingTextMessage>,
    pub toasts: Vec<Toast>,
    pub config: Config,
    pub fingerprint_verification_request: Option<(String, String, Uuid)>,
    pub history_key: Option<[u8; 32]>,
    /// Tracks if the application intends to be hosting.
    /// Used for auto-rehosting if the placeholder connection is consumed.
    pub is_hosting: bool,
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
            incoming_files: HashMap::new(),
            pending_file_end: std::collections::HashSet::new(),
            incoming_text_messages: HashMap::new(),
            toasts: Vec::new(),
            config,
            fingerprint_verification_request: None,
            fingerprint_confirm_senders: HashMap::new(),
            history_key: None,
            is_hosting: false,
            connection_password: None,
            conversation_locked: false,
            external_address: None,
            pending_upnp: None,
            upnp_cancel: None,
        }
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
        self.active_transfers.clear();
        self.incoming_files.clear();
        self.pending_file_end.clear();
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
