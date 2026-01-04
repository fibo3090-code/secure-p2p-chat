//! Chat management and application state orchestration.
//!
//! Provides the `ChatManager` which coordinates:
//! - Contacts and chats lifecycle (create, rename, group chats)
//! - Network sessions and event handling (`SessionEvent`)
//! - Message routing and typing indicators
//! - File transfer state and toasts/notifications
//! - Invite link generation and parsing (including QR codes)

use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::core::{ProtocolMessage, generate_rsa_keypair_async};
use crate::network::{run_client_session, run_host_session};
use crate::transfer::IncomingFileSync;
use crate::types::*;

/// Session handle for communication with network task
#[derive(Clone)]
pub struct SessionHandle {
    pub from_app_tx: mpsc::UnboundedSender<ProtocolMessage>,
}

/// Main chat manager - orchestrates sessions, messages, and file transfers
#[derive(Clone)]
pub struct ChatManager {
    pub chats: HashMap<Uuid, Chat>,
    pub contacts: HashMap<Uuid, Contact>,
    /// Map contact_id -> one-to-one chat id (if any). Used to find session/chat for a contact.
    pub contact_to_chat: HashMap<Uuid, Uuid>,
    sessions: HashMap<Uuid, SessionHandle>,
    session_events: HashMap<Uuid, Arc<Mutex<mpsc::UnboundedReceiver<SessionEvent>>>>,
    /// Channels used to confirm fingerprint verification with the running session task
    fingerprint_confirm_senders: HashMap<Uuid, mpsc::UnboundedSender<bool>>,
    active_transfers: HashMap<Uuid, FileTransferState>,
    #[allow(dead_code)] // Reserved for future file transfer implementation
    incoming_files: HashMap<Uuid, IncomingFileSync>,
    pub toasts: Vec<Toast>,
    pub config: Config,
    pub fingerprint_verification_request: Option<(String, String, Uuid)>,
}

impl ChatManager {
    /// Parse an address of the form host:port
    /// Returns (host, port) or an error if the format is invalid.
    fn parse_address(address: &str) -> Result<(String, u16)> {
        let parts: Vec<&str> = address.split(':').collect();
        if parts.len() != 2 {
            tracing::error!("Invalid address format for contact: {}", address);
            return Err(anyhow::anyhow!("Invalid address format for contact"));
        }
        let host = parts[0].trim();
        let port: u16 = parts[1]
            .trim()
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid port in contact address"))?;
        if host.is_empty() {
            tracing::error!("Host is empty in contact address: {}", address);
            return Err(anyhow::anyhow!("Host is empty in contact address"));
        }
        Ok((host.to_string(), port))
    }

    pub fn new(config: Config) -> Self {
        Self {
            chats: HashMap::new(),
            contacts: HashMap::new(),
            contact_to_chat: HashMap::new(),
            sessions: HashMap::new(),
            session_events: HashMap::new(),
            active_transfers: HashMap::new(),
            incoming_files: HashMap::new(),
            toasts: Vec::new(),
            config,
            fingerprint_verification_request: None,
            fingerprint_confirm_senders: HashMap::new(),
        }
    }

