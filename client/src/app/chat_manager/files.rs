//! Outgoing and incoming file transfers: validation, chunked sending, and
//! wire-level delivery confirmation (see SessionEvent::FileSendComplete).

use super::*;

impl ChatManager {
    /// Send a file to a chat
    pub async fn send_file(&mut self, chat_id: Uuid, path: std::path::PathBuf) -> Result<()> {
        use tokio::fs::File;
        use tokio::io::AsyncReadExt;

        tracing::info!(chat_id = %chat_id, path = %path.display().to_string(), "Preparing to send file");
        let session_id = *self.chat_id_mapping.get(&chat_id).unwrap_or(&chat_id);
        let sender = self
            .sessions
            .get(&session_id)
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
        tracing::info!(file = %filename, total_bytes = %file_size, "File queued for sending");

        // Add to local history.
        let message_id = Uuid::new_v4();
        chat.messages.push(Message {
            id: message_id,
            from_me: true,
            content: MessageContent::File {
                filename: filename.clone(),
                size: file_size,
                path: Some(path),
            },
            timestamp: chrono::Utc::now(),
            delivered: false,
        });

        // Queueing is not delivery: the success toast waits for the session to
        // report the final frame on the wire (SessionEvent::FileSendComplete).
        // File frames drain FIFO per session, so a per-session queue correlates;
        // keyed by session id because that is where the event will arrive. The
        // chat/message ids let the completion register for a delivery receipt.
        self.pending_file_sends
            .entry(session_id)
            .or_default()
            .push_back((filename, chat_id, message_id));

        Ok(())
    }

    /// Fail all not-yet-confirmed outgoing file sends for a chat (session died
    /// before their final frame was written) with an honest error toast.
    pub(super) fn fail_pending_file_sends(&mut self, chat_id: Uuid, reason: &str) {
        let Some(pending) = self.pending_file_sends.remove(&chat_id) else {
            return;
        };
        for (filename, _chat, _message) in pending {
            tracing::warn!(file = %filename, reason = %reason, "Outgoing file send interrupted");
            self.add_toast(
                ToastLevel::Error,
                format!(
                    "File may not have been delivered ({}): {}",
                    reason, filename
                ),
            );
        }
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
        // Honor the auto-accept setting: when off, the transfer is held in
        // AwaitingAcceptance until the user decides. Chunks still spool to the
        // temp file (the sender streams without flow control), but nothing
        // lands in the download directory or the chat history until accepted,
        // and declining deletes the spool immediately.
        let auto_accept = self.config.auto_accept_files;

        let state = FileTransferState {
            id: transfer_id,
            chat_id,
            filename: filename.to_string(),
            size,
            received: 0,
            status: if auto_accept {
                TransferStatus::Pending
            } else {
                TransferStatus::AwaitingAcceptance
            },
            seq: 0,
        };

        self.active_transfers.insert(transfer_id, state);

        if auto_accept {
            self.add_toast(ToastLevel::Info, format!("Receiving file: {}", filename));
        } else {
            self.add_toast(
                ToastLevel::Info,
                format!(
                    "Incoming file: {} ({}) — accept or decline it in the conversation",
                    filename,
                    crate::util::format_size(size)
                ),
            );
        }

        Ok(transfer_id)
    }

    /// Accept an incoming transfer that is awaiting the user's decision. If the
    /// sender already finished streaming, the held file is finalized right away;
    /// otherwise the transfer simply continues as a normal in-progress one.
    pub fn accept_incoming_file(&mut self, transfer_id: Uuid) -> Result<()> {
        let status = self
            .active_transfers
            .get(&transfer_id)
            .map(|t| t.status.clone())
            .ok_or_else(|| anyhow!("No such transfer"))?;
        if status != TransferStatus::AwaitingAcceptance {
            bail!("Transfer is not awaiting acceptance");
        }
        if let Some(end_seq) = self.pending_file_end.remove(&transfer_id) {
            self.finalize_incoming_file(transfer_id, Some(end_seq));
        } else if let Some(transfer) = self.active_transfers.get_mut(&transfer_id) {
            transfer.status = if transfer.received > 0 {
                TransferStatus::InProgress
            } else {
                TransferStatus::Pending
            };
        }
        Ok(())
    }

