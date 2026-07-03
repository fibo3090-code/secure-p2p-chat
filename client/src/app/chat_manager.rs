//! Chat management and application state orchestration.
//!
//! Provides the `ChatManager` which coordinates:
//! - Contacts and chats lifecycle (create, rename, group chats)
//! - Network sessions and event handling (`SessionEvent`)
//! - Message routing and typing indicators
//! - File transfer state and toasts/notifications
//! - Invite link generation and parsing (including QR codes)

use anyhow::{anyhow, bail, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::core::ProtocolMessage;
use crate::network::{
    generate_relay_token, run_client_session, run_client_session_via_relay, run_host_session,
    run_host_session_via_relay,
};
use crate::transfer::IncomingFileSync;
use crate::types::*;
use rsa::RsaPrivateKey;

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
    #[allow(dead_code)] // Reserved for future file transfer implementation
    incoming_files: HashMap<Uuid, IncomingFileSync>,
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
}

struct IncomingTextMessage {
    timestamp_millis: u64,
    parts: Vec<Option<String>>,
    updated_at: std::time::Instant,
}

impl ChatManager {
    /// Parse an address of the form host:port
    /// Returns (host, port) or an error if the format is invalid.
    pub fn parse_address(address: &str) -> Result<(String, u16)> {
        crate::util::parse_host_port(address, None).map_err(|e| {
            tracing::error!("Invalid address format for contact '{}': {}", address, e);
            anyhow::anyhow!("Invalid contact address: {}", e)
        })
    }

    fn build_text_protocol_messages(
        send_seq: &mut u64,
        text: &str,
        timestamp: u64,
    ) -> Result<Vec<ProtocolMessage>> {
        if text.len() <= crate::MAX_TEXT_MESSAGE_BYTES {
            *send_seq += 1;
            return Ok(vec![ProtocolMessage::Text {
                text: text.to_string(),
                timestamp,
                seq: *send_seq,
            }]);
        }

        let chunks = Self::split_text_chunks(text, crate::TEXT_CHUNK_BYTES);
        if chunks.is_empty() {
            bail!("Message is empty");
        }

        let message_id = Uuid::new_v4();
        let total_chunks = u32::try_from(chunks.len())
            .map_err(|_| anyhow!("Message is too large to chunk safely"))?;
        let mut messages = Vec::with_capacity(chunks.len());

        for (chunk_index, text_part) in chunks.into_iter().enumerate() {
            *send_seq += 1;
            messages.push(ProtocolMessage::TextChunk {
                message_id,
                chunk_index: chunk_index as u32,
                total_chunks,
                text_part,
                timestamp,
                seq: *send_seq,
            });
        }

        Ok(messages)
    }

    fn split_text_chunks(text: &str, max_bytes: usize) -> Vec<String> {
        debug_assert!(max_bytes > 0);

        let mut chunks = Vec::new();
        let mut current = String::new();
        let mut current_bytes = 0usize;

        for ch in text.chars() {
            let ch_bytes = ch.len_utf8();
            if current_bytes > 0 && current_bytes + ch_bytes > max_bytes {
                chunks.push(std::mem::take(&mut current));
                current_bytes = 0;
            }
            current.push(ch);
            current_bytes += ch_bytes;
        }

        if !current.is_empty() {
            chunks.push(current);
        }

        chunks
    }

    fn cleanup_stale_incoming_text_messages(&mut self) {
        const INCOMING_TEXT_TIMEOUT: Duration = Duration::from_secs(120);

        let now = std::time::Instant::now();
        let stale_keys: Vec<(Uuid, Uuid)> = self
            .incoming_text_messages
            .iter()
            .filter_map(|(key, pending)| {
                if now.duration_since(pending.updated_at) >= INCOMING_TEXT_TIMEOUT {
                    Some(*key)
                } else {
                    None
                }
            })
            .collect();

        for (chat_id, _message_id) in stale_keys {
            self.incoming_text_messages.remove(&(chat_id, _message_id));
            self.add_toast(
                ToastLevel::Warning,
                "A large incoming message could not be completed and was discarded.".to_string(),
            );
        }
    }

    fn register_incoming_text_chunk(
        &mut self,
        chat_id: Uuid,
        message_id: Uuid,
        chunk_index: u32,
        total_chunks: u32,
        text_part: String,
        timestamp_millis: u64,
    ) -> Result<Option<(String, chrono::DateTime<chrono::Utc>)>> {
        let entry = self
            .incoming_text_messages
            .entry((chat_id, message_id))
            .or_insert_with(|| IncomingTextMessage {
                timestamp_millis,
                parts: vec![None; total_chunks as usize],
                updated_at: std::time::Instant::now(),
            });

        if entry.parts.len() != total_chunks as usize {
            bail!("Chunk count mismatch for large text message");
        }

        let index = chunk_index as usize;
        if index >= entry.parts.len() {
            bail!("Chunk index out of bounds for large text message");
        }

        entry.timestamp_millis = timestamp_millis;
        entry.updated_at = std::time::Instant::now();
        entry.parts[index] = Some(text_part);

        if entry.parts.iter().all(Option::is_some) {
            let parts = self
                .incoming_text_messages
                .remove(&(chat_id, message_id))
                .ok_or_else(|| anyhow!("Large text message disappeared during reassembly"))?
                .parts;
            let text = parts
                .into_iter()
                .collect::<Option<Vec<String>>>()
                .ok_or_else(|| anyhow!("Large text message reassembly failed"))?
                .join("");
            let timestamp =
                chrono::DateTime::<chrono::Utc>::from_timestamp_millis(timestamp_millis as i64)
                    .unwrap_or_else(chrono::Utc::now);
            return Ok(Some((text, timestamp)));
        }

        Ok(None)
    }

    fn preview_text_for_notification(text: &str) -> String {
        const MAX_PREVIEW_CHARS: usize = 50;
        let truncated: String = text.chars().take(MAX_PREVIEW_CHARS).collect();
        if text.chars().count() > MAX_PREVIEW_CHARS {
            format!("{}...", truncated)
        } else {
            truncated
        }
    }

    async fn validate_outgoing_file(path: &Path) -> Result<(String, u64)> {
        let metadata = tokio::fs::metadata(path)
            .await
            .map_err(|e| Self::map_file_access_error(path, e, "read file metadata"))?;

        if !metadata.is_file() {
            bail!("Selected path is not a regular file: {}", path.display());
        }

        let file_size = metadata.len();
        if file_size > crate::MAX_FILE_SIZE {
            bail!(
                "File is too large: {} bytes exceeds the {} byte limit",
                file_size,
                crate::MAX_FILE_SIZE
            );
        }

        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| anyhow!("Selected file has an invalid filename"))?
            .to_string();

        tokio::fs::File::open(path)
            .await
            .map_err(|e| Self::map_file_access_error(path, e, "open file for reading"))?;