    /// Helper to create a local chat for testing purposes without network sessions
    pub fn create_local_chat_for_test(&mut self, id: Uuid, title: String) {
        let chat = Chat {
            id,
            title,
            peer_fingerprint: None,
            participants: Vec::new(),
            messages: Vec::new(),
            created_at: chrono::Utc::now(),
            peer_typing: false,
            typing_since: None,
            send_seq: 0,
            recv_seq: 0,
        };
        self.chats.insert(id, chat);
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
    pub async fn auto_reconnect_contacts(&mut self) {
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
                .connect_to_contact(contact_id, Some(mapped_chat_id))
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
            peer_fingerprint: None,
            participants,
            messages: Vec::new(),
            created_at: chrono::Utc::now(),
            peer_typing: false,
            typing_since: None,
            send_seq: 0,
            recv_seq: 0,
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
                            chat.send_seq += 1;
                            let msg = ProtocolMessage::Text {
                                text: text.clone(),
                                timestamp: crate::util::current_timestamp_millis(),
                                seq: chat.send_seq,
                            };
                            if session.from_app_tx.send(msg).is_ok() {
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
        let title = if new_title.len() > 50 {
            new_title[..50].to_string()
        } else {
            new_title
        };

        if let Some(chat) = self.chats.get_mut(&chat_id) {
            chat.title = title;
            Ok(())
        } else {
            tracing::error!("Chat not found for rename: {}", chat_id);
            Err(anyhow::anyhow!("Chat not found"))
        }
    }

    fn has_placeholder_host(&self, port: u16) -> bool {
        let expected = format!("Host on :{}", port);
        self.chats.values().any(|c| c.title == expected)
    }

    /// Start hosting on specified port
    pub async fn start_host(&mut self, port: u16) -> Result<Uuid> {
        // Guard: if a placeholder host already exists for this port, avoid spawning another.
        // If the placeholder has no active session, clean it up so we can rehost.
        if self.has_placeholder_host(port) {
            let expected = format!("Host on :{}", port);
            let placeholder_ids: Vec<Uuid> = self
                .chats
                .iter()
                .filter(|(_, c)| c.title == expected)
                .map(|(id, _)| *id)
                .collect();
            let has_active_session = placeholder_ids
                .iter()
                .any(|id| self.sessions.contains_key(id));

            if has_active_session {
                self.add_toast(
                    ToastLevel::Info,
                    format!("Already listening on port {}", port),
                );
                tracing::info!(port = %port, "start_host skipped: active placeholder host exists");
                return Err(anyhow::anyhow!("Already listening on port {}", port));
            }

            for id in placeholder_ids {
                self.chats.remove(&id);
                self.sessions.remove(&id);
                self.session_events.remove(&id);
                self.fingerprint_confirm_senders.remove(&id);
            }
            tracing::warn!(port = %port, "Removed stale placeholder host before restarting");
        }
        let chat_id = Uuid::new_v4();
        tracing::info!(chat_id = %chat_id, port = %port, "start_host called");
        let privkey = generate_rsa_keypair_async(2048).await?;

        // Create channels
        let (to_app_tx, to_app_rx) = mpsc::unbounded_channel();
        let (from_app_tx, from_app_rx) = mpsc::unbounded_channel();

        // Create confirmation channel so UI can accept/reject the fingerprint
        let (confirm_tx, confirm_rx) = mpsc::unbounded_channel();

        // Spawn session task
        tokio::spawn(async move {
            if let Err(e) =
                run_host_session(port, privkey, to_app_tx, from_app_rx, confirm_rx, chat_id).await
            {
                tracing::error!("Host session error: {}", e);
            }
        });

        // Create chat entry
        let chat = Chat {
            id: chat_id,
            title: format!("Host on :{}", port),
            peer_fingerprint: None,
            participants: Vec::new(),
            messages: Vec::new(),
            created_at: chrono::Utc::now(),
            peer_typing: false,
            typing_since: None,
            send_seq: 0,
            recv_seq: 0,
        };

        self.chats.insert(chat_id, chat);
        self.sessions.insert(chat_id, SessionHandle { from_app_tx });
        self.session_events
            .insert(chat_id, Arc::new(Mutex::new(to_app_rx)));
        self.fingerprint_confirm_senders.insert(chat_id, confirm_tx);

        self.add_toast(ToastLevel::Info, format!("Listening on port {}", port));
        tracing::debug!(chat_count = %self.chats.len(), session_count = %self.sessions.len(), "Host session initialized");

        Ok(chat_id)
    }

    /// Connect to a host
    pub async fn connect_to_host(
        &mut self,
        host: &str,
        port: u16,
        existing_chat_id: Option<Uuid>,
    ) -> Result<Uuid> {
        let chat_id = existing_chat_id.unwrap_or_else(Uuid::new_v4);
        tracing::info!(chat_id = %chat_id, host = %host, port = %port, "connect_to_host called");
        let privkey = generate_rsa_keypair_async(2048).await?;

        let (to_app_tx, to_app_rx) = mpsc::unbounded_channel();
        let (from_app_tx, from_app_rx) = mpsc::unbounded_channel();

        let host_copy = host.to_string();
        let (confirm_tx, confirm_rx) = mpsc::unbounded_channel();

        tokio::spawn(async move {
            if let Err(e) = run_client_session(
                &host_copy,
                port,
                privkey,
                to_app_tx,
                from_app_rx,
                confirm_rx,
                chat_id,
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
                peer_fingerprint: None,
                participants: Vec::new(),
                messages: Vec::new(),
                created_at: chrono::Utc::now(),
                peer_typing: false,
                typing_since: None,
                send_seq: 0,
                recv_seq: 0,
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

    pub async fn connect_to_contact(
        &mut self,
        contact_id: Uuid,
        existing_chat_id: Option<Uuid>,
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
                    let chat_id = self.connect_to_host(&host, port, Some(mapped)).await?;
                    self.associate_contact_with_chat(contact_id, chat_id);
                    return Ok(chat_id);
                }
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
            let chat_id = self.connect_to_host(&host, port, existing_chat_id).await?;
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
        // Determine if this is a true group chat
        let (participants_len, has_session) = if let Some(chat) = self.chats.get(&chat_id) {
            (
                chat.participants.len(),
                self.sessions.contains_key(&chat_id),
            )
        } else {
            (0, false)
        };

        let is_group_chat = participants_len >= 2;
        tracing::debug!(
            "chat classification: is_group_chat={}, participants_len={}, has_session={}",
            is_group_chat,
            participants_len,
            has_session
        );

        if is_group_chat {
            tracing::info!("Sending as group message to chat {}", chat_id);
            self.send_group_message(chat_id, text)?;
            return Ok(());
        }

        // One-to-one chat path
        if !has_session {
            tracing::warn!(
                "No active session for 1:1 chat {} yet. Likely still connecting.",
                chat_id
            );
            self.add_toast(
                ToastLevel::Info,
                "Connecting... please wait before sending messages".to_string(),
            );
            return Ok(()); // Do not error; just inform the user and skip sending
        }

        let session = self
            .sessions
            .get(&chat_id)
            .ok_or_else(|| anyhow::anyhow!("Session should exist but was not found"))?;

        let chat = self
            .chats
            .get_mut(&chat_id)
            .ok_or_else(|| anyhow::anyhow!("Chat not found for sending message"))?;

        chat.send_seq += 1;
        let msg = ProtocolMessage::Text {
            text: text.clone(),
            timestamp: crate::util::current_timestamp_millis(),
            seq: chat.send_seq,
        };

        if let Err(e) = session.from_app_tx.send(msg) {
            tracing::error!("Failed to send message to chat {}: {}", chat_id, e);
            return Err(e.into());
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
        let should_notify = if let Some(transfer) = self.active_transfers.get_mut(&transfer_id) {
            transfer.received = bytes;
            if bytes == transfer.size {
                transfer.status = TransferStatus::Completed;
                Some(transfer.filename.clone())
            } else {
                None
            }
        } else {
            None
        };

        if let Some(filename) = should_notify {
            self.add_toast(ToastLevel::Success, format!("File received: {}", filename));
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

        let session = self
            .sessions
            .get(&chat_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found"))?;

        let chat = self
            .chats
            .get_mut(&chat_id)
            .ok_or_else(|| anyhow::anyhow!("Chat not found"))?;
        chat.send_seq += 1;
        session.from_app_tx.send(ProtocolMessage::TypingStart { seq: chat.send_seq })?;
        Ok(())
    }

    /// Send typing stop indicator
    pub fn send_typing_stop(&mut self, chat_id: Uuid) -> Result<()> {
        if !self.config.enable_typing_indicators {
            return Ok(());
        }

        let session = self
            .sessions
            .get(&chat_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found"))?;

        let chat = self
            .chats
            .get_mut(&chat_id)
            .ok_or_else(|| anyhow::anyhow!("Chat not found"))?;
        chat.send_seq += 1;
        session.from_app_tx.send(ProtocolMessage::TypingStop { seq: chat.send_seq })?;
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
        self.sessions.contains_key(chat_id)
    }

    /// Get all chat IDs
    pub fn chat_ids(&self) -> Vec<Uuid> {
        self.chats.keys().copied().collect()
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
        self.toasts.clear();
        self.fingerprint_verification_request = None;

        // Save empty history to disk
        let _ = self.save_history(history_path);
        tracing::info!("History cleared and saved");
    }

    /// Send the user's accept/reject decision for a fingerprint verification to the session task
    pub fn confirm_fingerprint(&mut self, chat_id: Uuid, accept: bool) -> Result<()> {
        tracing::info!(chat_id = %chat_id, accept = %accept, "Confirming fingerprint");
        if let Some(tx) = self.fingerprint_confirm_senders.get(&chat_id) {
            // If the user accepted and we have a pending verification request matching this chat,
            // persist the fingerprint in the chat record before confirming the session.
            if accept {
                if let Some((fp, _peer_name, req_chat_id)) = &self.fingerprint_verification_request {
                    if *req_chat_id == chat_id {
                        if let Some(chat) = self.chats.get_mut(&chat_id) {
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
        let session = self
            .sessions
            .get(&chat_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found"))?;

        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow::anyhow!("Invalid filename"))?
            .to_string();

        let file_size = tokio::fs::metadata(&path).await?.len();
        tracing::debug!(file = %filename, size = %file_size, "Sending file metadata");

        if file_size > crate::MAX_FILE_SIZE {
            tracing::error!(
                "File is too large to send: {} > {}",
                file_size,
                crate::MAX_FILE_SIZE
            );
            self.add_toast(
                ToastLevel::Error,
                format!(
                    "File is too large ({} > {} bytes)",
                    file_size,
                    crate::MAX_FILE_SIZE
                ),
            );
            return Err(anyhow::anyhow!("File is too large"));
        }

        // Send file metadata
        let meta_msg = {
            let chat = self
                .chats
                .get_mut(&chat_id)
                .ok_or_else(|| anyhow::anyhow!("Chat not found for sending file"))?;
            chat.send_seq += 1;
            ProtocolMessage::FileMeta {
                filename: filename.clone(),
                size: file_size,
                seq: chat.send_seq,
            }
        };
        session.from_app_tx.send(meta_msg)?;

        // Send file chunks
        let mut file = File::open(&path).await?;
        let mut buffer = vec![0u8; crate::FILE_CHUNK_SIZE];
        let mut seq = 0u64;

        loop {
            let n = file.read(&mut buffer).await?;
            if n == 0 {
                break; // EOF
            }

            let chunk_msg = ProtocolMessage::FileChunk {
                chunk: buffer[..n].to_vec(),
                seq,
            };
            session.from_app_tx.send(chunk_msg)?;
            seq += 1;
            if seq.is_multiple_of(64) {
                tracing::trace!(sent_chunks = %seq, "File sending progress");
            }
        }

        // Send end marker
        session.from_app_tx.send(ProtocolMessage::FileEnd { seq })?;
        tracing::info!(file = %filename, total_bytes = %file_size, "File send complete");

        // Add to local history
        if let Some(chat) = self.chats.get_mut(&chat_id) {
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
        }

        self.add_toast(ToastLevel::Success, format!("File sent: {}", filename));

        Ok(())
    }

    /// Poll and process all pending session events
    pub fn poll_session_events(&mut self) {
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
                }
            }

            SessionEvent::NewConnection {
                peer_addr,
                fingerprint,
                chat_id: incoming_chat_id,
            } => {
                tracing::info!(
                    "New incoming connection from {} with chat_id {}",
                    peer_addr,
                    incoming_chat_id
                );
                // Create a chat for this new connection
                self.chats.entry(incoming_chat_id).or_insert_with(|| Chat {
                    id: incoming_chat_id,
                    title: peer_addr.clone(),
                    peer_fingerprint: Some(fingerprint.clone()),
                    participants: Vec::new(),
                    messages: Vec::new(),
                    created_at: chrono::Utc::now(),
                    peer_typing: false,
                    typing_since: None,
                    send_seq: 0,
                    recv_seq: 0,
                });
                self.add_toast(
                    ToastLevel::Info,
                    format!("New connection from {}", peer_addr),
                );
            }

            SessionEvent::ShowFingerprintVerification {
                fingerprint,
                peer_name,
                chat_id,
            } => {
                let chat = match self.chats.get_mut(&chat_id) {
                    Some(c) => c,
                    None => {
                        tracing::error!("TOFU check failed: Chat {} not found", chat_id);
                        return;
                    }
                };

                match &chat.peer_fingerprint {
                    // CASE 1: No stored fingerprint. This is the FIRST USE.
                    None => {
                        tracing::info!("Trust on First Use for chat {}. Requesting user confirmation.", chat_id);
                        // If user opted into auto-trust (not recommended), allow it.
                        if self.config.auto_trust_on_first_use {
                            tracing::info!("auto_trust_on_first_use enabled: auto-storing fingerprint for chat {}", chat_id);
                            chat.peer_fingerprint = Some(fingerprint.clone());
                            if let Some(tx) = self.fingerprint_confirm_senders.get(&chat_id) {
                                if let Err(e) = tx.send(true) {
                                    tracing::error!("Failed to auto-confirm fingerprint: {}", e);
                                }
                            }
                        } else {
                            // Request explicit user verification via UI
                            self.fingerprint_verification_request = Some((fingerprint, peer_name, chat_id));
                            self.add_toast(ToastLevel::Warning, "Fingerprint verification required".to_string());
                        }
                    }
                    // CASE 2: Stored fingerprint matches the new one.
                    Some(stored_fp) if stored_fp == &fingerprint => {
                        tracing::debug!("Fingerprint matches stored value for chat {}. Proceeding automatically.", chat_id);
                        // Automatically confirm this connection.
                        if let Some(tx) = self.fingerprint_confirm_senders.get(&chat_id) {
                            if let Err(e) = tx.send(true) {
                                tracing::error!("Failed to auto-confirm fingerprint: {}", e);
                            }
                        }
                    }
                    // CASE 3: Stored fingerprint MISMATCHES. Security alert!
                    Some(stored_fp) => {
                        tracing::warn!(
                            "FINGERPRINT MISMATCH for chat {}! Stored: `{}`, New: `{}`",
                            chat_id,
                            stored_fp,
                            fingerprint
                        );
                        // Trigger the UI dialog for manual verification.
                        self.fingerprint_verification_request = Some((fingerprint, peer_name, chat_id));
                        self.add_toast(ToastLevel::Warning, "SECURITY WARNING: Peer fingerprint has changed!".to_string());
                    }
                }
            }

            SessionEvent::Ready => {
                tracing::info!("Session {} is ready", chat_id);
                self.add_toast(ToastLevel::Success, "Connection established!".to_string());
            }

            SessionEvent::MessageReceived(proto_msg) => {
                tracing::debug!("Session {} received message: {:?}", chat_id, proto_msg);

                match proto_msg {
                    ProtocolMessage::Text { text, seq, .. } => {
                        if let Some(chat) = self.chats.get_mut(&chat_id) {
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
                                let preview = if text.len() > 50 {
                                    format!("{}...", &text[..50])
                                } else {
                                    text.clone()
                                };
                                self.show_notification("New message", &preview);

                                tracing::info!("Added received message to chat {}", chat_id);
                            } else {
                                tracing::warn!("Received message with invalid sequence number for chat {}. Expected > {}, got {}. Discarding.", chat_id, chat.recv_seq, seq);
                            }
                        } else {
                            tracing::error!("Chat {} not found for received message", chat_id);
                        }
                    }

                    ProtocolMessage::FileMeta { filename, size, seq } => {
                        if let Some(chat) = self.chats.get_mut(&chat_id) {
                            if seq > chat.recv_seq {
                                chat.recv_seq = seq;
                                tracing::info!("Received file metadata: {} ({} bytes)", filename, size);

                                match self.start_receiving_file(chat_id, &filename, size) {
                                    Ok(transfer_id) => {
                                        // Create new IncomingFileSync for this transfer
                                        let file_path = self.config.download_dir.join(&filename);

                                        match IncomingFileSync::new(&file_path, size) {
                                            Ok(incoming) => {
                                                self.incoming_files.insert(transfer_id, incoming);
                                            }
                                            Err(e) => {
                                                tracing::error!("Failed to create incoming file: {}", e);
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
                        let transfer_id = self.active_transfers.values().find(|t| t.chat_id == chat_id).map(|t| t.id);
                        if let Some(transfer_id) = transfer_id {
                            if let Some(transfer) = self.active_transfers.get_mut(&transfer_id) {
                                if seq == transfer.seq {
                                    transfer.seq += 1;
                                    tracing::debug!("Received file chunk {} ({} bytes)", seq, chunk.len());

                                    if let Some(incoming) = self.incoming_files.get_mut(&transfer_id) {
                                        if let Err(e) = incoming.write_chunk(&chunk) {
                                            tracing::error!("Failed to write chunk: {}", e);
                                            self.add_toast(
                                                ToastLevel::Error,
                                                format!("File transfer error: {}", e),
                                            );
                                        } else {
                                            let bytes_received = incoming.bytes_received();
                                            self.update_transfer_progress(transfer_id, bytes_received);
                                        }
                                    }
                                } else {
                                    tracing::warn!("Received file chunk with invalid sequence number for transfer {}. Expected {}, got {}. Discarding.", transfer_id, transfer.seq, seq);
                                }
                            }
                        }
                    }

                    ProtocolMessage::FileEnd { seq } => {
                        let transfer_id = self.active_transfers.values().find(|t| t.chat_id == chat_id).map(|t| t.id);
                        if let Some(transfer_id) = transfer_id {
                             if let Some(transfer) = self.active_transfers.get_mut(&transfer_id) {
                                if seq == transfer.seq {
                                    tracing::info!("File transfer completed");

                        // Finalize all active transfers
                        let transfer_ids: Vec<Uuid> = self.incoming_files.keys().copied().collect();
                        for transfer_id in transfer_ids {
                            if let Some(incoming) = self.incoming_files.remove(&transfer_id) {
                                let bytes_received = incoming.bytes_received();
                                match incoming.finalize() {
                                    Ok(final_path) => {
                                        if let Some(transfer) =
                                            self.active_transfers.get(&transfer_id)
                                        {
                                            // Add to chat history
                                            if let Some(chat) = self.chats.get_mut(&chat_id) {
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
                                        }
                                        self.update_transfer_progress(transfer_id, bytes_received);
                                    }
                                    Err(e) => {
                                        tracing::error!("Failed to finalize file: {}", e);
                                        self.add_toast(
                                            ToastLevel::Error,
                                            format!("File transfer error: {}", e),
                                        );
                                    }
                                }
                                    }
                                }
                                } else {
                                    tracing::warn!("Received FileEnd with invalid sequence number for transfer {}. Expected {}, got {}. Discarding.", transfer_id, transfer.seq, seq);
                                }
                            }
                        }
                    }

                    ProtocolMessage::Ping { seq } => {
                        if let Some(chat) = self.chats.get_mut(&chat_id) {
                            if seq > chat.recv_seq {
                                chat.recv_seq = seq;
                                tracing::trace!("Received ping with seq {}", seq);
                            } else {
                                tracing::warn!("Received Ping with invalid sequence number for chat {}. Expected > {}, got {}. Discarding.", chat_id, chat.recv_seq, seq);
                            }
                        }
                    }

                    ProtocolMessage::TypingStart { seq } => {
                        if let Some(chat) = self.chats.get_mut(&chat_id) {
                            if seq > chat.recv_seq {
                                chat.recv_seq = seq;
                                chat.peer_typing = true;
                                chat.typing_since = Some(std::time::Instant::now());
                            } else {
                                tracing::warn!("Received TypingStart with invalid sequence number for chat {}. Expected > {}, got {}. Discarding.", chat_id, chat.recv_seq, seq);
                            }
                        }
                    }

                    ProtocolMessage::TypingStop { seq } => {
                        if let Some(chat) = self.chats.get_mut(&chat_id) {
                            if seq > chat.recv_seq {
                                chat.recv_seq = seq;
                                chat.peer_typing = false;
                                chat.typing_since = None;
                            } else {
                                tracing::warn!("Received TypingStop with invalid sequence number for chat {}. Expected > {}, got {}. Discarding.", chat_id, chat.recv_seq, seq);
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
    pub fn parse_invite_link(&self, link: &str) -> Result<Contact> {
        use serde::{Deserialize, Serialize};

        #[derive(Serialize, Deserialize)]
        struct InvitePayload {
            name: String,
            address: Option<String>,
            fingerprint: String,
            public_key: String,
        }

        tracing::debug!("Parsing invite link");
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
        let address = payload.address.and_then(|addr| {
            let trimmed = addr.trim();
            if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("YOUR_IP:PORT") {
                None
            } else {
                // Basic validation: should contain a colon and a numeric port
                if let Some(idx) = trimmed.rfind(':') {
                    let (host, port_str) = trimmed.split_at(idx);
                    let port_str = &port_str[1..]; // skip ':'
                    if host.is_empty() || port_str.parse::<u16>().is_err() {
                        None
                    } else {
                        Some(trimmed.to_string())
                    }
                } else {
                    // no port provided, treat as invalid for now
                    None
                }
            }
        });

        // Create contact
        let contact = Contact {
            id: Uuid::new_v4(),
            name: payload.name,
            address,
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
        assert!(!mgr.has_placeholder_host(port));
        let chat = Chat {
            id: Uuid::new_v4(),
            title: format!("Host on :{}", port),
            peer_fingerprint: None,
            participants: Vec::new(),
            messages: Vec::new(),
            created_at: chrono::Utc::now(),
            peer_typing: false,
            typing_since: None,
            send_seq: 0,
            recv_seq: 0,
        };
        let id = chat.id;
        mgr.chats.insert(id, chat);
        assert!(mgr.has_placeholder_host(port));
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
            peer_fingerprint: None,
            participants: Vec::new(),
            messages: Vec::new(),
            created_at: chrono::Utc::now(),
            peer_typing: false,
            typing_since: None,
            send_seq: 0,
            recv_seq: 0,
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
        mgr.confirm_fingerprint(chat_id, true).expect("confirm should succeed");
        // Now the session should receive confirmation
        assert_eq!(rx.try_recv(), Ok(true));
        // And the fingerprint should now be stored
        assert_eq!(mgr.chats.get(&chat_id).unwrap().peer_fingerprint, Some(fingerprint1.clone()));

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
}
