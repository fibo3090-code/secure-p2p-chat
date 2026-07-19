//! Outgoing and incoming file transfers: validation, chunked streaming (in a
//! background task so a large send never holds the manager lock), cancellation,
//! and wire-level delivery confirmation (see SessionEvent::FileSendComplete).

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

impl ChatManager {
    /// Send a file to a chat.
    ///
    /// The transfer is registered and its chunks are streamed from a spawned
    /// task, so this returns as soon as the metadata frame is queued — a
    /// multi-gigabyte send neither blocks the caller nor holds the manager
    /// lock, and it can be cancelled mid-flight via [`cancel_transfer`].
    pub async fn send_file(&mut self, chat_id: Uuid, path: std::path::PathBuf) -> Result<()> {
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

        // Send file metadata. The session loop stamps the real monotonic wire
        // sequence onto every frame it writes, so the seq passed here is a
        // placeholder (kept only for local bookkeeping continuity).
        chat.send_seq += 1;
        sender.send(ProtocolMessage::FileMeta {
            filename: filename.clone(),
            size: file_size,
            seq: chat.send_seq,
        })?;

        // Add to local history.
        chat.messages.push(Message {
            id: Uuid::new_v4(),
            from_me: true,
            content: MessageContent::File {
                filename: filename.clone(),
                size: file_size,
                path: Some(path.clone()),
            },
            timestamp: chrono::Utc::now(),
        });

        // Track the transfer so the UI can show progress and cancel it.
        let transfer_id = Uuid::new_v4();
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicU64::new(0));
        self.active_transfers.insert(
            transfer_id,
            FileTransferState {
                id: transfer_id,
                chat_id,
                filename: filename.clone(),
                size: file_size,
                received: 0,
                status: TransferStatus::Pending,
                seq: 0,
                direction: TransferDirection::Outgoing,
            },
        );
        self.outgoing_transfers.insert(
            transfer_id,
            OutgoingTransfer {
                session_id,
                cancel: cancel.clone(),
                progress: progress.clone(),
            },
        );

        // Queueing is not delivery: the success toast waits for the session to
        // report the final frame on the wire (SessionEvent::FileSendComplete).
        // File frames drain FIFO per session, so a per-session queue correlates;
        // keyed by session id because that is where the event will arrive.
        self.pending_file_sends
            .entry(session_id)
            .or_default()
            .push_back((filename.clone(), transfer_id));

