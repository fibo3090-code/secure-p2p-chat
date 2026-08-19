//! Outgoing and incoming file transfers: validation, chunked streaming (in a
//! background task so a large send never holds the manager lock), cancellation,
//! and wire-level delivery confirmation (see SessionEvent::FileSendComplete).

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// How much of an *unaccepted* incoming file may spool to disk before the offer
/// is declined automatically.
///
/// With `auto_accept_files` off (the default) an incoming `FileMeta` is held in
/// `AwaitingAcceptance` while the sender keeps streaming — the wire has no way
/// to pause it. Without a ceiling a peer can therefore write up to
/// `MAX_FILE_SIZE` (10 GiB) into the temp directory of someone who has not
/// agreed to receive anything, and the first sign of it is a full disk.
pub const MAX_UNACCEPTED_SPOOL_BYTES: u64 = 64 * 1024 * 1024; // 64 MiB

/// Arguments for [`ChatManager::spawn_file_stream`]. Grouped into a struct to
/// keep the streamer's setup readable (and satisfy the too-many-arguments lint).
struct SpawnFileStream {
    /// Bounded lane for bulk data frames (`FileChunk`/`FileEnd`); provides
    /// backpressure via `send().await`.
    file_tx: mpsc::Sender<ProtocolMessage>,
    /// Unbounded control lane for the abort frame (`FileCancel`), so a cancel is
    /// never queued behind pending chunks.
    control_tx: mpsc::UnboundedSender<ProtocolMessage>,
    path: std::path::PathBuf,
    filename: String,
    file_size: u64,
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicU64>,
    failed: Arc<AtomicBool>,
}

