use anyhow::Result;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::core::{generate_rsa_keypair_async, ProtocolMessage};
use crate::network::{run_client_session, run_host_session};
use crate::transfer::IncomingFileSync;
use crate::types::*;

/// Session handle for communication with network task
pub struct SessionHandle {
    pub from_app_tx: mpsc::UnboundedSender<ProtocolMessage>,
}

/// Main chat manager - orchestrates sessions, messages, and file transfers
pub struct ChatManager {
    pub chats: HashMap<Uuid, Chat>,
    sessions: HashMap<Uuid, SessionHandle>,
    session_events: HashMap<Uuid, mpsc::UnboundedReceiver<SessionEvent>>,
    active_transfers: HashMap<Uuid, FileTransferState>,
    #[allow(dead_code)] // Reserved for future file transfer implementation
    incoming_files: HashMap<Uuid, IncomingFileSync>,
    pub toasts: Vec<Toast>,
    pub config: Config,
}

impl ChatManager {
    pub fn new(config: Config) -> Self {
        Self {
            chats: HashMap::new(),
            sessions: HashMap::new(),
            session_events: HashMap::new(),
            active_transfers: HashMap::new(),
            incoming_files: HashMap::new(),
            toasts: Vec::new(),
            config,
        }
    }

    /// Start hosting on specified port
    pub async fn start_host(&mut self, port: u16) -> Result<Uuid> {
        let chat_id = Uuid::new_v4();
        let privkey = generate_rsa_keypair_async(2048).await?;

        // Create channels
        let (to_app_tx, to_app_rx) = mpsc::unbounded_channel();
        let (from_app_tx, from_app_rx) = mpsc::unbounded_channel();

        // Spawn session task
        tokio::spawn(async move {
            if let Err(e) = run_host_session(port, privkey, to_app_tx, from_app_rx).await {
                tracing::error!("Host session error: {}", e);
            }
        });

        // Create chat entry
        let chat = Chat {
            id: chat_id,
            title: format!("Host on :{}", port),
            peer_fingerprint: None,
            messages: Vec::new(),
            created_at: chrono::Utc::now(),
            is_connected: false,
        };

        self.chats.insert(chat_id, chat);
        self.sessions.insert(chat_id, SessionHandle { from_app_tx });
        self.session_events.insert(chat_id, to_app_rx);

        self.add_toast(
            ToastLevel::Info,
            format!("Listening on port {}", port),
        );

        Ok(chat_id)
    }

    /// Connect to a host
    pub async fn connect_to_host(&mut self, host: &str, port: u16) -> Result<Uuid> {
        let chat_id = Uuid::new_v4();
        let privkey = generate_rsa_keypair_async(2048).await?;

        let (to_app_tx, to_app_rx) = mpsc::unbounded_channel();
        let (from_app_tx, from_app_rx) = mpsc::unbounded_channel();

        let host_copy = host.to_string();
        tokio::spawn(async move {
            if let Err(e) =
                run_client_session(&host_copy, port, privkey, to_app_tx, from_app_rx).await
            {
                tracing::error!("Client session error: {}", e);
            }
        });

        let chat = Chat {
            id: chat_id,
            title: format!("{}:{}", host, port),
            peer_fingerprint: None,
            messages: Vec::new(),
            created_at: chrono::Utc::now(),
            is_connected: false,
        };

        self.chats.insert(chat_id, chat);
        self.sessions.insert(chat_id, SessionHandle { from_app_tx });
        self.session_events.insert(chat_id, to_app_rx);

        self.add_toast(
            ToastLevel::Info,
            format!("Connecting to {}:{}", host, port),
        );

        Ok(chat_id)
    }