        // Stream the chunks off-thread so the send neither blocks the manager
        // lock nor buffers the whole file eagerly, and can be cancelled between
        // chunks. A cancel emits `FileCancel` and stops without a `FileEnd`, so
        // no `FileSendComplete` fires for a cancelled send.
        Self::spawn_file_stream(sender, path, filename, file_size, cancel, progress);
        Ok(())
    }

    /// Background chunk streamer for one outgoing file. Runs until EOF (then
    /// emits `FileEnd`), cancellation (then emits `FileCancel`), or a dead
    /// channel (peer gone — stop silently; the session teardown reports it).
    fn spawn_file_stream(
        sender: mpsc::UnboundedSender<ProtocolMessage>,
        path: std::path::PathBuf,
        filename: String,
        file_size: u64,
        cancel: Arc<AtomicBool>,
        progress: Arc<AtomicU64>,
    ) {
        use tokio::io::AsyncReadExt;
        tokio::spawn(async move {
            let mut file = match tokio::fs::File::open(&path).await {
                Ok(f) => f,
                Err(e) => {
                    tracing::error!(file = %filename, error = %e, "Could not open file to stream");
                    let _ = sender.send(ProtocolMessage::FileCancel { seq: 0 });
                    return;
                }
            };
            let mut buffer = vec![0u8; crate::FILE_CHUNK_SIZE];
            let mut sent_chunks = 0u64;
            loop {
                if cancel.load(Ordering::Relaxed) {
                    tracing::info!(file = %filename, "Outgoing transfer cancelled; sending FileCancel");
                    let _ = sender.send(ProtocolMessage::FileCancel { seq: 0 });
                    return;
                }
                let n = match file.read(&mut buffer).await {
                    Ok(0) => break, // EOF
                    Ok(n) => n,
                    Err(e) => {
                        tracing::error!(file = %filename, error = %e, "Read error while streaming file");
                        let _ = sender.send(ProtocolMessage::FileCancel { seq: 0 });
                        return;
                    }
                };
                if sender
                    .send(ProtocolMessage::FileChunk {
                        chunk: buffer[..n].to_vec(),
                        seq: 0,
                    })
                    .is_err()
                {
                    return; // peer/session gone
                }
                progress.fetch_add(n as u64, Ordering::Relaxed);
                sent_chunks += 1;
                if sent_chunks % 64 == 0 {
                    tracing::trace!(sent_chunks = %sent_chunks, "File sending progress");
                }
            }
            let _ = sender.send(ProtocolMessage::FileEnd { seq: 0 });
            tracing::info!(file = %filename, total_bytes = %file_size, "File fully queued for sending");
        });
    }

    /// Cancel an in-flight transfer (either direction). Marks it `Cancelled`,
    /// notifies the peer with `FileCancel`, and cleans up local state (the
    /// partial temp file for an incoming transfer; the streaming task for an
    /// outgoing one). A no-op for an unknown or already-finished transfer.
    pub fn cancel_transfer(&mut self, transfer_id: Uuid) {
        let Some(state) = self.active_transfers.get(&transfer_id) else {
            return;
        };
        if matches!(
            state.status,
            TransferStatus::Completed | TransferStatus::Failed(_) | TransferStatus::Cancelled
        ) {
            return;
        }
        let chat_id = state.chat_id;
        let filename = state.filename.clone();
        let direction = state.direction;

        match direction {
            TransferDirection::Outgoing => {
                // Stop the streaming task; it emits FileCancel itself. Drop the
                // pending-delivery entry so no "not delivered" toast fires and
                // no FileSendComplete is expected.
                if let Some(handle) = self.outgoing_transfers.remove(&transfer_id) {
                    handle.cancel.store(true, Ordering::Relaxed);
                    if let Some(queue) = self.pending_file_sends.get_mut(&handle.session_id) {
                        queue.retain(|(_, id)| *id != transfer_id);
                    }
                }
            }
            TransferDirection::Incoming => {
                // Tell the sender to stop, then discard the partial file.
                let session_id = *self.chat_id_mapping.get(&chat_id).unwrap_or(&chat_id);
                if let Some(session) = self.sessions.get(&session_id) {
                    let _ = session
                        .from_app_tx
                        .send(ProtocolMessage::FileCancel { seq: 0 });
                }
                if let Some(incoming) = self.incoming_files.remove(&transfer_id) {
                    if let Err(e) = incoming.abort_cleanup() {
                        tracing::warn!(%transfer_id, error = %e, "Failed to clean up cancelled transfer");
                    }
                }
            }
        }

        if let Some(state) = self.active_transfers.get_mut(&transfer_id) {
            state.status = TransferStatus::Cancelled;
        }
        self.add_toast(
            ToastLevel::Warning,
            format!("File transfer cancelled: {}", filename),
        );
    }

    /// Mirror background outgoing-stream progress into the tracked transfer
    /// state so the UI's snapshot reflects live send progress. Called each
    /// poll tick before events are drained.
    pub(super) fn sync_outgoing_transfer_progress(&mut self) {
        for (transfer_id, handle) in &self.outgoing_transfers {
            if let Some(state) = self.active_transfers.get_mut(transfer_id) {
                let sent = handle.progress.load(Ordering::Relaxed);
                state.received = sent;
                if sent > 0 && state.status == TransferStatus::Pending {
                    state.status = TransferStatus::InProgress;
                }
            }
        }
    }

    /// Handle a `FileCancel` frame from the peer: abort whichever transfer is
    /// active on this chat (an incoming receive, or a send the peer refused).
    pub(super) fn handle_peer_file_cancel(&mut self, chat_id: Uuid) {
        // Prefer an active incoming transfer (the common case: peer is the
        // sender and we are receiving); otherwise cancel our outgoing send.
        let target = self
            .active_incoming_transfer_id_for_chat(chat_id)
            .or_else(|| self.active_outgoing_transfer_id_for_chat(chat_id));
        let Some(transfer_id) = target else {
            tracing::debug!(%chat_id, "FileCancel received with no active transfer");
            return;
        };

        if let Some(handle) = self.outgoing_transfers.remove(&transfer_id) {
            handle.cancel.store(true, Ordering::Relaxed);
            if let Some(queue) = self.pending_file_sends.get_mut(&handle.session_id) {
                queue.retain(|(_, id)| *id != transfer_id);
            }
        }
        if let Some(incoming) = self.incoming_files.remove(&transfer_id) {
            let _ = incoming.abort_cleanup();
        }
        let filename = self
            .active_transfers
            .get_mut(&transfer_id)
            .map(|s| {
                s.status = TransferStatus::Cancelled;
                s.filename.clone()
            })
            .unwrap_or_default();
        self.add_toast(
            ToastLevel::Warning,
            format!("Peer cancelled the file transfer: {}", filename),
        );
    }

    /// Fail all not-yet-confirmed outgoing file sends for a chat (session died
    /// before their final frame was written) with an honest error toast.
    pub(super) fn fail_pending_file_sends(&mut self, chat_id: Uuid, reason: &str) {
        let Some(pending) = self.pending_file_sends.remove(&chat_id) else {
            return;
        };
        for (filename, transfer_id) in pending {
            tracing::warn!(file = %filename, reason = %reason, "Outgoing file send interrupted");
            // Stop the streaming task and mark the transfer failed so its UI row
            // reflects the interruption instead of an eternal "in progress".
            if let Some(handle) = self.outgoing_transfers.remove(&transfer_id) {
                handle.cancel.store(true, Ordering::Relaxed);
            }
            if let Some(state) = self.active_transfers.get_mut(&transfer_id) {
                state.status = TransferStatus::Failed(reason.to_string());
            }
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

        if self.active_incoming_transfer_id_for_chat(chat_id).is_some() {
            bail!("Another incoming file transfer is already in progress for this chat");
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
            direction: TransferDirection::Incoming,
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

    /// The active (pending/in-progress) transfer for a chat in a given
    /// direction, if any. Incoming and outgoing are tracked separately so a
    /// send and a receive can run on the same chat without misrouting.
    fn active_transfer_id_for_chat_dir(
        &self,
        chat_id: Uuid,
        direction: TransferDirection,
    ) -> Option<Uuid> {
        self.active_transfers
            .iter()
            .find(|(_, t)| {
                t.chat_id == chat_id
                    && t.direction == direction
                    && matches!(
                        t.status,
                        TransferStatus::Pending | TransferStatus::InProgress
                    )
            })
            .map(|(id, _)| *id)
    }

    pub(super) fn active_incoming_transfer_id_for_chat(&self, chat_id: Uuid) -> Option<Uuid> {
        self.active_transfer_id_for_chat_dir(chat_id, TransferDirection::Incoming)
    }

    pub(super) fn active_outgoing_transfer_id_for_chat(&self, chat_id: Uuid) -> Option<Uuid> {
        self.active_transfer_id_for_chat_dir(chat_id, TransferDirection::Outgoing)
    }

    fn clear_transfer_state(&mut self, transfer_id: Uuid) {
        self.active_transfers.remove(&transfer_id);
        self.incoming_files.remove(&transfer_id);
        self.outgoing_transfers.remove(&transfer_id);
    }
}