        Ok((filename, file_size))
    }

    fn map_file_access_error(path: &Path, error: std::io::Error, action: &str) -> anyhow::Error {
        let lower = error.to_string().to_lowercase();
        let message = match error.kind() {
            std::io::ErrorKind::NotFound => format!(
                "Cannot {}: file not found or not available locally ({})",
                action,
                path.display()
            ),
            std::io::ErrorKind::PermissionDenied => format!(
                "Cannot {}: permission denied for {}",
                action,
                path.display()
            ),
            _ if lower.contains("cloud")
                || lower.contains("offline")
                || lower.contains("not available") =>
            {
                format!(
                    "Cannot {}: {} is not fully available locally. If it is stored in OneDrive, iCloud, or Dropbox, mark it for offline use first.",
                    action,
                    path.display()
                )
            }
            _ => format!("Cannot {} {}: {}", action, path.display(), error),
        };

        anyhow!(message)
    }

    fn transfer_ids_for_chat_with_status<F>(&self, chat_id: Uuid, mut predicate: F) -> Vec<Uuid>
    where
        F: FnMut(&TransferStatus) -> bool,
    {
        self.active_transfers
            .iter()
            .filter_map(|(transfer_id, transfer)| {
                (transfer.chat_id == chat_id && predicate(&transfer.status)).then_some(*transfer_id)
            })
            .collect()
    }

    fn active_transfer_id_for_chat(&self, chat_id: Uuid) -> Option<Uuid> {
        self.transfer_ids_for_chat_with_status(chat_id, |status| {
            matches!(status, TransferStatus::Pending | TransferStatus::InProgress)
        })
        .into_iter()
        .next()
    }

    fn clear_transfer_state(&mut self, transfer_id: Uuid) {
        self.active_transfers.remove(&transfer_id);
        self.incoming_files.remove(&transfer_id);
    }

    pub fn new(config: Config) -> Self {
        Self {
            chats: HashMap::new(),
            contacts: HashMap::new(),
            contact_to_chat: HashMap::new(),
            sessions: HashMap::new(),
            session_events: HashMap::new(),
            chat_id_mapping: HashMap::new(),
            active_transfers: HashMap::new(),
            incoming_files: HashMap::new(),
            incoming_text_messages: HashMap::new(),
            toasts: Vec::new(),
            config,
            fingerprint_verification_request: None,
            fingerprint_confirm_senders: HashMap::new(),
            history_key: None,
            is_hosting: false,
            connection_password: None,
            conversation_locked: false,
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

    /// Add a contact
    pub fn add_contact(
        &mut self,
        name: String,
        address: Option<String>,
        fingerprint: Option<String>,
        public_key: Option<String>,
    ) -> Uuid {
        let id = Uuid::new_v4();
        tracing::info!(id = %id, name = %name, has_address = %address.is_some(), has_fp = %fingerprint.is_some(), "Adding contact");
        let contact = Contact {
            id,
            name,
            address,
            relay_server: None,
            relay_token: None,
            fingerprint,
            public_key,
            created_at: chrono::Utc::now(),
            trust_state: TrustState::Unverified,
            notes: String::new(),
            tags: Vec::new(),
            last_seen: None,
        };
        self.contacts.insert(id, contact);
        // no chat association by default
        tracing::debug!(id = %id, total_contacts = %self.contacts.len(), "Contact added");
        id
    }

    pub fn import_contact(&mut self, mut contact: Contact) -> Uuid {
        let id = Uuid::new_v4();
        contact.id = id;
        contact.created_at = chrono::Utc::now();
        self.contacts.insert(id, contact);
        id
    }

    /// Remove a contact
    pub fn remove_contact(&mut self, contact_id: Uuid) {
        tracing::info!(contact_id = %contact_id, "Removing contact");
        self.contacts.remove(&contact_id);
        self.contact_to_chat.remove(&contact_id);
        tracing::debug!(remaining_contacts = %self.contacts.len(), "Contact removed");
    }

    /// Get a contact
    pub fn get_contact(&self, contact_id: Uuid) -> Option<&Contact> {
        self.contacts.get(&contact_id)
    }

    /// Associate a contact with a one-to-one chat (useful when a session is created for that contact)
    pub fn associate_contact_with_chat(&mut self, contact_id: Uuid, chat_id: Uuid) {
        tracing::debug!(
            "associate_contact_with_chat: contact_id={}, chat_id={}",
            contact_id,
            chat_id
        );
        self.contact_to_chat.insert(contact_id, chat_id);
        if let Some(chat) = self.chats.get_mut(&chat_id) {
            if !chat.participants.contains(&contact_id) {
                chat.participants.push(contact_id);
            }
        }
        tracing::info!("Associated contact {} -> chat {}", contact_id, chat_id);
    }

    /// Attempt to reconnect to previously mapped contacts based on persisted history.
    /// Best-effort: skips missing contacts and logs warnings instead of failing fast.
    pub async fn auto_reconnect_contacts(&mut self, privkey: &RsaPrivateKey) {
        if !self.config.auto_connect {
            tracing::info!("auto_connect disabled; skipping auto reconnect");
            return;
        }

        let mappings: Vec<(Uuid, Uuid)> = self
            .contact_to_chat
            .iter()
            .map(|(c, ch)| (*c, *ch))
            .collect();
        tracing::info!(
            count = mappings.len(),
            "Starting auto reconnect for mapped contacts"
        );

        for (contact_id, mapped_chat_id) in mappings {
            let Some(contact) = self.contacts.get(&contact_id) else {
                tracing::warn!(%contact_id, "Skipping reconnect: contact missing; removing stale mapping");
                self.contact_to_chat.remove(&contact_id);
                continue;
            };

            tracing::debug!(
                %contact_id,
                mapped_chat_id = %mapped_chat_id,
                has_address = %contact.address.is_some(),
                has_fp = %contact.fingerprint.is_some(),
                "Auto reconnect attempt"
            );

            match self
                .connect_to_contact(contact_id, Some(mapped_chat_id), privkey)
                .await
            {
                Ok(chat_id) => {
                    tracing::info!(%contact_id, %chat_id, "Auto reconnect succeeded");
                }
                Err(e) => {
                    tracing::warn!(%contact_id, error = %e, "Auto reconnect failed");
                }
            }
        }
    }

    /// Create a group chat with given participants and optional title
    pub fn create_group_chat(&mut self, participants: Vec<Uuid>, title: Option<String>) -> Uuid {
        let chat_id = Uuid::new_v4();
        let default_title = title.unwrap_or_else(|| {
            if participants.is_empty() {
                "Group".to_string()
            } else {
                format!("Group ({})", participants.len())
            }
        });

        let chat = Chat {
            id: chat_id,
            title: default_title,
            kind: ChatKind::Group,
            transport: Transport::Direct,
            peer_fingerprint: None,
            participants,
            messages: Vec::new(),
            created_at: chrono::Utc::now(),
            peer_typing: false,
            typing_since: None,
            send_seq: 0,
            recv_seq: 0,
            is_host_placeholder: false,
        };

        self.chats.insert(chat_id, chat);
        chat_id
    }

    /// Send a text message to all participants of a group chat (convenience broadcast).
    /// This looks up one-to-one chats associated with each contact and sends the message
    /// via the existing session channels. Contacts without an active session are skipped.
    ///
    /// Returns the number of participants the message was successfully sent to.
    pub fn send_group_message(&mut self, group_chat_id: Uuid, text: String) -> Result<usize> {
        let chat = self
            .chats
            .get(&group_chat_id)
            .ok_or_else(|| anyhow::anyhow!("Group chat not found"))?;

        // Clone participants so we don't hold an immutable borrow while mutating chats
        let participants = chat.participants.clone();

        // Add message to group chat history ONCE (not per recipient)
        if let Some(gchat) = self.chats.get_mut(&group_chat_id) {
            gchat.messages.push(Message {
                id: Uuid::new_v4(),
                from_me: true,
                content: MessageContent::Text { text: text.clone() },
                timestamp: chrono::Utc::now(),
            });
        }

        // Try to send to all participants with active sessions
        let mut sent_count = 0;
        let mut offline_contacts = Vec::new();

        for participant_id in participants {
            if let Some(contact) = self.contacts.get(&participant_id) {
                if let Some(one_chat_id) = self.contact_to_chat.get(&participant_id).copied() {
                    if let Some(session) = self.sessions.get(&one_chat_id) {
                        if let Some(chat) = self.chats.get_mut(&one_chat_id) {
                            let timestamp = crate::util::current_timestamp_millis();
                            let Ok(messages) = Self::build_text_protocol_messages(
                                &mut chat.send_seq,
                                &text,
                                timestamp,
                            ) else {
                                continue;
                            };

                            if messages
                                .into_iter()
                                .all(|msg| session.from_app_tx.send(msg).is_ok())
                            {
                                sent_count += 1;
                            }
                        }
                    } else {
                        offline_contacts.push(contact.name.clone());
                    }
                } else {
                    offline_contacts.push(contact.name.clone());
                }
            }
        }

        // Show toast notification about offline participants
        if !offline_contacts.is_empty() {
            let offline_str = offline_contacts.join(", ");
            let message = if sent_count == 0 {
                format!(
                    "⚠ Message sent locally but all recipients are offline: {}",
                    offline_str
                )
            } else {
                format!(
                    "⚠ Sent to {} recipient(s), but offline: {}",
                    sent_count, offline_str
                )
            };
            self.add_toast(ToastLevel::Warning, message);
        }

        Ok(sent_count)
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

    /// Start hosting on specified port
    pub async fn start_host(&mut self, port: u16, privkey: RsaPrivateKey) -> Result<Uuid> {
        // Guard: if a placeholder host already exists, avoid spawning another.
        if let Some(existing_host) = self.chats.values().find(|c| c.is_host_placeholder) {
            // If the placeholder has no active session, clean it up so we can rehost.
            if !self.sessions.contains_key(&existing_host.id) {
                let id_to_remove = existing_host.id;
                self.chats.remove(&id_to_remove);
                self.sessions.remove(&id_to_remove);
                self.session_events.remove(&id_to_remove);
                self.fingerprint_confirm_senders.remove(&id_to_remove);
                tracing::warn!(port = %port, "Removed stale placeholder host before restarting");
            } else {
                self.add_toast(
                    ToastLevel::Info,
                    format!("Already listening on port {}", port),
                );
                tracing::info!(port = %port, "start_host skipped: active placeholder host exists");
                return Err(anyhow::anyhow!("Already listening on port {}", port));
            }
        }

        let chat_id = Uuid::new_v4();
        tracing::info!(chat_id = %chat_id, port = %port, "start_host called");

        // Create channels
        let (to_app_tx, to_app_rx) = mpsc::unbounded_channel();
        let (from_app_tx, from_app_rx) = mpsc::unbounded_channel();

        // Create confirmation channel so UI can accept/reject the fingerprint
        let (confirm_tx, confirm_rx) = mpsc::unbounded_channel();
        let connection_password = self.connection_password.clone();

        // Spawn session task
        tokio::spawn(async move {
            if let Err(e) = run_host_session(
                port,
                privkey,
                to_app_tx,
                from_app_rx,
                confirm_rx,
                chat_id,
                connection_password,
            )
            .await
            {
                tracing::error!("Host session error: {}", e);
            }
        });

        // Create chat entry
        let chat = Chat {
            id: chat_id,
            title: format!("Host on :{}", port),
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
            is_host_placeholder: true,
        };

        self.chats.insert(chat_id, chat);
        self.sessions.insert(chat_id, SessionHandle { from_app_tx });
        self.session_events
            .insert(chat_id, Arc::new(Mutex::new(to_app_rx)));
        self.fingerprint_confirm_senders.insert(chat_id, confirm_tx);

        self.is_hosting = true;
        self.add_toast(ToastLevel::Info, format!("Listening on port {}", port));
        tracing::debug!(chat_count = %self.chats.len(), session_count = %self.sessions.len(), "Host session initialized");

        Ok(chat_id)
    }

    pub async fn start_host_via_relay(
        &mut self,
        relay_server: &str,
        token: Option<String>,
        privkey: RsaPrivateKey,
    ) -> Result<(Uuid, String)> {
        if let Some(existing_host) = self.chats.values().find(|c| c.is_host_placeholder) {
            if self.sessions.contains_key(&existing_host.id) {
                self.add_toast(
                    ToastLevel::Info,
                    "Already hosting a session. Disconnect it before starting relay hosting."
                        .to_string(),
                );
                return Err(anyhow!("Already hosting a session"));
            }
        }

        let relay_token = token.unwrap_or_else(generate_relay_token);
        let chat_id = Uuid::new_v4();
        let (to_app_tx, to_app_rx) = mpsc::unbounded_channel();
        let (from_app_tx, from_app_rx) = mpsc::unbounded_channel();
        let (confirm_tx, confirm_rx) = mpsc::unbounded_channel();
        let relay_server_owned = relay_server.to_string();
        let relay_token_owned = relay_token.clone();

        tokio::spawn(async move {
            if let Err(e) = run_host_session_via_relay(
                &relay_server_owned,
                &relay_token_owned,
                privkey,
                to_app_tx,
                from_app_rx,
                confirm_rx,
                chat_id,
            )
            .await
            {
                tracing::error!("Relay host session error: {}", e);
            }
        });

        self.chats.insert(
            chat_id,
            Chat {
                id: chat_id,
                title: format!("Relay host via {}", relay_server),
                kind: ChatKind::Dm,
                transport: Transport::Relay,
                peer_fingerprint: None,
                participants: Vec::new(),
                messages: Vec::new(),
                created_at: chrono::Utc::now(),
                peer_typing: false,
                typing_since: None,
                send_seq: 0,
                recv_seq: 0,
                is_host_placeholder: true,
            },
        );
        self.sessions.insert(chat_id, SessionHandle { from_app_tx });
        self.session_events
            .insert(chat_id, Arc::new(Mutex::new(to_app_rx)));
        self.fingerprint_confirm_senders.insert(chat_id, confirm_tx);
        self.is_hosting = true;
        self.add_toast(
            ToastLevel::Info,
            format!("Waiting for relay peer via {}", relay_server),
        );
        Ok((chat_id, relay_token))
    }

    /// Connect to a host
    pub async fn connect_to_host(
        &mut self,
        host: &str,
        port: u16,
        existing_chat_id: Option<Uuid>,
        privkey: RsaPrivateKey,
    ) -> Result<Uuid> {
        let chat_id = existing_chat_id.unwrap_or_else(Uuid::new_v4);
        tracing::info!(chat_id = %chat_id, host = %host, port = %port, "connect_to_host called");

        let (to_app_tx, to_app_rx) = mpsc::unbounded_channel();
        let (from_app_tx, from_app_rx) = mpsc::unbounded_channel();

        let host_copy = host.to_string();
        let (confirm_tx, confirm_rx) = mpsc::unbounded_channel();
        let connection_password = self.connection_password.clone();

        tokio::spawn(async move {
            if let Err(e) = run_client_session(
                &host_copy,
                port,
                privkey,
                to_app_tx,
                from_app_rx,
                confirm_rx,
                chat_id,
                connection_password,
            )
            .await
            {
                tracing::error!("Client session error: {}", e);
            }
        });

        if let std::collections::hash_map::Entry::Vacant(e) = self.chats.entry(chat_id) {
            let chat = Chat {
                id: chat_id,
                title: format!("{}:{}", host, port),
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
            e.insert(chat);
            tracing::debug!(chat_id = %chat_id, "Created local chat entry for client session");
        }

        self.sessions.insert(chat_id, SessionHandle { from_app_tx });
        self.session_events
            .insert(chat_id, Arc::new(Mutex::new(to_app_rx)));
        self.fingerprint_confirm_senders.insert(chat_id, confirm_tx);
        tracing::debug!(session_count = %self.sessions.len(), has_events = %self.session_events.contains_key(&chat_id), "Client session initialized");

        self.add_toast(ToastLevel::Info, format!("Connecting to {}:{}", host, port));

        Ok(chat_id)
    }

    pub async fn connect_via_relay(
        &mut self,
        relay_server: &str,
        token: &str,
        existing_chat_id: Option<Uuid>,
        privkey: RsaPrivateKey,
    ) -> Result<Uuid> {
        let chat_id = existing_chat_id.unwrap_or_else(Uuid::new_v4);
        let (to_app_tx, to_app_rx) = mpsc::unbounded_channel();
        let (from_app_tx, from_app_rx) = mpsc::unbounded_channel();
        let (confirm_tx, confirm_rx) = mpsc::unbounded_channel();
        let relay_server_owned = relay_server.to_string();
        let relay_token_owned = token.to_string();

        tokio::spawn(async move {
            if let Err(e) = run_client_session_via_relay(
                &relay_server_owned,
                &relay_token_owned,
                privkey,
                to_app_tx,
                from_app_rx,
                confirm_rx,
                chat_id,
            )
            .await
            {
                tracing::error!("Relay client session error: {}", e);
            }
        });

        match self.chats.entry(chat_id) {
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(Chat {
                    id: chat_id,
                    title: format!("Relay via {}", relay_server),
                    kind: ChatKind::Dm,
                    transport: Transport::Relay,
                    peer_fingerprint: None,
                    participants: Vec::new(),
                    messages: Vec::new(),
                    created_at: chrono::Utc::now(),
                    peer_typing: false,
                    typing_since: None,
                    send_seq: 0,
                    recv_seq: 0,
                    is_host_placeholder: false,
                });
            }
            // Reconnecting or a chat pre-created elsewhere (e.g. the contacts
            // dialog defaults to Direct): normalize its route metadata to Relay so
            // it isn't persisted or rendered as a direct chat.
            std::collections::hash_map::Entry::Occupied(mut e) => {
                e.get_mut().transport = Transport::Relay;
            }
        }

        self.sessions.insert(chat_id, SessionHandle { from_app_tx });
        self.session_events
            .insert(chat_id, Arc::new(Mutex::new(to_app_rx)));
        self.fingerprint_confirm_senders.insert(chat_id, confirm_tx);
        self.add_toast(
            ToastLevel::Info,
            format!("Connecting via relay {}", relay_server),
        );
        Ok(chat_id)
    }

    pub async fn connect_to_contact(
        &mut self,
        contact_id: Uuid,
        existing_chat_id: Option<Uuid>,
        privkey: &RsaPrivateKey,
    ) -> Result<Uuid> {
        let contact = self
            .contacts
            .get(&contact_id)
            .ok_or_else(|| anyhow::anyhow!("Contact not found"))?
            .clone();
        // If we already have a mapped chat for this contact, ensure it has a session; otherwise try to establish one
        if let Some(mapped) = self.contact_to_chat.get(&contact_id).copied() {
            let has_session = self.sessions.contains_key(&mapped);
            tracing::debug!(
                "connect_to_contact: mapped chat exists: {} (has_session={})",
                mapped,
                has_session
            );
            if has_session {
                return Ok(mapped);
            }

            // Try to re-associate to an existing active session by fingerprint first
            if let Some(fp) = contact.fingerprint.clone() {
                if let Some(active_chat_id) = self.chats.iter().find_map(|(id, chat)| {
                    if chat.peer_fingerprint.as_deref() == Some(fp.as_str())
                        && self.sessions.contains_key(id)
                    {
                        Some(*id)
                    } else {
                        None
                    }
                }) {
                    tracing::info!(
                        "Re-associating mapped contact {} to active chat {} by fingerprint",
                        contact_id,
                        active_chat_id
                    );
                    self.associate_contact_with_chat(contact_id, active_chat_id);
                    return Ok(active_chat_id);
                }
            }
            // Otherwise, if the contact has an address, start a connection using the mapped chat id
            if let Some(address) = contact.address.clone() {
                if let Ok((host, port)) = Self::parse_address(&address) {
                    tracing::info!("Connecting mapped chat {} to {}:{}", mapped, host, port);
                    let chat_id = self
                        .connect_to_host(&host, port, Some(mapped), privkey.clone())
                        .await?;
                    self.associate_contact_with_chat(contact_id, chat_id);
                    return Ok(chat_id);
                }
            }
            if let (Some(relay_server), Some(relay_token)) =
                (contact.relay_server.clone(), contact.relay_token.clone())
            {
                let chat_id = self
                    .connect_via_relay(&relay_server, &relay_token, Some(mapped), privkey.clone())
                    .await?;
                self.associate_contact_with_chat(contact_id, chat_id);
                return Ok(chat_id);
            }
            // No way to create a session yet; fall through to fingerprint/address logic below
        }

        tracing::debug!(
            "connect_to_contact: id={}, has_address={}, has_fp={}",
            contact_id,
            contact.address.is_some(),
            contact.fingerprint.is_some()
        );
        if let Some(address) = contact.address.clone() {
            let (host, port) = Self::parse_address(&address)?;
            tracing::info!("Connecting to contact {} via {}:{}", contact_id, host, port);
            let chat_id = self
                .connect_to_host(&host, port, existing_chat_id, privkey.clone())
                .await?;
            self.associate_contact_with_chat(contact_id, chat_id);
            Ok(chat_id)
        } else if let (Some(relay_server), Some(relay_token)) =
            (contact.relay_server.clone(), contact.relay_token.clone())
        {
            tracing::info!(
                "Connecting to contact {} via relay {}",
                contact_id,
                relay_server
            );
            let chat_id = self
                .connect_via_relay(
                    &relay_server,
                    &relay_token,
                    existing_chat_id,
                    privkey.clone(),
                )
                .await?;
            self.associate_contact_with_chat(contact_id, chat_id);
            Ok(chat_id)
        } else {
            // Try to match an existing active session by fingerprint
            if let Some(fp) = contact.fingerprint.clone() {
                // Find a chat with matching peer_fingerprint and active session
                if let Some((&chat_id, _)) = self.chats.iter().find(|(_, chat)| {
                    chat.peer_fingerprint.as_deref() == Some(fp.as_str())
                        && self.sessions.contains_key(&chat.id)
                }) {
                    tracing::info!(
                        "Found active chat {} by fingerprint match; associating",
                        chat_id
                    );
                    self.associate_contact_with_chat(contact_id, chat_id);
                    return Ok(chat_id);
                }
            }
            tracing::error!(
                "Contact {} has no address and no active session found by fingerprint",
                contact_id
            );
            Err(anyhow::anyhow!(
                "Contact has no address. Edit the contact to set IP:PORT, or connect first so we can match by fingerprint."
            ))
        }
    }

    /// Send a text message (handles both 1-on-1 chats and group chats)
    pub fn send_message(&mut self, chat_id: Uuid, text: String) -> Result<()> {
        tracing::debug!(
            "send_message called for chat_id={}, len(text)={} chars",
            chat_id,
            text.len()
        );

        // Resolve chat_id through mapping (in case this is an incoming connection with a different chat_id)
        let actual_session_chat_id = self
            .chat_id_mapping
            .get(&chat_id)
            .copied()
            .unwrap_or(chat_id);

        // Determine if this is a true group chat
        let (participants_len, has_session) = if let Some(chat) = self.chats.get(&chat_id) {
            (
                chat.participants.len(),
                self.sessions.contains_key(&actual_session_chat_id),
            )
        } else {
            (0, false)
        };

        let is_group_chat = participants_len >= 2;
        tracing::debug!(
            "chat classification: is_group_chat={}, participants_len={}, has_session={}, actual_session_chat_id={}",
            is_group_chat,
            participants_len,
            has_session,
            actual_session_chat_id,
        );

        if is_group_chat {
            tracing::info!("Sending as group message to chat {}", chat_id);
            self.send_group_message(chat_id, text)?;
            return Ok(());
        }

        // One-to-one chat path
        if !has_session {
            tracing::warn!(
                "No active session for 1:1 chat {} (mapped to {}) yet. Likely still connecting.",
                chat_id,
                actual_session_chat_id,
            );
            self.add_toast(
                ToastLevel::Info,
                "Connecting... please wait before sending messages".to_string(),
            );
            return Ok(()); // Do not error; just inform the user and skip sending
        }

        let session = self
            .sessions
            .get(&actual_session_chat_id)
            .ok_or_else(|| anyhow::anyhow!("Session should exist but was not found"))?;

        let chat = self
            .chats
            .get_mut(&chat_id)
            .ok_or_else(|| anyhow::anyhow!("Chat not found for sending message"))?;

        let timestamp = crate::util::current_timestamp_millis();
        let messages = Self::build_text_protocol_messages(&mut chat.send_seq, &text, timestamp)?;

        for msg in messages {
            if let Err(e) = session.from_app_tx.send(msg) {
                tracing::error!("Failed to send message to chat {}: {}", chat_id, e);
                return Err(e.into());
            }
        }

        // Add to local history
        chat.messages.push(Message {
            id: Uuid::new_v4(),
            from_me: true,
            content: MessageContent::Text { text },
            timestamp: chrono::Utc::now(),
        });

        Ok(())
    }

    /// Start receiving a file
    pub fn start_receiving_file(
        &mut self,
        chat_id: Uuid,
        filename: &str,
        size: u64,
    ) -> Result<Uuid> {
        if size > crate::MAX_FILE_SIZE {
            anyhow::bail!(
                "File size {} exceeds maximum allowed ({} bytes)",
                size,
                crate::MAX_FILE_SIZE
            );
        }

        for transfer_id in self.transfer_ids_for_chat_with_status(chat_id, |status| {
            matches!(
                status,
                TransferStatus::Completed | TransferStatus::Failed(_) | TransferStatus::Cancelled
            )
        }) {
            self.clear_transfer_state(transfer_id);
        }

        if self.active_transfer_id_for_chat(chat_id).is_some() {
            bail!("Another file transfer is already in progress for this chat");
        }

        let transfer_id = Uuid::new_v4();

        let state = FileTransferState {
            id: transfer_id,
            chat_id,
            filename: filename.to_string(),
            size,
            received: 0,
            status: TransferStatus::Pending,
            seq: 0,
        };

        self.active_transfers.insert(transfer_id, state);

        self.add_toast(ToastLevel::Info, format!("Receiving file: {}", filename));

        Ok(transfer_id)
    }

    /// Update file transfer progress
    pub fn update_transfer_progress(&mut self, transfer_id: Uuid, bytes: u64) {
        if let Some(transfer) = self.active_transfers.get_mut(&transfer_id) {
            transfer.received = bytes;
            if bytes > 0 {
                transfer.status = TransferStatus::InProgress;
            }
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

    /// Send typing start indicator
    pub fn send_typing_start(&mut self, chat_id: Uuid) -> Result<()> {
        if !self.config.enable_typing_indicators {
            return Ok(());
        }

        let actual_id = self.chat_id_mapping.get(&chat_id).unwrap_or(&chat_id);
        let session = self
            .sessions
            .get(actual_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found"))?;

        let chat = self
            .chats
            .get_mut(&chat_id)
            .ok_or_else(|| anyhow::anyhow!("Chat not found"))?;
        chat.send_seq += 1;
        session
            .from_app_tx
            .send(ProtocolMessage::TypingStart { seq: chat.send_seq })?;
        Ok(())
    }

    /// Send typing stop indicator
    pub fn send_typing_stop(&mut self, chat_id: Uuid) -> Result<()> {
        if !self.config.enable_typing_indicators {
            return Ok(());
        }

        let actual_id = self.chat_id_mapping.get(&chat_id).unwrap_or(&chat_id);
        let session = self
            .sessions
            .get(actual_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found"))?;

        let chat = self
            .chats
            .get_mut(&chat_id)
            .ok_or_else(|| anyhow::anyhow!("Chat not found"))?;
        chat.send_seq += 1;
        session
            .from_app_tx
            .send(ProtocolMessage::TypingStop { seq: chat.send_seq })?;
        Ok(())
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

    /// Send the user's accept/reject decision for a fingerprint verification to the session task
    pub fn confirm_fingerprint(&mut self, chat_id: Uuid, accept: bool) -> Result<()> {
        tracing::info!(chat_id = %chat_id, accept = %accept, "Confirming fingerprint");
        if let Some(tx) = self.fingerprint_confirm_senders.get(&chat_id) {
            // If the user accepted and we have a pending verification request matching this chat,
            // persist the fingerprint in the chat record before confirming the session.
            if accept {
                if let Some((fp, _peer_name, req_chat_id)) = &self.fingerprint_verification_request
                {
                    // IMPORTANT: In host mode, req_chat_id is the session ID (the host placeholder ID)
                    // but the fingerprint should be stored in the actual chat (the client's ID).
                    // However, confirm_fingerprint is called with the session ID.
                    if *req_chat_id == chat_id {
                        // Resolve the actual chat ID if this is a mapped session
                        let target_chat_id = self
                            .chat_id_mapping
                            .iter()
                            .find(|(_, &session_id)| session_id == chat_id)
                            .map(|(&incoming_id, _)| incoming_id)
                            .unwrap_or(chat_id);

                        if let Some(chat) = self.chats.get_mut(&target_chat_id) {
                            tracing::debug!(
                                "Storing verified fingerprint for chat {}",
                                target_chat_id
                            );
                            chat.peer_fingerprint = Some(fp.clone());
                        }
                        // Clear the pending request now that we've stored it
                        self.fingerprint_verification_request = None;
                    }
                }
            }

            tx.send(accept)
                .map_err(|e| anyhow::anyhow!("Failed to send confirmation: {}", e))?;
            Ok(())
        } else {
            tracing::error!("No confirmation channel for chat {}", chat_id);
            Err(anyhow::anyhow!(
                "No confirmation channel for chat {}",
                chat_id
            ))
        }
    }

    /// Send a file to a chat
    pub async fn send_file(&mut self, chat_id: Uuid, path: std::path::PathBuf) -> Result<()> {
        use tokio::fs::File;
        use tokio::io::AsyncReadExt;

        tracing::info!(chat_id = %chat_id, path = %path.display().to_string(), "Preparing to send file");
        let actual_id = self.chat_id_mapping.get(&chat_id).unwrap_or(&chat_id);
        let sender = self
            .sessions
            .get(actual_id)
            .map(|s| s.from_app_tx.clone())
            .ok_or_else(|| anyhow!("Session not found"))?;

        let (filename, file_size) = Self::validate_outgoing_file(&path).await?;
        tracing::debug!(file = %filename, size = %file_size, "Sending file metadata");

        let chat = self
            .chats
            .get_mut(&chat_id)
            .ok_or_else(|| anyhow::anyhow!("Chat not found for sending file"))?;

        // Send file metadata using the same monotonic sequence space as all other chat messages.
        chat.send_seq += 1;
        sender.send(ProtocolMessage::FileMeta {
            filename: filename.clone(),
            size: file_size,
            seq: chat.send_seq,
        })?;

        // Send file chunks with globally monotonic sequence numbers.
        let mut file = File::open(&path).await?;
        let mut buffer = vec![0u8; crate::FILE_CHUNK_SIZE];
        let mut sent_chunks = 0u64;

        loop {
            let n = file.read(&mut buffer).await?;
            if n == 0 {
                break; // EOF
            }

            chat.send_seq += 1;
            sender.send(ProtocolMessage::FileChunk {
                chunk: buffer[..n].to_vec(),
                seq: chat.send_seq,
            })?;
            sent_chunks += 1;
            if sent_chunks % 64 == 0 {
                tracing::trace!(sent_chunks = %sent_chunks, "File sending progress");
            }
        }

        // Send end marker with the next monotonic sequence value.
        chat.send_seq += 1;
        sender.send(ProtocolMessage::FileEnd { seq: chat.send_seq })?;
        tracing::info!(file = %filename, total_bytes = %file_size, "File send complete");

        // Add to local history.
        chat.messages.push(Message {
            id: Uuid::new_v4(),
            from_me: true,
            content: MessageContent::File {
                filename: filename.clone(),
                size: file_size,
                path: Some(path),
            },
            timestamp: chrono::Utc::now(),
        });

        self.add_toast(ToastLevel::Success, format!("File sent: {}", filename));

        Ok(())
    }

    /// Poll and process all pending session events
    pub fn poll_session_events(&mut self) {
        self.cleanup_stale_incoming_text_messages();
        let chat_ids: Vec<Uuid> = self.session_events.keys().copied().collect();
        tracing::trace!(tracked_sessions = %chat_ids.len(), "Polling session events");

        for chat_id in chat_ids {
            // Collect all pending events for this session
            let mut events = Vec::new();
            if let Some(rx_mutex) = self.session_events.get(&chat_id) {
                if let Ok(mut rx) = rx_mutex.try_lock() {
                    while let Ok(event) = rx.try_recv() {
                        events.push(event);
                    }
                }
            }

            // Process collected events
            tracing::trace!(chat_id = %chat_id, events = %events.len(), "Processing session events for chat");
            for event in events {
                self.handle_session_event(chat_id, event);
            }
        }
    }

    /// Consolidate Trust-On-First-Use (TOFU) verification logic
    fn handle_tofu_verification(&mut self, session_id: Uuid, fingerprint: &str, peer_name: &str) {
        // Resolve the actual chat ID (important for host mode where session_id != chat_id)
        let actual_chat_id = self
            .chat_id_mapping
            .iter()
            .find(|(_, &sid)| sid == session_id)
            .map(|(&cid, _)| cid)
            .unwrap_or(session_id);

        // Have we already confirmed this fingerprint elsewhere (another chat with
        // this peer, or a saved contact)? Then it's a returning peer under TOFU and
        // we accept without another prompt. Computed before the mutable borrow below.
        let known_trusted = self.chats.iter().any(|(id, c)| {
            *id != actual_chat_id && c.peer_fingerprint.as_deref() == Some(fingerprint)
        }) || self
            .contacts
            .values()
            .any(|c| c.fingerprint.as_deref() == Some(fingerprint));

        let chat = match self.chats.get_mut(&actual_chat_id) {
            Some(c) => c,
            None => {
                tracing::error!("TOFU check failed: Chat {} not found", actual_chat_id);
                return;
            }
        };

        match &chat.peer_fingerprint {
            // CASE 1: No stored fingerprint. This is the FIRST USE.
            None => {
                tracing::info!(
                    "Trust on First Use for chat {}. Requesting user confirmation.",
                    actual_chat_id
                );
                // Auto-trust only if the user opted in, or this fingerprint is one
                // we've already verified (a returning peer) — otherwise prompt.
                if self.config.auto_trust_on_first_use || known_trusted {
                    tracing::info!(
                        "auto_trust_on_first_use enabled: auto-storing fingerprint for chat {}",
                        actual_chat_id
                    );
                    chat.peer_fingerprint = Some(fingerprint.to_string());
                    if let Some(tx) = self.fingerprint_confirm_senders.get(&session_id) {
                        if let Err(e) = tx.send(true) {
                            tracing::error!("Failed to auto-confirm fingerprint: {}", e);
                        }
                    }
                } else {
                    // Request explicit user verification via UI
                    // Note: fingerprint_verification_request uses the SESSION ID
                    // because confirmation (accept/reject) must be sent to that session's task.
                    self.fingerprint_verification_request =
                        Some((fingerprint.to_string(), peer_name.to_string(), session_id));
                    self.add_toast(
                        ToastLevel::Warning,
                        "Fingerprint verification required".to_string(),
                    );
                }
            }
            // CASE 2: Stored fingerprint matches the new one.
            Some(stored_fp) if stored_fp == fingerprint => {
                tracing::debug!(
                    "Fingerprint matches stored value for chat {}. Proceeding automatically.",
                    actual_chat_id
                );
                // Automatically confirm this connection using the session ID.
                if let Some(tx) = self.fingerprint_confirm_senders.get(&session_id) {
                    if let Err(e) = tx.send(true) {
                        tracing::error!("Failed to auto-confirm fingerprint: {}", e);
                    }
                }
            }
            // CASE 3: Stored fingerprint MISMATCHES. Security alert!
            Some(stored_fp) => {
                tracing::warn!(
                    "FINGERPRINT MISMATCH for chat {}! Stored: `{}`, New: `{}`",
                    actual_chat_id,
                    stored_fp,
                    fingerprint
                );
                // Trigger the UI dialog for manual verification using the session ID.
                self.fingerprint_verification_request =
                    Some((fingerprint.to_string(), peer_name.to_string(), session_id));
                self.add_toast(
                    ToastLevel::Warning,
                    "SECURITY WARNING: Peer fingerprint has changed!".to_string(),
                );
            }
        }
    }

    /// Handle a single session event
    fn handle_session_event(&mut self, chat_id: Uuid, event: SessionEvent) {
        tracing::debug!("Handling session event for {}: {:?}", chat_id, event);

        match event {
            SessionEvent::Listening { port } => {
                tracing::info!("Session {} listening on port {}", chat_id, port);
                self.add_toast(ToastLevel::Info, format!("Listening on port {}", port));
            }

            SessionEvent::Connected { peer } => {
                tracing::info!("Session {} connected to {}", chat_id, peer);
                self.add_toast(ToastLevel::Success, format!("Connected to {}", peer));

                if let Some(chat) = self.chats.get_mut(&chat_id) {
                    chat.title = peer;
                    chat.is_host_placeholder = false;
                }
            }

            SessionEvent::NewConnection {
                peer_addr,
                fingerprint,
                chat_id: incoming_chat_id,
            } => {
                tracing::info!(
                    "New incoming connection from {} with chat_id {}, session_chat_id={}",
                    peer_addr,
                    incoming_chat_id,
                    chat_id,
                );

                // Map incoming_chat_id to the session's chat_id (the placeholder host chat)
                // This allows messages sent to incoming_chat_id to be routed to the session
                self.chat_id_mapping.insert(incoming_chat_id, chat_id);
                tracing::debug!(
                    "Mapped incoming chat {} to session chat {}",
                    incoming_chat_id,
                    chat_id
                );

                // Format a title using the peer's fingerprint instead of IP address
                let title = format!(
                    "Peer {}",
                    crate::util::format_fingerprint_short(&fingerprint)
                );

                // Inherit the transport of the host session that accepted this peer:
                // a relay-hosted listener (`start_host_via_relay`) must produce a
                // Relay chat, not a hardcoded Direct one.
                let inherited_transport = self
                    .chats
                    .get(&chat_id)
                    .map(|c| c.transport)
                    .unwrap_or(Transport::Direct);

                // Create a chat for this new connection, or (on reconnect, where
                // the entry already exists) normalize its transport so it isn't
                // left as a stale Direct — the same fix applied in connect_via_relay.
                self.chats
                    .entry(incoming_chat_id)
                    .and_modify(|c| c.transport = inherited_transport)
                    .or_insert_with(|| Chat {
                        id: incoming_chat_id,
                        title,
                        kind: ChatKind::Dm,
                        transport: inherited_transport,
                        // Leave unset so TOFU actually runs for this incoming peer:
                        // pre-filling the peer's own fingerprint made the check below
                        // trivially "match" and silently auto-trust every caller.
                        peer_fingerprint: None,
                        participants: Vec::new(),
                        messages: Vec::new(),
                        created_at: chrono::Utc::now(),
                        peer_typing: false,
                        typing_since: None,
                        send_seq: 0,
                        recv_seq: 0,
                        is_host_placeholder: false,
                    });

                // If this connection consumes a placeholder host chat, remove the placeholder
                // so the UI shows only the real chat. Auto-rehost will spawn a new listener.
                if let Some(placeholder) = self.chats.get(&chat_id) {
                    if placeholder.is_host_placeholder {
                        tracing::debug!("Removing consumed host placeholder chat {}", chat_id);
                        self.chats.remove(&chat_id);
                    }
                }

                self.handle_tofu_verification(chat_id, &fingerprint, &peer_addr);
            }

            SessionEvent::ShowFingerprintVerification {
                fingerprint,
                peer_name,
                chat_id,
            } => {
                self.handle_tofu_verification(chat_id, &fingerprint, &peer_name);
            }

            SessionEvent::Ready => {
                tracing::info!("Session {} is ready", chat_id);
                self.add_toast(ToastLevel::Success, "Connection established!".to_string());
            }

            SessionEvent::MessageReceived(proto_msg) => {
                tracing::debug!("Session {} received message: {:?}", chat_id, proto_msg);

                // Find the actual chat to add the message to
                // If this session is mapped from an incoming connection, use the incoming chat_id
                // Otherwise use the session's chat_id
                let actual_chat_id = self
                    .chat_id_mapping
                    .iter()
                    .find(|(_, &session_id)| session_id == chat_id)
                    .map(|(&incoming_id, _)| incoming_id)
                    .unwrap_or(chat_id);

                tracing::debug!("Message routed to actual_chat_id={}", actual_chat_id);

                match proto_msg {
                    ProtocolMessage::Text { text, seq, .. } => {
                        if let Some(chat) = self.chats.get_mut(&actual_chat_id) {
                            if seq > chat.recv_seq {
                                chat.recv_seq = seq;
                                chat.messages.push(Message {
                                    id: Uuid::new_v4(),
                                    from_me: false,
                                    content: MessageContent::Text { text: text.clone() },
                                    timestamp: chrono::Utc::now(),
                                });

                                // Clear typing indicator
                                chat.peer_typing = false;
                                chat.typing_since = None;
                                // Show desktop notification
                                let preview = Self::preview_text_for_notification(&text);
                                self.show_notification("New message", &preview);

                                tracing::info!("Added received message to chat {}", actual_chat_id);
                            } else {
                                tracing::warn!("Received message with invalid sequence number for chat {}. Expected > {}, got {}. Discarding.", actual_chat_id, chat.recv_seq, seq);
                            }
                        } else {
                            tracing::error!(
                                "Chat {} not found for received message",
                                actual_chat_id
                            );
                        }
                    }
                    ProtocolMessage::TextChunk {
                        message_id,
                        chunk_index,
                        total_chunks,
                        text_part,
                        timestamp,
                        seq,
                    } => {
                        let valid_seq = if let Some(chat) = self.chats.get_mut(&actual_chat_id) {
                            if seq > chat.recv_seq {
                                chat.recv_seq = seq;
                                true
                            } else {
                                tracing::warn!("Received TextChunk with invalid sequence number for chat {}. Expected > {}, got {}. Discarding.", actual_chat_id, chat.recv_seq, seq);
                                false
                            }
                        } else {
                            false
                        };
                        if !valid_seq {
                            return;
                        }

                        match self.register_incoming_text_chunk(
                            actual_chat_id,
                            message_id,
                            chunk_index,
                            total_chunks,
                            text_part,
                            timestamp,
                        ) {
                            Ok(Some((text, assembled_timestamp))) => {
                                if let Some(chat) = self.chats.get_mut(&actual_chat_id) {
                                    chat.messages.push(Message {
                                        id: Uuid::new_v4(),
                                        from_me: false,
                                        content: MessageContent::Text { text: text.clone() },
                                        timestamp: assembled_timestamp,
                                    });
                                    chat.peer_typing = false;
                                    chat.typing_since = None;
                                }

                                let preview = Self::preview_text_for_notification(&text);
                                self.show_notification("New message", &preview);
                                tracing::info!(
                                    "Reassembled large text message {} for chat {}",
                                    message_id,
                                    actual_chat_id
                                );
                            }
                            Ok(None) => {
                                tracing::trace!(
                                    "Buffered text chunk {}/{} for chat {}",
                                    chunk_index + 1,
                                    total_chunks,
                                    actual_chat_id
                                );
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "Discarding large text message {} for chat {}: {}",
                                    message_id,
                                    actual_chat_id,
                                    e
                                );
                                self.incoming_text_messages
                                    .remove(&(actual_chat_id, message_id));
                                self.add_toast(
                                    ToastLevel::Warning,
                                    "A large incoming message could not be reconstructed."
                                        .to_string(),
                                );
                            }
                        }
                    }

                    ProtocolMessage::FileMeta {
                        filename,
                        size,
                        seq,
                    } => {
                        if let Some(chat) = self.chats.get_mut(&actual_chat_id) {
                            if seq > chat.recv_seq {
                                chat.recv_seq = seq;
                                tracing::info!(
                                    "Received file metadata: {} ({} bytes)",
                                    filename,
                                    size
                                );

                                match self.start_receiving_file(actual_chat_id, &filename, size) {
                                    Ok(transfer_id) => {
                                        // Create new IncomingFileSync for this transfer
                                        let file_path = self.config.download_dir.join(&filename);

                                        match IncomingFileSync::new(&file_path, size) {
                                            Ok(incoming) => {
                                                self.incoming_files.insert(transfer_id, incoming);
                                            }
                                            Err(e) => {
                                                tracing::error!(
                                                    "Failed to create incoming file: {}",
                                                    e
                                                );
                                                self.add_toast(
                                                    ToastLevel::Error,
                                                    format!("Failed to receive file: {}", e),
                                                );
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::error!("Failed to start receiving file: {}", e);
                                        self.add_toast(
                                            ToastLevel::Error,
                                            format!("Failed to receive file: {}", e),
                                        );
                                    }
                                }
                            } else {
                                tracing::warn!("Received FileMeta with invalid sequence number for chat {}. Expected > {}, got {}. Discarding.", chat_id, chat.recv_seq, seq);
                            }
                        }
                    }

                    ProtocolMessage::FileChunk { chunk, seq } => {
                        let valid_seq = if let Some(chat) = self.chats.get_mut(&actual_chat_id) {
                            if seq > chat.recv_seq {
                                chat.recv_seq = seq;
                                true
                            } else {
                                tracing::warn!("Received file chunk with invalid sequence number for chat {}. Expected > {}, got {}. Discarding.", actual_chat_id, chat.recv_seq, seq);
                                false
                            }
                        } else {
                            false
                        };
                        if !valid_seq {
                            return;
                        }

                        let transfer_id = self.active_transfer_id_for_chat(actual_chat_id);
                        if let Some(transfer_id) = transfer_id {
                            if let Some(transfer) = self.active_transfers.get_mut(&transfer_id) {
                                transfer.seq += 1;
                                tracing::debug!(
                                    "Received file chunk {} ({} bytes)",
                                    seq,
                                    chunk.len()
                                );

                                if let Some(incoming) = self.incoming_files.get_mut(&transfer_id) {
                                    if let Err(e) = incoming.write_chunk(&chunk) {
                                        tracing::error!("Failed to write chunk: {}", e);
                                        if let Some(transfer) =
                                            self.active_transfers.get_mut(&transfer_id)
                                        {
                                            transfer.status = TransferStatus::Failed(e.to_string());
                                        }
                                        if let Some(incoming) =
                                            self.incoming_files.remove(&transfer_id)
                                        {
                                            if let Err(cleanup_err) = incoming.abort_cleanup() {
                                                tracing::warn!(
                                                    "Failed to clean up aborted transfer {}: {}",
                                                    transfer_id,
                                                    cleanup_err
                                                );
                                            }
                                        }
                                        self.add_toast(
                                            ToastLevel::Error,
                                            format!("File transfer error: {}", e),
                                        );
                                    } else {
                                        let bytes_received = incoming.bytes_received();
                                        self.update_transfer_progress(transfer_id, bytes_received);
                                    }
                                }
                            }
                        }
                    }

                    ProtocolMessage::FileEnd { seq } => {
                        let valid_seq = if let Some(chat) = self.chats.get_mut(&actual_chat_id) {
                            if seq > chat.recv_seq {
                                chat.recv_seq = seq;
                                true
                            } else {
                                tracing::warn!("Received FileEnd with invalid sequence number for chat {}. Expected > {}, got {}. Discarding.", actual_chat_id, chat.recv_seq, seq);
                                false
                            }
                        } else {
                            false
                        };
                        if !valid_seq {
                            return;
                        }

                        let transfer_id = self.active_transfer_id_for_chat(actual_chat_id);
                        if let Some(transfer_id) = transfer_id {
                            if self.active_transfers.contains_key(&transfer_id) {
                                tracing::info!("File transfer completed");

                                // Finalize only the matching transfer, not all incoming files.
                                if let Some(incoming) = self.incoming_files.remove(&transfer_id) {
                                    let bytes_received = incoming.bytes_received();
                                    match incoming.finalize() {
                                        Ok(final_path) => {
                                            if let Some(mut transfer) =
                                                self.active_transfers.remove(&transfer_id)
                                            {
                                                transfer.status = TransferStatus::Completed;
                                                // Add to chat history.
                                                if let Some(chat) =
                                                    self.chats.get_mut(&actual_chat_id)
                                                {
                                                    chat.messages.push(Message {
                                                        id: Uuid::new_v4(),
                                                        from_me: false,
                                                        content: MessageContent::File {
                                                            filename: transfer.filename.clone(),
                                                            size: transfer.size,
                                                            path: Some(final_path),
                                                        },
                                                        timestamp: chrono::Utc::now(),
                                                    });
                                                }
                                                self.add_toast(
                                                    ToastLevel::Success,
                                                    format!("File received: {}", transfer.filename),
                                                );
                                            }
                                            self.update_transfer_progress(
                                                transfer_id,
                                                bytes_received,
                                            );
                                        }
                                        Err(e) => {
                                            if let Some(transfer) =
                                                self.active_transfers.get_mut(&transfer_id)
                                            {
                                                transfer.status =
                                                    TransferStatus::Failed(e.to_string());
                                            }
                                            tracing::error!("Failed to finalize file: {}", e);
                                            self.add_toast(
                                                ToastLevel::Error,
                                                format!("File transfer error: {}", e),
                                            );
                                            self.active_transfers.remove(&transfer_id);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    ProtocolMessage::Ping { seq } => {
                        if let Some(chat) = self.chats.get_mut(&actual_chat_id) {
                            if seq > chat.recv_seq {
                                chat.recv_seq = seq;
                                tracing::trace!("Received ping with seq {}", seq);
                            } else {
                                tracing::warn!("Received Ping with invalid sequence number for chat {}. Expected > {}, got {}. Discarding.", actual_chat_id, chat.recv_seq, seq);
                            }
                        }
                    }

                    ProtocolMessage::TypingStart { seq } => {
                        if let Some(chat) = self.chats.get_mut(&actual_chat_id) {
                            if seq > chat.recv_seq {
                                chat.recv_seq = seq;
                                chat.peer_typing = true;
                                chat.typing_since = Some(std::time::Instant::now());
                            } else {
                                tracing::warn!("Received TypingStart with invalid sequence number for chat {}. Expected > {}, got {}. Discarding.", actual_chat_id, chat.recv_seq, seq);
                            }
                        }
                    }

                    ProtocolMessage::TypingStop { seq } => {
                        if let Some(chat) = self.chats.get_mut(&actual_chat_id) {
                            if seq > chat.recv_seq {
                                chat.recv_seq = seq;
                                chat.peer_typing = false;
                                chat.typing_since = None;
                            } else {
                                tracing::warn!("Received TypingStop with invalid sequence number for chat {}. Expected > {}, got {}. Discarding.", actual_chat_id, chat.recv_seq, seq);
                            }
                        }
                    }
                    other => {
                        tracing::warn!(
                            "Received unhandled protocol message in message loop: {:?}",
                            other
                        );
                    }
                }
            }

            SessionEvent::Disconnected => {
                tracing::warn!("Session {} disconnected", chat_id);
                self.add_toast(ToastLevel::Warning, "Connection lost".to_string());

                // Clean up session
                self.sessions.remove(&chat_id);
                self.session_events.remove(&chat_id);
                self.chat_id_mapping.retain(|_, v| *v != chat_id);
            }

            SessionEvent::Error(err) => {
                tracing::error!("Session {} error: {}", chat_id, err);
                self.add_toast(ToastLevel::Error, format!("Connection error: {}", err));
            }

            SessionEvent::Warning(msg) => {
                tracing::warn!("Session {} warning: {}", chat_id, msg);
                self.add_toast(ToastLevel::Warning, msg);
            }
        }
    }

    /// Generate an invite link for sharing contact information
    /// Format: chat-p2p://invite/<base64_json>
    pub fn generate_invite_link(
        &self,
        name: &str,
        address: Option<String>,
        fingerprint: &str,
        public_key_pem: &str,
    ) -> Result<String> {
        use base64::Engine;
        use serde::{Deserialize, Serialize};

        #[derive(Serialize, Deserialize)]
        struct InvitePayload {
            name: String,
            address: Option<String>,
            fingerprint: String,
            public_key: String,
        }

        let payload = InvitePayload {
            name: name.to_string(),
            address,
            fingerprint: fingerprint.to_string(),
            public_key: public_key_pem.to_string(),
        };

        let json = serde_json::to_string(&payload)?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(json);
        Ok(format!("chat-p2p://invite/{}", encoded))
    }

    /// Parse an invite link and create a Contact
    /// Supports both v1 (unsigned) and v2 (signed) formats
    /// v1: chat-p2p://invite/<base64_json>
    /// v2: chat-p2p://invite/v2/<url_safe_base64_json>
    pub fn parse_invite_link(&self, link: &str) -> Result<Contact> {
        use serde::{Deserialize, Serialize};

        #[derive(Serialize, Deserialize)]
        struct InvitePayload {
            name: String,
            address: Option<String>,
            fingerprint: String,
            public_key: String,
        }

        #[derive(Serialize, Deserialize)]
        struct SignedInvitePayload {
            version: u32,
            timestamp: u64,
            nonce: String,
            name: String,
            address: Option<String>,
            relay_server: Option<String>,
            relay_token: Option<String>,
            fingerprint: String,
            public_key: String,
        }

        #[derive(Serialize, Deserialize)]
        struct SignedInvite {
            payload: SignedInvitePayload,
            signature: Vec<u8>,
        }

        tracing::debug!("Parsing invite link");

        // Check if this is a v2 (signed) or v1 (unsigned) invite
        if link.contains("/v2/") {
            // V2: Signed invite with RSA-PSS signature
            let encoded = link
                .strip_prefix("chat-p2p://invite/v2/")
                .ok_or_else(|| anyhow::anyhow!("Invalid v2 invite link format"))?;

            // Decode URL-safe base64
            use base64::Engine;
            let json = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(encoded)
                .map_err(|e| {
                    tracing::warn!(error = %e, "Invalid v2 invite link during base64 decode");
                    anyhow::anyhow!("Invalid v2 invite link: {}", e)
                })?;
            let json_str = String::from_utf8(json).map_err(|e| {
                tracing::warn!(error = %e, "Invalid UTF-8 in v2 invite link");
                anyhow::anyhow!("Invalid UTF-8 in v2 invite link: {}", e)
            })?;

            // Parse signed invite structure
            let signed_invite: SignedInvite = serde_json::from_str(&json_str).map_err(|e| {
                tracing::warn!(error = %e, "Invalid v2 invite data JSON");
                anyhow::anyhow!("Invalid v2 invite data: {}", e)
            })?;

            // Serialize payload back to JSON for signature verification
            let payload_json = serde_json::to_string(&signed_invite.payload).map_err(|e| {
                tracing::warn!(error = %e, "Failed to serialize payload for verification");
                anyhow::anyhow!("Serialization error: {}", e)
            })?;

            // Verify RSA-PSS signature using the public key from the invite
            let pubkey_pem = &signed_invite.payload.public_key;
            let pubkey = crate::core::crypto::pem_decode_public(pubkey_pem).map_err(|e| {
                tracing::warn!(error = %e, "Failed to decode public key from invite");
                anyhow::anyhow!("Invalid public key in invite: {}", e)
            })?;

            crate::core::crypto::rsa_verify_pss(
                &pubkey,
                payload_json.as_bytes(),
                &signed_invite.signature,
            )
            .map_err(|e| {
                tracing::warn!(error = %e, "v2 invite signature verification failed");
                anyhow::anyhow!("Invite signature verification failed: {}", e)
            })?;

            tracing::debug!(
                timestamp = signed_invite.payload.timestamp,
                "Successfully verified v2 signed invite"
            );

            let payload = &signed_invite.payload;

            // Sanitize address: ignore placeholder or clearly invalid addresses like "YOUR_IP:PORT"
            let address = payload.address.as_ref().and_then(|addr| {
                let trimmed = addr.trim();
                if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("YOUR_IP:PORT") {
                    None
                } else {
                    Self::parse_address(trimmed)
                        .ok()
                        .map(|(host, port)| crate::util::format_host_port(&host, port))
                }
            });
            let relay_server = payload.relay_server.as_ref().and_then(|server| {
                let trimmed = server.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    crate::util::parse_host_port(trimmed, Some(crate::PORT_DEFAULT))
                        .ok()
                        .map(|(host, port)| crate::util::format_host_port(&host, port))
                }
            });
            let relay_token = payload
                .relay_token
                .clone()
                .filter(|token| !token.trim().is_empty());

            // Create contact from v2 invite
            let contact = Contact {
                id: Uuid::new_v4(),
                name: payload.name.clone(),
                address,
                relay_server,
                relay_token,
                fingerprint: Some(payload.fingerprint.clone()),
                public_key: Some(payload.public_key.clone()),
                created_at: chrono::Utc::now(),
                trust_state: TrustState::Unverified,
                notes: String::new(),
                tags: Vec::new(),
                last_seen: None,
            };

            Ok(contact)
        } else {
            // V1: Legacy unsigned invite
            tracing::warn!(
                "Parsing legacy v1 unsigned invite link - prefer v2 signed format for security"
            );

            // Remove prefix if present
            let encoded = link.strip_prefix("chat-p2p://invite/").unwrap_or(link);

            // Decode base64
            use base64::Engine;
            let json = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|e| {
                    tracing::warn!(error = %e, "Invalid invite link during base64 decode");
                    anyhow::anyhow!("Invalid invite link: {}", e)
                })?;
            let json_str = String::from_utf8(json).map_err(|e| {
                tracing::warn!(error = %e, "Invalid UTF-8 in invite link");
                anyhow::anyhow!("Invalid UTF-8 in invite link: {}", e)
            })?;

            // Parse JSON
            let payload: InvitePayload = serde_json::from_str(&json_str).map_err(|e| {
                tracing::warn!(error = %e, "Invalid invite data JSON");
                anyhow::anyhow!("Invalid invite data: {}", e)
            })?;

            // Sanitize address: ignore placeholder or clearly invalid addresses like "YOUR_IP:PORT"
            let address = payload.address.as_ref().and_then(|addr| {
                let trimmed = addr.trim();
                if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("YOUR_IP:PORT") {
                    None
                } else {
                    Self::parse_address(trimmed)
                        .ok()
                        .map(|(host, port)| crate::util::format_host_port(&host, port))
                }
            });

            // Create contact
            let contact = Contact {
                id: Uuid::new_v4(),
                name: payload.name,
                address,
                relay_server: None,
                relay_token: None,
                fingerprint: Some(payload.fingerprint),
                public_key: Some(payload.public_key),
                created_at: chrono::Utc::now(),
                trust_state: TrustState::Unverified,
                notes: String::new(),
                tags: Vec::new(),
                last_seen: None,
            };

            Ok(contact)
        }
    }

    /// Generate a QR code for an invite link (as PNG bytes)
    pub fn generate_invite_qr(&self, invite_link: &str) -> Result<Vec<u8>> {
        use qrcode::QrCode;

        let code = QrCode::new(invite_link.as_bytes())
            .map_err(|e| anyhow::anyhow!("Failed to generate QR code: {}", e))?;

        let qr_image = code
            .render::<image::Luma<u8>>()
            .min_dimensions(200, 200)
            .build();

        let mut bytes = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut bytes);
        image::DynamicImage::ImageLuma8(qr_image)
            .write_to(&mut cursor, image::ImageFormat::Png)
            .map_err(|e| anyhow::anyhow!("Failed to encode QR code: {}", e))?;

        Ok(bytes)
    }
}

impl Default for ChatManager {
    fn default() -> Self {
        Self::new(Config::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use tempfile::tempdir;

    #[test]
    fn parse_invite_placeholder_is_ignored() {
        let mgr = ChatManager::default();

        let payload = serde_json::json!({
            "name": "Alice",
            "address": "YOUR_IP:PORT",
            "fingerprint": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "public_key": "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkq...\n-----END PUBLIC KEY-----",
        });

        let json = serde_json::to_string(&payload).unwrap();
        use base64::engine::general_purpose;
        let encoded = general_purpose::STANDARD.encode(json);
        let link = format!("chat-p2p://invite/{}", encoded);

        let contact = mgr.parse_invite_link(&link).expect("should parse invite");
        assert!(
            contact.address.is_none(),
            "placeholder address must be ignored"
        );
    }

    #[test]
    fn host_prompts_for_an_unknown_incoming_fingerprint() {
        // Simulate an incoming connection: a fresh chat (no stored fingerprint)
        // mapped from the peer's chat id to the session id, plus a confirm channel.
        let mut mgr = ChatManager::new(Config::default());
        let session_id = Uuid::new_v4();
        let incoming = Uuid::new_v4();
        mgr.create_local_chat_for_test(incoming, "Peer".into());
        mgr.chat_id_mapping.insert(incoming, session_id);
        let (tx, mut rx) = mpsc::unbounded_channel();
        mgr.add_fingerprint_confirm_sender_for_test(session_id, tx);

        mgr.handle_tofu_verification(session_id, "UNKNOWN-FP", "Peer");

        // The host must PROMPT for verification, not silently auto-trust.
        assert!(
            mgr.fingerprint_verification_request.is_some(),
            "host must prompt to verify an unknown incoming peer"
        );
        assert!(
            rx.try_recv().is_err(),
            "host must not auto-confirm an unknown incoming peer"
        );
    }

    #[test]
    fn host_auto_accepts_a_returning_known_fingerprint() {
        // A prior chat already has this fingerprint verified (a returning peer).
        let mut mgr = ChatManager::new(Config::default());
        let prior = Uuid::new_v4();
        mgr.create_local_chat_for_test(prior, "Known".into());
        mgr.get_chat_mut(prior).unwrap().peer_fingerprint = Some("KNOWN-FP".into());

        let session_id = Uuid::new_v4();
        let incoming = Uuid::new_v4();
        mgr.create_local_chat_for_test(incoming, "Peer".into());
        mgr.chat_id_mapping.insert(incoming, session_id);
        let (tx, mut rx) = mpsc::unbounded_channel();
        mgr.add_fingerprint_confirm_sender_for_test(session_id, tx);

        mgr.handle_tofu_verification(session_id, "KNOWN-FP", "Peer");

        // Returning peer: auto-confirmed without re-prompting.
        assert!(
            mgr.fingerprint_verification_request.is_none(),
            "a known fingerprint must not trigger another prompt"
        );
        assert_eq!(
            rx.try_recv().ok(),
            Some(true),
            "a known fingerprint must be auto-confirmed"
        );
    }

    #[test]
    fn parse_invite_with_valid_address_keeps_it() {
        let mgr = ChatManager::default();

        let payload = serde_json::json!({
            "name": "Bob",
            "address": "127.0.0.1:54321",
            "fingerprint": "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210",
            "public_key": "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkq...\n-----END PUBLIC KEY-----",
        });

        let json = serde_json::to_string(&payload).unwrap();
        use base64::engine::general_purpose;
        let encoded = general_purpose::STANDARD.encode(json);
        let link = format!("chat-p2p://invite/{}", encoded);

        let contact = mgr.parse_invite_link(&link).expect("should parse invite");
        assert_eq!(contact.address, Some("127.0.0.1:54321".to_string()));
    }

    #[test]
    fn parse_invite_invalid_address_no_port() {
        let mgr = ChatManager::default();

        let payload = serde_json::json!({
            "name": "Charlie",
            "address": "127.0.0.1",
            "fingerprint": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "public_key": "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkq...\n-----END PUBLIC KEY-----",
        });

        let json = serde_json::to_string(&payload).unwrap();
        use base64::engine::general_purpose;
        let encoded = general_purpose::STANDARD.encode(json);
        let link = format!("chat-p2p://invite/{}", encoded);

        let contact = mgr.parse_invite_link(&link).expect("should parse invite");
        assert!(
            contact.address.is_none(),
            "address without port should be None"
        );
    }

    #[test]
    fn parse_invite_invalid_address_bad_port() {
        let mgr = ChatManager::default();

        let payload = serde_json::json!({
            "name": "Dana",
            "address": "127.0.0.1:notaport",
            "fingerprint": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "public_key": "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkq...\n-----END PUBLIC KEY-----",
        });

        let json = serde_json::to_string(&payload).unwrap();
        use base64::engine::general_purpose;
        let encoded = general_purpose::STANDARD.encode(json);
        let link = format!("chat-p2p://invite/{}", encoded);

        let contact = mgr.parse_invite_link(&link).expect("should parse invite");
        assert!(
            contact.address.is_none(),
            "address with non-numeric port should be None"
        );
    }

    #[test]
    fn placeholder_detection_works() {
        let mut mgr = ChatManager::new(Config::default());
        let port = 5001u16;
        assert!(!mgr.chats.values().any(|c| c.is_host_placeholder));
        let chat = Chat {
            id: Uuid::new_v4(),
            title: format!("Host on :{}", port),
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
            is_host_placeholder: true,
        };
        let id = chat.id;
        mgr.chats.insert(id, chat);
        assert!(mgr.chats.values().any(|c| c.is_host_placeholder));
    }

    #[test]
    fn test_tofu_logic() {
        let mut mgr = ChatManager::default();
        let chat_id = Uuid::new_v4();
        let peer_name = "peer".to_string();
        let fingerprint1 = "fingerprint1".to_string();
        let fingerprint2 = "fingerprint2".to_string();

        // 1. First Use: No fingerprint exists.
        let chat = Chat {
            id: chat_id,
            title: "Test Chat".to_string(),
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
        // Add a dummy confirmation sender for the test
        let (tx, mut rx) = mpsc::unbounded_channel();
        mgr.fingerprint_confirm_senders.insert(chat_id, tx);
        mgr.chats.insert(chat_id, chat);
        // 1. First Use: No fingerprint exists -> UI prompt expected (no auto-confirm)
        let event1 = SessionEvent::ShowFingerprintVerification {
            fingerprint: fingerprint1.clone(),
            peer_name: peer_name.clone(),
            chat_id,
        };
        mgr.handle_session_event(chat_id, event1);

        // Assert: No auto-storage, request pending, and no confirmation sent automatically.
        assert_eq!(mgr.chats.get(&chat_id).unwrap().peer_fingerprint, None);
        assert!(mgr.fingerprint_verification_request.is_some());
        assert!(rx.try_recv().is_err());

        // Simulate user accepting the fingerprint via UI
        mgr.confirm_fingerprint(chat_id, true)
            .expect("confirm should succeed");
        // Now the session should receive confirmation
        assert_eq!(rx.try_recv(), Ok(true));
        // And the fingerprint should now be stored
        assert_eq!(
            mgr.chats.get(&chat_id).unwrap().peer_fingerprint,
            Some(fingerprint1.clone())
        );

        // 2. Second Use: Matching fingerprint -> auto-confirm
        let event2 = SessionEvent::ShowFingerprintVerification {
            fingerprint: fingerprint1.clone(),
            peer_name: peer_name.clone(),
            chat_id,
        };
        mgr.handle_session_event(chat_id, event2);

        // Assert: No UI request, and connection is confirmed automatically.
        assert!(mgr.fingerprint_verification_request.is_none());
        assert_eq!(rx.try_recv(), Ok(true));

        // 3. Third Use: Mismatched fingerprint -> UI prompt, no auto-confirm
        let event3 = SessionEvent::ShowFingerprintVerification {
            fingerprint: fingerprint2.clone(),
            peer_name: peer_name.clone(),
            chat_id,
        };
        mgr.handle_session_event(chat_id, event3);

        // Assert: A UI request IS made, and no confirmation is sent automatically.
        assert!(mgr.fingerprint_verification_request.is_some());
        let (fp, _, _) = mgr.fingerprint_verification_request.clone().unwrap();
        assert_eq!(fp, fingerprint2);
        assert!(rx.try_recv().is_err());
    }

    /// PHASE 0 REGRESSION TEST: auto_trust_on_first_use must default to false.
    /// If this test fails, the TOFU auto-trust MEDIUM-priority issue is present.
    /// auto_trust_on_first_use=true silently accepts first-contact MITM without verification prompt.
    #[test]
    fn test_regression_auto_trust_default_off() {
        let config = Config::default();
        assert!(
            !config.auto_trust_on_first_use,
            "auto_trust_on_first_use MUST default to false for security"
        );

        // When auto_trust_on_first_use=false (the default), first fingerprint should require user verification.
        let mut mgr = ChatManager::new(config);
        let chat_id = Uuid::new_v4();

        let chat = Chat {
            id: chat_id,
            title: "Test Chat".to_string(),
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

        let (tx, _rx) = mpsc::unbounded_channel();
        mgr.fingerprint_confirm_senders.insert(chat_id, tx);
        mgr.chats.insert(chat_id, chat);

        // When a new fingerprint is received (first use), it should prompt the user
        let event = SessionEvent::ShowFingerprintVerification {
            fingerprint: "first_fingerprint".to_string(),
            peer_name: "Alice".to_string(),
            chat_id,
        };

        mgr.handle_session_event(chat_id, event);

        // With auto_trust=false, fingerprint should NOT be auto-stored
        assert_eq!(mgr.chats.get(&chat_id).unwrap().peer_fingerprint, None);
        // And a verification request should be pending
        assert!(
            mgr.fingerprint_verification_request.is_some(),
            "Should show fingerprint verification dialog when auto_trust_on_first_use=false"
        );
    }

    /// PHASE 0 REGRESSION TEST: mDNS discovery should be disabled by default.
    /// If this test fails, the mDNS metadata exposure LOW-priority issue is present.
    /// Enabling mDNS broadcasts fingerprint + hostname on the local network.
    #[test]
    fn test_regression_mdns_default_off() {
        let config = Config::default();
        assert!(
            !config.enable_mdns,
            "enable_mdns MUST default to false for privacy (LAN fingerprint disclosure risk)"
        );
    }

    // ============================================================================
    // Signed Invite Link (v2) Parsing Tests
    // ============================================================================

    #[test]
    fn parse_v2_signed_invite_link_valid() {
        use crate::identity::Identity;

        let mgr = ChatManager::default();
        let identity = Identity::new_with_plaintext("Test Signer".to_string()).unwrap();

        // Generate a v2 signed invite
        let link = identity
            .generate_signed_invite_link(Some("127.0.0.1:9001".to_string()))
            .unwrap();

        // Parse it
        let contact = mgr
            .parse_invite_link(&link)
            .expect("should parse v2 signed invite");

        // Verify contact fields
        assert_eq!(contact.name, "Test Signer");
        assert_eq!(contact.address, Some("127.0.0.1:9001".to_string()));
        assert_eq!(contact.fingerprint, Some(identity.fingerprint.clone()));
        assert_eq!(contact.public_key, Some(identity.public_key_pem.clone()));
        assert_eq!(contact.trust_state, TrustState::Unverified);
    }

    #[test]
    fn parse_v2_signed_invite_rejects_tampered_signature() {
        use crate::identity::Identity;
        use base64::Engine;

        let mgr = ChatManager::default();
        let identity = Identity::new_with_plaintext("Tamper Test".to_string()).unwrap();

        // Generate a v2 signed invite
        let link = identity.generate_signed_invite_link(None).unwrap();
        let encoded = link.strip_prefix("chat-p2p://invite/v2/").unwrap();

        // Decode, tamper with signature, re-encode
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .unwrap();
        let json_str = String::from_utf8(decoded).unwrap();

        #[derive(serde::Deserialize, serde::Serialize)]
        struct SignedInvite {
            payload: serde_json::Value,
            signature: Vec<u8>,
        }

        let mut invite: SignedInvite = serde_json::from_str(&json_str).unwrap();
        // Flip a bit in the signature to tamper with it
        if !invite.signature.is_empty() {
            invite.signature[0] ^= 0xFF;
        }

        let tampered_json = serde_json::to_string(&invite).unwrap();
        let tampered_encoded =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&tampered_json);
        let tampered_link = format!("chat-p2p://invite/v2/{}", tampered_encoded);

        // Parsing should fail due to signature verification
        assert!(
            mgr.parse_invite_link(&tampered_link).is_err(),
            "should reject tampered signature"
        );
    }

    #[test]
    fn parse_v1_invite_link_still_works_with_warning() {
        let mgr = ChatManager::default();

        let payload = serde_json::json!({
            "name": "Legacy User",
            "address": "192.168.1.50:8001",
            "fingerprint": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "public_key": "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkq...\n-----END PUBLIC KEY-----",
        });

        let json = serde_json::to_string(&payload).unwrap();
        let encoded = base64::engine::general_purpose::STANDARD.encode(json);
        let link = format!("chat-p2p://invite/{}", encoded);

        // Should still parse v1 unsigned invites (backward compatibility)
        let contact = mgr
            .parse_invite_link(&link)
            .expect("should parse v1 invite");
        assert_eq!(contact.name, "Legacy User");
        assert_eq!(contact.address, Some("192.168.1.50:8001".to_string()));
        assert_eq!(
            contact.fingerprint,
            Some("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_string())
        );
    }

    #[test]
    fn parse_v2_signed_invite_without_address() {
        use crate::identity::Identity;

        let mgr = ChatManager::default();
        let identity = Identity::new_with_plaintext("No Address User".to_string()).unwrap();

        // Generate a v2 signed invite without address
        let link = identity.generate_signed_invite_link(None).unwrap();

        // Parse it
        let contact = mgr
            .parse_invite_link(&link)
            .expect("should parse v2 signed invite without address");

        assert_eq!(contact.name, "No Address User");
        assert_eq!(contact.address, None);
        assert_eq!(contact.fingerprint, Some(identity.fingerprint.clone()));
    }

    #[test]
    fn parse_v2_signed_invite_preserves_identity_fields() {
        use crate::identity::Identity;

        let mgr = ChatManager::default();
        let identity = Identity::new_with_plaintext("Complete Info User".to_string()).unwrap();

        // Generate with full info
        let link = identity
            .generate_signed_invite_link(Some("10.20.30.40:6500".to_string()))
            .unwrap();

        let contact = mgr
            .parse_invite_link(&link)
            .expect("should parse v2 signed invite");

        // All fields should match the identity
        assert_eq!(contact.name, "Complete Info User");
        assert_eq!(contact.address, Some("10.20.30.40:6500".to_string()));
        assert_eq!(contact.fingerprint, Some(identity.fingerprint.clone()));
        assert_eq!(contact.public_key, Some(identity.public_key_pem.clone()));
    }

    #[test]
    fn v2_invite_signature_verification_prevents_fingerprint_swap() {
        use crate::identity::Identity;
        use base64::Engine;

        let mgr = ChatManager::default();
        let identity1 = Identity::new_with_plaintext("User One".to_string()).unwrap();
        let identity2 = Identity::new_with_plaintext("User Two".to_string()).unwrap();

        // Generate invite from identity1
        let link = identity1.generate_signed_invite_link(None).unwrap();
        let encoded = link.strip_prefix("chat-p2p://invite/v2/").unwrap();

        // Decode and try to swap the fingerprint with identity2's
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .unwrap();
        let json_str = String::from_utf8(decoded).unwrap();

        #[derive(serde::Deserialize, serde::Serialize)]
        struct SignedInvite {
            payload: serde_json::Value,
            signature: Vec<u8>,
        }

        let mut invite: SignedInvite = serde_json::from_str(&json_str).unwrap();

        // Swap fingerprint (attack attempt)
        invite.payload["fingerprint"] = serde_json::Value::String(identity2.fingerprint.clone());

        let tampered_json = serde_json::to_string(&invite).unwrap();
        let tampered_encoded =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&tampered_json);
        let tampered_link = format!("chat-p2p://invite/v2/{}", tampered_encoded);

        // Should reject because signature won't verify with modified payload
        assert!(
            mgr.parse_invite_link(&tampered_link).is_err(),
            "should reject invite with swapped fingerprint"
        );
    }

    #[tokio::test]
    async fn send_file_uses_monotonic_chat_sequence_space() {
        let mut mgr = ChatManager::new(Config::default());
        let chat_id = Uuid::new_v4();
        mgr.create_local_chat_for_test(chat_id, "File Seq Test".to_string());

        let (from_app_tx, mut from_app_rx) = mpsc::unbounded_channel();
        mgr.sessions.insert(chat_id, SessionHandle { from_app_tx });

        // Start from a non-zero value to ensure file transfer continues existing sequence space.
        mgr.chats.get_mut(&chat_id).unwrap().send_seq = 5;

        let temp_file = tempfile::NamedTempFile::new().unwrap();
        let content = vec![b'x'; crate::FILE_CHUNK_SIZE * 2 + 13];
        std::fs::write(temp_file.path(), content).unwrap();

        mgr.send_file(chat_id, temp_file.path().to_path_buf())
            .await
            .expect("send_file should succeed");

        let mut seqs = Vec::new();
        let mut saw_meta = false;
        let mut saw_end = false;
        let mut chunk_count = 0usize;
        while let Ok(msg) = from_app_rx.try_recv() {
            match msg {
                ProtocolMessage::FileMeta { seq, .. } => {
                    saw_meta = true;
                    seqs.push(seq);
                }
                ProtocolMessage::FileChunk { seq, .. } => {
                    chunk_count += 1;
                    seqs.push(seq);
                }
                ProtocolMessage::FileEnd { seq } => {
                    saw_end = true;
                    seqs.push(seq);
                }
                _ => {}
            }
        }

        assert!(saw_meta, "FileMeta should be emitted");
        assert!(saw_end, "FileEnd should be emitted");
        assert!(chunk_count >= 2, "Test file should produce multiple chunks");
        assert_eq!(
            seqs.first().copied(),
            Some(6),
            "Sequence should continue from chat.send_seq"
        );
        assert!(
            seqs.windows(2).all(|w| w[1] == w[0] + 1),
            "File transfer messages must use strictly increasing sequence numbers"
        );
    }

    #[test]
    fn large_incoming_message_reassembles_into_one_chat_message() {
        let mut mgr = ChatManager::new(Config::default());
        let chat_id = Uuid::new_v4();
        mgr.create_local_chat_for_test(chat_id, "Chunked Incoming".to_string());

        let text = "hello large world ".repeat(8_000);
        let timestamp = 123_456_789u64;
        let mut seq = 0u64;
        let messages = ChatManager::build_text_protocol_messages(&mut seq, &text, timestamp)
            .expect("large text should chunk successfully");
        assert!(messages.len() > 1, "test message should be chunked");

        for msg in messages {
            mgr.handle_session_event(chat_id, SessionEvent::MessageReceived(msg));
        }

        let chat = mgr.chats.get(&chat_id).unwrap();
        assert_eq!(chat.messages.len(), 1);
        match &chat.messages[0].content {
            MessageContent::Text { text: reassembled } => assert_eq!(reassembled, &text),
            other => panic!("expected text message, got {:?}", other),
        }
    }

    #[test]
    fn stale_incoming_large_message_is_discarded() {
        let mut mgr = ChatManager::new(Config::default());
        let chat_id = Uuid::new_v4();
        let message_id = Uuid::new_v4();

        mgr.incoming_text_messages.insert(
            (chat_id, message_id),
            IncomingTextMessage {
                timestamp_millis: 1,
                parts: vec![Some("partial".to_string()), None],
                updated_at: std::time::Instant::now() - Duration::from_secs(121),
            },
        );

        mgr.cleanup_stale_incoming_text_messages();

        assert!(!mgr
            .incoming_text_messages
            .contains_key(&(chat_id, message_id)));
        assert!(
            mgr.toasts
                .iter()
                .any(|toast| toast.message.contains("large incoming message")),
            "cleanup should surface a warning toast"
        );
    }

    #[test]
    fn mapped_session_ping_updates_actual_chat() {
        let mut mgr = ChatManager::new(Config::default());
        let session_chat_id = Uuid::new_v4();
        let actual_chat_id = Uuid::new_v4();

        mgr.chat_id_mapping.insert(actual_chat_id, session_chat_id);
        mgr.create_local_chat_for_test(actual_chat_id, "Mapped Chat".to_string());
        mgr.chats.get_mut(&actual_chat_id).unwrap().recv_seq = 0;

        mgr.handle_session_event(
            session_chat_id,
            SessionEvent::MessageReceived(ProtocolMessage::Ping { seq: 1 }),
        );

        assert_eq!(
            mgr.chats.get(&actual_chat_id).unwrap().recv_seq,
            1,
            "Ping sequence must be applied to mapped actual chat"
        );
    }

    #[test]
    fn sequential_incoming_files_do_not_reuse_completed_transfer_state() {
        let temp_dir = tempdir().unwrap();
        let download_dir = temp_dir.path().join("downloads");
        let temp_download_dir = temp_dir.path().join("temp");
        let config = Config {
            download_dir: download_dir.clone(),
            temp_dir: temp_download_dir,
            ..Config::default()
        };

        let mut mgr = ChatManager::new(config);
        let chat_id = Uuid::new_v4();
        mgr.create_local_chat_for_test(chat_id, "Sequential Files".to_string());

        let first_payload = b"first file payload";
        let second_payload = b"second payload";

        mgr.handle_session_event(
            chat_id,
            SessionEvent::MessageReceived(ProtocolMessage::FileMeta {
                filename: "first.txt".to_string(),
                size: first_payload.len() as u64,
                seq: 1,
            }),
        );
        mgr.handle_session_event(
            chat_id,
            SessionEvent::MessageReceived(ProtocolMessage::FileChunk {
                chunk: first_payload.to_vec(),
                seq: 2,
            }),
        );
        mgr.handle_session_event(
            chat_id,
            SessionEvent::MessageReceived(ProtocolMessage::FileEnd { seq: 3 }),
        );

        mgr.handle_session_event(
            chat_id,
            SessionEvent::MessageReceived(ProtocolMessage::FileMeta {
                filename: "second.txt".to_string(),
                size: second_payload.len() as u64,
                seq: 4,
            }),
        );
        mgr.handle_session_event(
            chat_id,
            SessionEvent::MessageReceived(ProtocolMessage::FileChunk {
                chunk: second_payload.to_vec(),
                seq: 5,
            }),
        );
        mgr.handle_session_event(
            chat_id,
            SessionEvent::MessageReceived(ProtocolMessage::FileEnd { seq: 6 }),
        );

        let chat = mgr.chats.get(&chat_id).unwrap();
        let file_messages: Vec<_> = chat
            .messages
            .iter()
            .filter_map(|message| match &message.content {
                MessageContent::File {
                    path: Some(path), ..
                } => Some(path.clone()),
                _ => None,
            })
            .collect();

        assert_eq!(file_messages.len(), 2, "both files should be recorded");
        assert!(
            mgr.active_transfers.is_empty(),
            "completed transfers should not remain active"
        );
        assert!(
            mgr.incoming_files.is_empty(),
            "no incoming file handles should remain after completion"
        );
        assert_eq!(
            std::fs::read(&file_messages[0]).unwrap(),
            first_payload,
            "first file should keep its payload"
        );
        assert_eq!(
            std::fs::read(&file_messages[1]).unwrap(),
            second_payload,
            "second file should keep its payload"
        );
    }

    #[test]
    fn parse_v2_signed_invite_normalizes_ipv6_address() {
        use crate::identity::Identity;

        let mgr = ChatManager::default();
        let identity = Identity::new_with_plaintext("IPv6 User".to_string()).unwrap();
        let link = identity
            .generate_signed_invite_link(Some("[2001:db8::1]:12345".to_string()))
            .unwrap();

        let contact = mgr.parse_invite_link(&link).expect("should parse invite");
        assert_eq!(contact.address.as_deref(), Some("[2001:db8::1]:12345"));
    }

    #[test]
    fn parse_v2_signed_invite_drops_unbracketed_ipv6_with_port() {
        use crate::identity::Identity;

        let mgr = ChatManager::default();
        let identity = Identity::new_with_plaintext("Broken IPv6".to_string()).unwrap();
        let link = identity
            .generate_signed_invite_link(Some("2001:db8::1:12345".to_string()))
            .unwrap();

        let contact = mgr
            .parse_invite_link(&link)
            .expect("invite itself should remain valid");
        assert_eq!(
            contact.address, None,
            "invalid address payloads should be dropped instead of normalized"
        );
    }

    #[test]
    fn parse_v3_signed_invite_keeps_relay_route() {
        use crate::identity::Identity;

        let mgr = ChatManager::default();
        let identity = Identity::new_with_plaintext("Relay Invite User".to_string()).unwrap();
        let relay_token = "0123456789abcdef0123456789abcdef".to_string();
        let link = identity
            .generate_signed_invite_link_with_route(
                None,
                Some("relay.example.com:23456".to_string()),
                Some(relay_token.clone()),
            )
            .unwrap();

        let contact = mgr
            .parse_invite_link(&link)
            .expect("should parse relay invite");
        assert_eq!(
            contact.relay_server.as_deref(),
            Some("relay.example.com:23456")
        );
        assert_eq!(contact.relay_token.as_deref(), Some(relay_token.as_str()));
        assert_eq!(contact.address, None);
    }

    #[test]
    fn delete_all_data_removes_files_and_clears_state() {
        let dir = tempdir().unwrap();
        let data_dir = dir.path().join("data");
        let history_path = data_dir.join("history.json.enc");
        let identity_path = data_dir.join("identity.json");
        let crash_log_path = data_dir
            .join("diagnostics")
            .join("crashes")
            .join("panic.log");

        std::fs::create_dir_all(crash_log_path.parent().unwrap()).unwrap();

        std::fs::write(&history_path, b"encrypted-history").unwrap();
        std::fs::write(&identity_path, b"encrypted-identity").unwrap();
        std::fs::write(&crash_log_path, b"crash").unwrap();

        let mut mgr = ChatManager::new(Config::default());
        let chat_id = Uuid::new_v4();
        let contact_id = mgr.add_contact(
            "Contact".to_string(),
            Some("127.0.0.1:12345".to_string()),
            None,
            None,
        );
        mgr.create_local_chat_for_test(chat_id, "Chat".to_string());
        mgr.contact_to_chat.insert(contact_id, chat_id);
        mgr.fingerprint_verification_request =
            Some(("fingerprint".to_string(), "peer".to_string(), chat_id));

        mgr.delete_all_data(&data_dir, &history_path, &identity_path)
            .unwrap();

        assert!(!data_dir.exists(), "app data directory should be deleted");
        assert!(!history_path.exists(), "history file should be deleted");
        assert!(!identity_path.exists(), "identity file should be deleted");
        assert!(!crash_log_path.exists(), "diagnostics should be deleted");
        assert!(mgr.chats.is_empty());
        assert!(mgr.contacts.is_empty());
        assert!(mgr.contact_to_chat.is_empty());
        assert!(mgr.fingerprint_verification_request.is_none());
    }
}