impl ChatManager {
    /// Send a file to a chat.
    ///
    /// The transfer is registered and its chunks are streamed from a spawned
    /// task, so this returns as soon as the metadata frame is queued — a
    /// multi-gigabyte send neither blocks the caller nor holds the manager
    /// lock, and it can be cancelled mid-flight via [`cancel_transfer`].
    pub async fn send_file(&mut self, chat_id: Uuid, path: std::path::PathBuf) -> Result<()> {
        tracing::info!(chat_id = %chat_id, path = %path.display().to_string(), "Preparing to send file");
        // One outgoing transfer per conversation at a time. `FileChunk` carries
        // no transfer id, so two concurrent streams on the same session
        // interleave their chunks into whichever spool the receiver has open —
        // silently corrupting BOTH files. Serializing here is what makes the
        // wire format safe; do not relax it without adding a transfer id to the
        // chunk frames on both sides of `protocol.rs`.
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
        let session_id = *self.chat_id_mapping.get(&chat_id).unwrap_or(&chat_id);
        let (control_tx, file_tx) = self
            .sessions
            .get(&session_id)
            .map(|s| (s.from_app_tx.clone(), s.file_tx.clone()))
            .ok_or_else(|| anyhow!("Session not found"))?;

        let (filename, file_size) = Self::validate_outgoing_file(&path).await?;
        tracing::debug!(file = %filename, size = %file_size, "Sending file metadata");

        // Placeholder seq: the session loop stamps the real monotonic wire
        // sequence onto every frame it writes. FileMeta rides the bounded `file`
        // lane so it stays ordered ahead of the chunks the stream task queues.
        {
            let chat = self
                .chats
                .get_mut(&chat_id)
                .ok_or_else(|| anyhow::anyhow!("Chat not found for sending file"))?;
            chat.send_seq += 1;
        }
        file_tx
            .send(ProtocolMessage::FileMeta {
                filename: filename.clone(),
                size: file_size,
                seq: 0,
            })
            .await
            .map_err(|_| anyhow!("Session closed before file metadata was queued"))?;

        let chat = self
            .chats
            .get_mut(&chat_id)
            .ok_or_else(|| anyhow::anyhow!("Chat not found for sending file"))?;

        // Add to local history.
        let message_id = Uuid::new_v4();
        chat.messages.push(Message {
            id: message_id,
            from_me: true,
            content: MessageContent::File {
                filename: filename.clone(),
                size: file_size,
                path: Some(path.clone()),
            },
            timestamp: chrono::Utc::now(),
            delivered: false,
        });

        // Track the transfer so the UI can show progress and cancel it.
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

        // Queueing is not delivery: the success toast waits for the session to
        // report the final frame on the wire (SessionEvent::FileSendComplete).
        // File frames drain FIFO per session, so a per-session queue correlates;
        // keyed by session id because that is where the event will arrive. The
        // chat/message ids let the completion register for a delivery receipt.
        self.pending_file_sends
            .entry(session_id)
            .or_default()
            .push_back((filename.clone(), transfer_id, chat_id, message_id));

        // Stream the chunks off-thread so the send never holds the manager lock
        // for the (potentially multi-gigabyte) transfer and can be cancelled
        // between chunks. Bounded backpressure (the `file` lane) caps how far the
        // reader may run ahead of the network, so a slow link cannot balloon the
        // outbound queue. A cancel or local I/O error stops without a `FileEnd`,
        // so no `FileSendComplete` fires for it.
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

    /// Background chunk streamer for one outgoing file. Runs until EOF (then
    /// emits `FileEnd`), cancellation (then emits `FileCancel` and flags the
    /// task done), a local I/O error (flags `failed` so the poll loop marks the
    /// transfer `Failed`), or a dead channel (peer gone — stop silently; the
    /// session teardown reports it).
    ///
    /// Data frames (`FileMeta` is already queued by the caller, then `FileChunk`
    /// / `FileEnd`) ride the **bounded** `file_tx` lane, so `send().await`
    /// applies backpressure: the reader blocks once the lane is full instead of
    /// buffering the whole file in memory when the network is slower than disk.
    /// Abort frames (`FileCancel`) ride the unbounded `control_tx` lane so a
    /// cancel is never stuck behind queued chunks.
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
                // Bounded lane: awaits when full, so a slow peer paces the reader.
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
                if sent_chunks % 64 == 0 {
                    tracing::trace!(sent_chunks = %sent_chunks, "File sending progress");
                }
            }
            let _ = file_tx.send(ProtocolMessage::FileEnd { seq: 0 }).await;
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
                        queue.retain(|(_, id, _, _)| *id != transfer_id);
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
        // Collect transfers whose stream task hit a local I/O error, so we can
        // finalize them after the immutable borrow of `outgoing_transfers` ends.
        let mut failed_ids = Vec::new();
        for (transfer_id, handle) in &self.outgoing_transfers {
            if handle.failed.load(Ordering::Relaxed) {
                failed_ids.push((*transfer_id, handle.session_id));
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

        for (transfer_id, session_id) in failed_ids {
            self.outgoing_transfers.remove(&transfer_id);
            // Drop the pending-delivery entry so session teardown doesn't also
            // report it as "not delivered".
            if let Some(queue) = self.pending_file_sends.get_mut(&session_id) {
                queue.retain(|(_, id, _, _)| *id != transfer_id);
            }
            let filename = self
                .active_transfers
                .get_mut(&transfer_id)
                .map(|state| {
                    state.status = TransferStatus::Failed("could not read the file".to_string());
                    state.filename.clone()
                })
                .unwrap_or_default();
            self.add_toast(
                ToastLevel::Error,
                format!("File send failed (could not read the file): {}", filename),
            );
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
                queue.retain(|(_, id, _, _)| *id != transfer_id);
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
        for (filename, transfer_id, _chat, _message) in pending {
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
    /// Abort every still-running *incoming* transfer on a session that just
    /// died: mark it failed and delete its partial spool.
    ///
    /// `session_id` is what the event pump reports; transfers are tracked under
    /// the conversation the UI displays, so it is resolved first (on the host
    /// side those differ). Call this **before** the session's `chat_id_mapping`
    /// entries are dropped, or the transfer can no longer be matched.
    ///
    /// Without it a mid-receive disconnect left a progress row stuck at its last
    /// byte count forever plus an orphaned temp file on disk, and the chat's
    /// incoming slot occupied so the peer could never retry the send.
    pub(super) fn fail_incoming_transfers(&mut self, session_id: Uuid, reason: &str) {
        let chat_id = self.resolve_display_chat_id(session_id);
        let stranded: Vec<Uuid> = self
            .active_transfers
            .iter()
            .filter_map(|(id, t)| {
                let active = matches!(
                    t.status,
                    TransferStatus::Pending
                        | TransferStatus::AwaitingAcceptance
                        | TransferStatus::InProgress
                );
                (t.chat_id == chat_id && t.direction == TransferDirection::Incoming && active)
                    .then_some(*id)
            })
            .collect();

        for transfer_id in stranded {
            self.pending_file_end.remove(&transfer_id);
            if let Some(incoming) = self.incoming_files.remove(&transfer_id) {
                if let Err(e) = incoming.abort_cleanup() {
                    tracing::warn!(%transfer_id, error = %e, "Failed to clean up stranded transfer");
                }
            }
            let filename = self
                .active_transfers
                .get_mut(&transfer_id)
                .map(|t| {
                    t.status = TransferStatus::Failed(reason.to_string());
                    t.filename.clone()
                })
                .unwrap_or_default();
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
            direction: TransferDirection::Incoming,
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
        let chat_id = transfer.chat_id;
        self.pending_file_end.remove(&transfer_id);
        // Tell the sender to stop, exactly as `cancel_transfer` does. Without
        // this, declining a 5 GB offer still pulls all 5 GB across the wire —
        // the decision only ever reached our own disk.
        let session_id = *self.chat_id_mapping.get(&chat_id).unwrap_or(&chat_id);
        if let Some(session) = self.sessions.get(&session_id) {
            let _ = session
                .from_app_tx
                .send(ProtocolMessage::FileCancel { seq: 0 });
        }
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
                    // A file landing in a background conversation deserves the
                    // same desktop notification a text message gets.
                    self.notify_incoming_message(
                        transfer.chat_id,
                        &format!("📎 {}", transfer.filename),
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

    /// Whether an offer still awaiting the user's decision has spooled more
    /// than [`MAX_UNACCEPTED_SPOOL_BYTES`] to disk.
    pub(super) fn unaccepted_spool_exceeded(&self, transfer_id: Uuid) -> bool {
        self.active_transfers.get(&transfer_id).is_some_and(|t| {
            t.status == TransferStatus::AwaitingAcceptance
                && t.received > MAX_UNACCEPTED_SPOOL_BYTES
        })
    }

    /// Decline an offer on the user's behalf because it has spooled too much
    /// while waiting for them.
    ///
    /// Silently accepting an unbounded spool is the wrong trade: the user asked
    /// to approve incoming files, and "approve" cannot mean "we already wrote
    /// nine gigabytes to your disk while you thought about it".
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

    /// Forget everything tracked about one transfer, stopping its streaming
    /// task first so a cancelled send does not keep reading from disk into a
    /// channel nobody is draining. Used when the conversation itself goes away.
    pub(super) fn clear_transfer_bookkeeping(&mut self, transfer_id: Uuid) {
        if let Some(handle) = self.outgoing_transfers.remove(&transfer_id) {
            handle.cancel.store(true, Ordering::Relaxed);
        }
        self.active_transfers.remove(&transfer_id);
        self.incoming_files.remove(&transfer_id);
        self.pending_file_end.remove(&transfer_id);
    }

    /// Every tracked transfer belonging to a conversation, whatever its state.
    /// Used when the conversation itself goes away and all of its transfer
    /// bookkeeping must go with it.
    pub(super) fn transfer_ids_for_chat(&self, chat_id: Uuid) -> Vec<Uuid> {
        self.active_transfers
            .iter()
            .filter_map(|(id, t)| (t.chat_id == chat_id).then_some(*id))
            .collect()
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

    /// The active (pending/awaiting/in-progress) transfer for a chat in a given
    /// direction, if any. Incoming and outgoing are tracked separately so a
    /// send and a receive can run on the same chat without misrouting.
    /// AwaitingAcceptance counts as active: an incoming offer held for the
    /// user's decision still owns the chat's incoming slot.
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
                        TransferStatus::Pending
                            | TransferStatus::AwaitingAcceptance
                            | TransferStatus::InProgress
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
