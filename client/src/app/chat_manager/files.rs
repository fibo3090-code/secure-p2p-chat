//! Outgoing and incoming file transfers: validation, chunked streaming (in a
//! background task, so a large send never holds the manager lock),
//! cancellation, and wire-level send confirmation.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// How much *unaccepted* incoming data one conversation may spool, across every
/// offer awaiting the user's decision, before the offer crossing it is declined.
/// The sender streams without flow control, so without it a peer could fill the
/// disk (up to `MAX_FILE_SIZE`) of someone who agreed to receive nothing.
pub const MAX_UNACCEPTED_SPOOL_BYTES: u64 = 64 * 1024 * 1024; // 64 MiB

/// Arguments for [`ChatManager::spawn_file_stream`].
struct SpawnFileStream {
    /// Bounded lane for `FileChunk`/`FileEnd` (backpressure via `send().await`).
    file_tx: mpsc::Sender<ProtocolMessage>,
    /// Unbounded control lane, so `FileCancel` never waits behind chunks.
    control_tx: mpsc::UnboundedSender<ProtocolMessage>,
    path: std::path::PathBuf,
    filename: String,
    file_size: u64,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicU64>,
    failed: Arc<AtomicBool>,
}

impl ChatManager {
    /// Send a file to a chat. Chunks stream from a spawned task, so this returns
    /// once `FileMeta` is queued; cancel with [`Self::cancel_transfer`].
    pub async fn send_file(&mut self, chat_id: Uuid, path: std::path::PathBuf) -> Result<()> {
        tracing::info!(chat_id = %chat_id, path = %path.display().to_string(), "Preparing to send file");
        // One outgoing transfer per conversation: `FileChunk` carries no transfer
        // id, so concurrent streams would interleave and corrupt both files.
        if let Some(existing) = self.active_outgoing_transfer_id_for_chat(chat_id) {
            let in_flight = self
                .active_transfers
                .get(&existing)
                .map(|t| t.filename.clone())
                .unwrap_or_else(|| "another file".to_string());
            bail!(
                "Still sending {} in this conversation. Wait for it to finish, or cancel it first.",
                in_flight
            );
        }
        let session_id = self.session_for_chat(chat_id);
        let (control_tx, file_tx) = self
            .sessions
            .get(&session_id)
            .map(|s| (s.from_app_tx.clone(), s.file_tx.clone()))
            .ok_or_else(|| anyhow!("Session not found"))?;

        let (filename, file_size) = Self::validate_outgoing_file(&path).await?;
        tracing::debug!(file = %filename, size = %file_size, "Sending file metadata");

        // Placeholder seq (the session loop stamps the wire seq). FileMeta rides
        // the file lane so it stays ahead of the chunks.
        let chat = self
            .chats
            .get_mut(&chat_id)
            .ok_or_else(|| anyhow::anyhow!("Chat not found for sending file"))?;
        chat.send_seq += 1;
        file_tx
            .send(ProtocolMessage::FileMeta {
                filename: filename.clone(),
                size: file_size,
                seq: 0,
            })
            .await
            .map_err(|_| anyhow!("Session closed before file metadata was queued"))?;
        let message_id = Uuid::new_v4();
        chat.messages.push(Message::sent(
            message_id,
            MessageContent::File {
                filename: filename.clone(),
                size: file_size,
                path: Some(path.clone()),
            },
        ));

        let transfer_id = Uuid::new_v4();
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicU64::new(0));
        let failed = Arc::new(AtomicBool::new(false));
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
                failed: failed.clone(),
            },
        );

        // Queueing is not delivery: the toast and receipt wait for
        // `FileSendComplete`, which resolves this FIFO per session.
        self.pending_file_sends
            .entry(session_id)
            .or_default()
            .push_back(PendingFileSend {
                filename: filename.clone(),
                transfer_id,
                chat_id,
                message_id,
            });

        // A cancel or local I/O error stops without `FileEnd`, so no
        // `FileSendComplete` fires for it.
        Self::spawn_file_stream(SpawnFileStream {
            file_tx,
            control_tx,
            path,
            filename,
            file_size,
            cancel,
            progress,
            failed,
        });
        Ok(())
    }

    /// Background chunk streamer for one outgoing file. Ends at EOF (`FileEnd`),
    /// on cancel (`FileCancel`), on a local I/O error (sets `failed`, then
    /// `FileCancel`), or silently when the session is gone. The bounded lane
    /// paces the reader to the network.
    fn spawn_file_stream(args: SpawnFileStream) {
        let SpawnFileStream {
            file_tx,
            control_tx,
            path,
            filename,
            file_size,
            cancel,
            progress,
            failed,
        } = args;
        use tokio::io::AsyncReadExt;
        tokio::spawn(async move {
            let mut file = match tokio::fs::File::open(&path).await {
                Ok(f) => f,
                Err(e) => {
                    tracing::error!(file = %filename, error = %e, "Could not open file to stream");
                    failed.store(true, Ordering::Relaxed);
                    let _ = control_tx.send(ProtocolMessage::FileCancel { seq: 0 });
                    return;
                }
            };
            let mut buffer = vec![0u8; crate::FILE_CHUNK_SIZE];
            let mut sent_chunks = 0u64;
            loop {
                if cancel.load(Ordering::Relaxed) {
                    tracing::info!(file = %filename, "Outgoing transfer cancelled; sending FileCancel");
                    let _ = control_tx.send(ProtocolMessage::FileCancel { seq: 0 });
                    return;
                }
                let n = match file.read(&mut buffer).await {
                    Ok(0) => break, // EOF
                    Ok(n) => n,
                    Err(e) => {
                        tracing::error!(file = %filename, error = %e, "Read error while streaming file");
                        failed.store(true, Ordering::Relaxed);
                        let _ = control_tx.send(ProtocolMessage::FileCancel { seq: 0 });
                        return;
                    }
                };
                if file_tx
                    .send(ProtocolMessage::FileChunk {
                        chunk: buffer[..n].to_vec(),
                        seq: 0,
                    })
                    .await
                    .is_err()
                {
                    return; // peer/session gone
                }
                progress.fetch_add(n as u64, Ordering::Relaxed);
                sent_chunks += 1;
                if sent_chunks.is_multiple_of(64) {
                    tracing::trace!(sent_chunks = %sent_chunks, "File sending progress");
                }
            }
            let _ = file_tx.send(ProtocolMessage::FileEnd { seq: 0 }).await;
            tracing::info!(file = %filename, total_bytes = %file_size, "File fully queued for sending");
        });
    }

    /// Cancel an in-flight transfer (either direction): mark it `Cancelled`, tell
    /// the peer, and clean up locally. A no-op for unknown/finished transfers.
    pub fn cancel_transfer(&mut self, transfer_id: Uuid) {
        let Some(state) = self.active_transfers.get(&transfer_id) else {
            return;
        };
        if !state.status.is_active() {
            return;
        }
        let chat_id = state.chat_id;

        match state.direction {
            // The streaming task emits FileCancel itself once it sees the flag.
            TransferDirection::Outgoing => self.stop_outgoing_stream(transfer_id),
            // Tell the sender to stop, then discard the partial file.
            TransferDirection::Incoming => {
                self.send_control(chat_id, ProtocolMessage::FileCancel { seq: 0 });
                self.discard_spool(transfer_id, "cancelled");
            }
        }

        let filename = self.set_transfer_status(transfer_id, TransferStatus::Cancelled);
        self.add_toast(
            ToastLevel::Warning,
            format!("File transfer cancelled: {}", filename),
        );
    }

    /// Mirror background send progress (and local read failures) into the
    /// tracked transfer state. Called each poll tick.
    pub(super) fn sync_outgoing_transfer_progress(&mut self) {
        let mut failed_ids = Vec::new();
        for (transfer_id, handle) in &self.outgoing_transfers {
            if handle.failed.load(Ordering::Relaxed) {
                failed_ids.push(*transfer_id);
                continue;
            }
            if let Some(state) = self.active_transfers.get_mut(transfer_id) {
                let sent = handle.progress.load(Ordering::Relaxed);
                state.received = sent;
                if sent > 0 && state.status == TransferStatus::Pending {
                    state.status = TransferStatus::InProgress;
                }
            }
        }

        for transfer_id in failed_ids {
            self.stop_outgoing_stream(transfer_id);
            let filename = self.set_transfer_status(
                transfer_id,
                TransferStatus::Failed("could not read the file".to_string()),
            );
            self.add_toast(
                ToastLevel::Error,
                format!("File send failed (could not read the file): {}", filename),
            );
        }
    }

    /// A `FileCancel` from the peer: abort the incoming stream on this chat, or
    /// else our outgoing send they refused.
    pub(super) fn handle_peer_file_cancel(&mut self, chat_id: Uuid) {
        let target = self
            .incoming_streams
            .remove(&chat_id)
            .filter(|id| self.is_active_transfer(*id))
            .or_else(|| self.active_outgoing_transfer_id_for_chat(chat_id));
        let Some(transfer_id) = target else {
            tracing::debug!(%chat_id, "FileCancel received with no active transfer");
            return;
        };

        self.stop_outgoing_stream(transfer_id);
        self.discard_spool(transfer_id, "cancelled by the peer");
        let filename = self.set_transfer_status(transfer_id, TransferStatus::Cancelled);
        self.add_toast(
            ToastLevel::Warning,
            format!("Peer cancelled the file transfer: {}", filename),
        );
    }

    /// Fail not-yet-confirmed outgoing sends for a dead session, honestly.
    pub(super) fn fail_pending_file_sends(&mut self, chat_id: Uuid, reason: &str) {
        let Some(pending) = self.pending_file_sends.remove(&chat_id) else {
            return;
        };
        for PendingFileSend {
            filename,
            transfer_id,
            ..
        } in pending
        {
            tracing::warn!(file = %filename, reason = %reason, "Outgoing file send interrupted");
            self.stop_outgoing_stream(transfer_id);
            self.set_transfer_status(transfer_id, TransferStatus::Failed(reason.to_string()));
            self.add_toast(
                ToastLevel::Error,
                format!(
                    "File may not have been delivered ({}): {}",
                    reason, filename
                ),
            );
        }
    }
    /// Fail every still-running incoming transfer of a dead session and delete
    /// its spool, freeing the chat's incoming slot for a retry. Resolves the
    /// session id to the conversation first, so call it **before** the
    /// session's `chat_id_mapping` entries are dropped.
    pub(super) fn fail_incoming_transfers(&mut self, session_id: Uuid, reason: &str) {
        let chat_id = self.resolve_display_chat_id(session_id);
        self.incoming_streams.remove(&chat_id);
        let stranded: Vec<Uuid> = self
            .active_transfers
            .iter()
            .filter_map(|(id, t)| {
                (t.chat_id == chat_id
                    && t.direction == TransferDirection::Incoming
                    && t.status.is_active())
                .then_some(*id)
            })
            .collect();

        for transfer_id in stranded {
            self.pending_file_end.remove(&transfer_id);
            self.discard_spool(transfer_id, reason);
            let filename =
                self.set_transfer_status(transfer_id, TransferStatus::Failed(reason.to_string()));
            tracing::warn!(file = %filename, reason = %reason, "Incoming file transfer interrupted");
            self.add_toast(
                ToastLevel::Error,
                format!("File transfer interrupted ({}): {}", reason, filename),
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

        let finished: Vec<Uuid> = self
            .active_transfers
            .iter()
            .filter_map(|(id, t)| (t.chat_id == chat_id && !t.status.is_active()).then_some(*id))
            .collect();
        for transfer_id in finished {
            self.active_transfers.remove(&transfer_id);
            self.incoming_files.remove(&transfer_id);
            self.outgoing_transfers.remove(&transfer_id);
        }

        // Only a transfer still *streaming* blocks a new one, not a held offer.
        if self.streaming_transfer(chat_id).is_some() {
            bail!("Another incoming file transfer is already in progress for this chat");
        }

        let transfer_id = Uuid::new_v4();
        // With auto-accept off the transfer is held in AwaitingAcceptance: chunks
        // spool, but nothing reaches downloads or history until accepted.
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
            direction: TransferDirection::Incoming,
        };

        self.active_transfers.insert(transfer_id, state);
        self.incoming_streams.insert(chat_id, transfer_id);

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

    /// Accept an offer awaiting the user's decision: finalize it now if fully
    /// received, otherwise let it continue.
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

    /// Decline an offer awaiting the user's decision: delete its spool, discard
    /// later chunks, and tell the sender to stop (or it streams the whole file).
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
        let chat_id = transfer.chat_id;
        self.pending_file_end.remove(&transfer_id);
        self.send_control(chat_id, ProtocolMessage::FileCancel { seq: 0 });
        self.discard_spool(transfer_id, "declined");
        self.add_toast(
            ToastLevel::Info,
            format!("Declined incoming file: {}", filename),
        );
        Ok(())
    }

    /// Move a fully received file into downloads, record it in the chat, and
    /// drop its bookkeeping. `ack_seq` (the FileEnd's wire seq) gets a receipt.
    pub(super) fn finalize_incoming_file(&mut self, transfer_id: Uuid, ack_seq: Option<u64>) {
        let Some(incoming) = self.incoming_files.remove(&transfer_id) else {
            return;
        };
        match incoming.finalize() {
            Ok(final_path) => {
                if let Some(mut transfer) = self.active_transfers.remove(&transfer_id) {
                    transfer.status = TransferStatus::Completed;
                    if let Some(chat) = self.chats.get_mut(&transfer.chat_id) {
                        chat.messages.push(Message::received(
                            MessageContent::File {
                                filename: transfer.filename.clone(),
                                size: transfer.size,
                                path: Some(final_path),
                            },
                            chrono::Utc::now(),
                        ));
                    }
                    if let Some(acked_seq) = ack_seq {
                        self.send_ack_for_chat(transfer.chat_id, acked_seq);
                    }
                    self.add_toast(
                        ToastLevel::Success,
                        format!("File received: {}", transfer.filename),
                    );
                    self.notify_incoming_message(
                        transfer.chat_id,
                        &format!("📎 {}", transfer.filename),
                    );
                }
            }
            Err(e) => {
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
            // Only promote Pending: a held offer keeps AwaitingAcceptance.
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

    /// Whether this held offer has taken its conversation's held data past
    /// [`MAX_UNACCEPTED_SPOOL_BYTES`].
    pub(super) fn unaccepted_spool_exceeded(&self, transfer_id: Uuid) -> bool {
        let Some(offer) = self.active_transfers.get(&transfer_id) else {
            return false;
        };
        if offer.status != TransferStatus::AwaitingAcceptance {
            return false;
        }
        let held: u64 = self
            .active_transfers
            .values()
            .filter(|t| {
                t.chat_id == offer.chat_id
                    && t.direction == TransferDirection::Incoming
                    && t.status == TransferStatus::AwaitingAcceptance
            })
            .map(|t| t.received)
            .sum();
        held > MAX_UNACCEPTED_SPOOL_BYTES
    }

    /// Decline an offer on the user's behalf because it spooled too much while
    /// waiting for them.
    pub(super) fn auto_decline_oversized_offer(&mut self, transfer_id: Uuid) {
        let filename = self
            .active_transfers
            .get(&transfer_id)
            .map(|t| t.filename.clone())
            .unwrap_or_else(|| "the incoming file".to_string());
        if let Err(e) = self.reject_incoming_file(transfer_id) {
            tracing::warn!(%transfer_id, error = %e, "could not decline an oversized held offer");
            return;
        }
        self.add_toast(
            ToastLevel::Warning,
            format!(
                "Declined {} automatically: more than {} arrived before you accepted it. \
                 Turn on auto-accept, or ask them to resend once you are ready.",
                filename,
                crate::util::format_size(MAX_UNACCEPTED_SPOOL_BYTES)
            ),
        );
    }

    /// Forget everything tracked about one transfer (stopping its stream first).
    /// Used when the conversation itself goes away.
    pub(super) fn clear_transfer_bookkeeping(&mut self, transfer_id: Uuid) {
        self.stop_outgoing_stream(transfer_id);
        self.active_transfers.remove(&transfer_id);
        self.discard_spool(transfer_id, "conversation removed");
        self.pending_file_end.remove(&transfer_id);
        self.incoming_streams.retain(|_, id| *id != transfer_id);
    }

    /// Stop an outgoing stream and drop its pending-send entry, so no completion
    /// or "may not have been delivered" toast follows a deliberate end.
    fn stop_outgoing_stream(&mut self, transfer_id: Uuid) {
        let Some(handle) = self.outgoing_transfers.remove(&transfer_id) else {
            return;
        };
        handle.cancel.store(true, Ordering::Relaxed);
        if let Some(queue) = self.pending_file_sends.get_mut(&handle.session_id) {
            queue.retain(|p| p.transfer_id != transfer_id);
        }
    }

    /// Delete an incoming transfer's partial spool (dropping the receiver alone
    /// would leave the `tmp_<uuid>_` file behind).
    pub(super) fn discard_spool(&mut self, transfer_id: Uuid, why: &str) {
        if let Some(incoming) = self.incoming_files.remove(&transfer_id) {
            if let Err(e) = incoming.abort_cleanup() {
                tracing::warn!(%transfer_id, why, error = %e, "failed to delete a partial download");
            }
        }
    }

    /// Set a transfer's status, returning its filename (empty if untracked).
    pub(super) fn set_transfer_status(
        &mut self,
        transfer_id: Uuid,
        status: TransferStatus,
    ) -> String {
        self.active_transfers
            .get_mut(&transfer_id)
            .map(|t| {
                t.status = status;
                t.filename.clone()
            })
            .unwrap_or_default()
    }

    /// Every tracked transfer belonging to a conversation, whatever its state.
    pub(super) fn transfer_ids_for_chat(&self, chat_id: Uuid) -> Vec<Uuid> {
        self.active_transfers
            .iter()
            .filter_map(|(id, t)| (t.chat_id == chat_id).then_some(*id))
            .collect()
    }

    /// The incoming transfer the conversation's wire is streaming into, if it
    /// is still live (a declined or failed one takes no more chunks).
    pub(super) fn streaming_transfer(&self, chat_id: Uuid) -> Option<Uuid> {
        self.incoming_streams
            .get(&chat_id)
            .copied()
            .filter(|id| self.is_active_transfer(*id))
    }

    fn is_active_transfer(&self, transfer_id: Uuid) -> bool {
        self.active_transfers
            .get(&transfer_id)
            .is_some_and(|t| t.status.is_active())
    }

    /// The active outgoing transfer for a chat. Tracked separately from
    /// incoming, so a send and a receive can share a chat without misrouting.
    pub(super) fn active_outgoing_transfer_id_for_chat(&self, chat_id: Uuid) -> Option<Uuid> {
        self.active_transfers
            .iter()
            .find(|(_, t)| {
                t.chat_id == chat_id
                    && t.direction == TransferDirection::Outgoing
                    && t.status.is_active()
            })
            .map(|(id, _)| *id)
    }
}