    /// Send a text message
    pub fn send_message(&mut self, chat_id: Uuid, text: String) -> Result<()> {
        let session = self
            .sessions
            .get(&chat_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found"))?;

        let msg = ProtocolMessage::Text {
            text: text.clone(),
            timestamp: crate::util::current_timestamp_millis(),
        };

        session.from_app_tx.send(msg)?;

        // Add to local history
        if let Some(chat) = self.chats.get_mut(&chat_id) {
            chat.messages.push(Message {
                id: Uuid::new_v4(),
                from_me: true,
                content: MessageContent::Text { text },
                timestamp: chrono::Utc::now(),
            });
        }

        Ok(())
    }

    /// Start receiving a file
    pub fn start_receiving_file(
        &mut self,
        _chat_id: Uuid,
        filename: &str,
        size: u64,
    ) -> Result<Uuid> {
        let transfer_id = Uuid::new_v4();

        let state = FileTransferState {
            id: transfer_id,
            filename: filename.to_string(),
            size,
            received: 0,
            status: TransferStatus::Pending,
        };

        self.active_transfers.insert(transfer_id, state);

        self.add_toast(
            ToastLevel::Info,
            format!("Receiving file: {}", filename),
        );

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
            self.add_toast(
                ToastLevel::Success,
                format!("File received: {}", filename),
            );
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
        self.toasts.retain(|toast| {
            now.duration_since(toast.created_at) < toast.duration
        });
    }

    /// Get a chat by ID
    pub fn get_chat(&self, chat_id: Uuid) -> Option<&Chat> {
        self.chats.get(&chat_id)
    }

    /// Get a mutable chat by ID
    pub fn get_chat_mut(&mut self, chat_id: Uuid) -> Option<&mut Chat> {
        self.chats.get_mut(&chat_id)
    }

    /// Get all chat IDs
    pub fn chat_ids(&self) -> Vec<Uuid> {
        self.chats.keys().copied().collect()
    }

    /// Delete a chat and its associated session
    pub fn delete_chat(&mut self, chat_id: Uuid) {
        self.chats.remove(&chat_id);
        self.sessions.remove(&chat_id);
        self.session_events.remove(&chat_id);
        self.add_toast(ToastLevel::Info, "Chat deleted".to_string());
    }

    /// Send a file to a chat
    pub async fn send_file(&mut self, chat_id: Uuid, path: std::path::PathBuf) -> Result<()> {
        use tokio::fs::File;
        use tokio::io::AsyncReadExt;

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

        // Send file metadata
        let meta_msg = ProtocolMessage::FileMeta {
            filename: filename.clone(),
            size: file_size,
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
        }

        // Send end marker
        session.from_app_tx.send(ProtocolMessage::FileEnd)?;

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

        self.add_toast(
            ToastLevel::Success,
            format!("File sent: {}", filename),
        );

        Ok(())
    }

    /// Poll and process all pending session events
    pub fn poll_session_events(&mut self) {
        let chat_ids: Vec<Uuid> = self.session_events.keys().copied().collect();

        for chat_id in chat_ids {
            // Collect all pending events for this session
            let mut events = Vec::new();
            if let Some(rx) = self.session_events.get_mut(&chat_id) {
                while let Ok(event) = rx.try_recv() {
                    events.push(event);
                }
            }

            // Process collected events
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

            SessionEvent::FingerprintReceived { fingerprint } => {
                tracing::info!("Session {} received fingerprint: {}", chat_id, &fingerprint[..16]);

                if let Some(chat) = self.chats.get_mut(&chat_id) {
                    chat.peer_fingerprint = Some(fingerprint.clone());
                }

                self.add_toast(
                    ToastLevel::Warning,
                    format!("Verify fingerprint: {}...", &fingerprint[..16])
                );
            }

            SessionEvent::Ready => {
                tracing::info!("Session {} is ready", chat_id);
                if let Some(chat) = self.chats.get_mut(&chat_id) {
                    chat.is_connected = true;
                }
                self.add_toast(ToastLevel::Success, "Connection established!".to_string());
            }

            SessionEvent::MessageReceived(proto_msg) => {
                tracing::debug!("Session {} received message: {:?}", chat_id, proto_msg);

                match proto_msg {
                    ProtocolMessage::Text { text, .. } => {
                        if let Some(chat) = self.chats.get_mut(&chat_id) {
                            chat.messages.push(Message {
                                id: Uuid::new_v4(),
                                from_me: false,
                                content: MessageContent::Text { text: text.clone() },
                                timestamp: chrono::Utc::now(),
                            });

                            tracing::info!("Added received message to chat {}", chat_id);
                        } else {
                            tracing::error!("Chat {} not found for received message", chat_id);
                        }
                    }

                    ProtocolMessage::FileMeta { filename, size } => {
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
                                            format!("Failed to receive file: {}", e)
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::error!("Failed to start receiving file: {}", e);
                                self.add_toast(
                                    ToastLevel::Error,
                                    format!("Failed to receive file: {}", e)
                                );
                            }
                        }
                    }

                    ProtocolMessage::FileChunk { chunk, seq } => {
                        tracing::debug!("Received file chunk {} ({} bytes)", seq, chunk.len());
                        
                        // Find the active transfer for this chat
                        let transfer_ids: Vec<Uuid> = self.active_transfers.keys().copied().collect();
                        for transfer_id in transfer_ids {
                            let result = if let Some(incoming) = self.incoming_files.get_mut(&transfer_id) {
                                let write_result = incoming.write_chunk(&chunk);
                                let bytes_received = incoming.bytes_received();
                                Some((write_result, bytes_received))
                            } else {
                                None
                            };
                            
                            if let Some((write_result, bytes_received)) = result {
                                if let Err(e) = write_result {
                                    tracing::error!("Failed to write chunk: {}", e);
                                    self.add_toast(
                                        ToastLevel::Error,
                                        format!("File transfer error: {}", e)
                                    );
                                } else {
                                    self.update_transfer_progress(transfer_id, bytes_received);
                                }
                                break;
                            }
                        }
                    }

                    ProtocolMessage::FileEnd => {
                        tracing::info!("File transfer completed");
                        
                        // Finalize all active transfers
                        let transfer_ids: Vec<Uuid> = self.incoming_files.keys().copied().collect();
                        for transfer_id in transfer_ids {
                            if let Some(incoming) = self.incoming_files.remove(&transfer_id) {
                                let bytes_received = incoming.bytes_received();
                                match incoming.finalize() {
                                    Ok(final_path) => {
                                        if let Some(transfer) = self.active_transfers.get(&transfer_id) {
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
                                            format!("File transfer error: {}", e)
                                        );
                                    }
                                }
                            }
                        }
                    }

                    ProtocolMessage::Ping => {
                        tracing::trace!("Received ping");
                    }

                    ProtocolMessage::Version { .. } | ProtocolMessage::EphemeralKey { .. } => {
                        // These are handshake messages, should not appear in message loop
                        tracing::warn!("Received handshake message in message loop: {:?}", proto_msg);
                    }
                }
            }

            SessionEvent::Disconnected => {
                tracing::warn!("Session {} disconnected", chat_id);
                if let Some(chat) = self.chats.get_mut(&chat_id) {
                    chat.is_connected = false;
                }
                self.add_toast(ToastLevel::Warning, "Connection lost".to_string());

                // Clean up session
                self.sessions.remove(&chat_id);
                self.session_events.remove(&chat_id);
            }

            SessionEvent::Error(err) => {
                tracing::error!("Session {} error: {}", chat_id, err);
                self.add_toast(ToastLevel::Error, format!("Connection error: {}", err));
            }
        }
    }
}

impl Default for ChatManager {
    fn default() -> Self {
        Self::new(Config::default())
    }
}