    /// Decline an incoming transfer that is awaiting the user's decision: the
    /// spooled temp file is deleted and any further chunks for it are discarded.
    pub fn reject_incoming_file(&mut self, transfer_id: Uuid) -> Result<()> {
        let transfer = self
            .active_transfers
            .get_mut(&transfer_id)
            .ok_or_else(|| anyhow!("No such transfer"))?;
        if transfer.status != TransferStatus::AwaitingAcceptance {
            bail!("Transfer is not awaiting acceptance");
        }
        transfer.status = TransferStatus::Cancelled;
        let filename = transfer.filename.clone();
        self.pending_file_end.remove(&transfer_id);
        if let Some(incoming) = self.incoming_files.remove(&transfer_id) {
            if let Err(e) = incoming.abort_cleanup() {
                tracing::warn!(
                    "Failed to clean up declined transfer {}: {}",
                    transfer_id,
                    e
                );
            }
        }
        self.add_toast(
            ToastLevel::Info,
            format!("Declined incoming file: {}", filename),
        );
        Ok(())
    }

    /// Move a fully received file into the download directory, record it in the
    /// chat history, and drop the transfer bookkeeping. Shared by the `FileEnd`
    /// handler (auto-accepted transfers) and `accept_incoming_file` (held ones).
    /// `ack_seq` is the FileEnd's wire seq: on success a delivery receipt for
    /// it is sent back to the peer.
    pub(super) fn finalize_incoming_file(&mut self, transfer_id: Uuid, ack_seq: Option<u64>) {
        let Some(incoming) = self.incoming_files.remove(&transfer_id) else {
            return;
        };
        let bytes_received = incoming.bytes_received();
        match incoming.finalize() {
            Ok(final_path) => {
                if let Some(mut transfer) = self.active_transfers.remove(&transfer_id) {
                    transfer.status = TransferStatus::Completed;
                    if let Some(chat) = self.chats.get_mut(&transfer.chat_id) {
                        chat.messages.push(Message {
                            id: Uuid::new_v4(),
                            from_me: false,
                            content: MessageContent::File {
                                filename: transfer.filename.clone(),
                                size: transfer.size,
                                path: Some(final_path),
                            },
                            timestamp: chrono::Utc::now(),
                            delivered: false,
                        });
                    }
                    // Delivery receipt: the file is on disk, tell the sender.
                    if let Some(acked_seq) = ack_seq {
                        self.send_ack_for_chat(transfer.chat_id, acked_seq);
                    }
                    self.add_toast(
                        ToastLevel::Success,
                        format!("File received: {}", transfer.filename),
                    );
                }
                self.update_transfer_progress(transfer_id, bytes_received);
            }
            Err(e) => {
                if let Some(transfer) = self.active_transfers.get_mut(&transfer_id) {
                    transfer.status = TransferStatus::Failed(e.to_string());
                }
                tracing::error!("Failed to finalize file: {}", e);
                self.add_toast(ToastLevel::Error, format!("File transfer error: {}", e));
                self.active_transfers.remove(&transfer_id);
            }
        }
    }

    /// Update file transfer progress
    pub fn update_transfer_progress(&mut self, transfer_id: Uuid, bytes: u64) {
        if let Some(transfer) = self.active_transfers.get_mut(&transfer_id) {
            transfer.received = bytes;
            // Only promote Pending: a transfer awaiting user acceptance keeps
            // its status even while chunks spool in the background.
            if bytes > 0 && transfer.status == TransferStatus::Pending {
                transfer.status = TransferStatus::InProgress;
            }
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

    pub(super) fn active_transfer_id_for_chat(&self, chat_id: Uuid) -> Option<Uuid> {
        self.transfer_ids_for_chat_with_status(chat_id, |status| {
            matches!(
                status,
                TransferStatus::Pending
                    | TransferStatus::AwaitingAcceptance
                    | TransferStatus::InProgress
            )
        })
        .into_iter()
        .next()
    }

    fn clear_transfer_state(&mut self, transfer_id: Uuid) {
        self.active_transfers.remove(&transfer_id);
        self.incoming_files.remove(&transfer_id);
    }
}
