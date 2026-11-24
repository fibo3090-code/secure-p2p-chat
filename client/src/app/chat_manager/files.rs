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

/// The channel background finalizes report on. Its own type so the manager
/// can keep deriving `Default`.
pub(super) struct FinalizeLane {
    tx: tokio::sync::mpsc::UnboundedSender<Finalized>,
    rx: tokio::sync::mpsc::UnboundedReceiver<Finalized>,
}

impl Default for FinalizeLane {
    fn default() -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        Self { tx, rx }
    }
}

/// The outcome of a background `finalize`, sent back to the manager.
pub(super) struct Finalized {
    transfer_id: Uuid,
    ack_seq: Option<u64>,
    result: Result<std::path::PathBuf, String>,
}

/// Holds a background `finalize` until released, so a test can observe the
/// window between `FileEnd` and the file landing (PERF-07).
#[cfg(test)]
pub(super) static FINALIZE_GATE: std::sync::Mutex<Option<std::sync::mpsc::Receiver<()>>> =
    std::sync::Mutex::new(None);

impl ChatManager {
    /// Remove spools left in the download folder by an earlier run.
    ///
    /// A receive spools to `tmp_<uuid>_<name>` *in the download folder*,
    /// because finalizing is a rename and a rename needs one filesystem. After
    /// a crash, or an exit with an offer still held, nothing removed them:
    /// peer-chosen bytes under a peer-chosen extension, where indexers,
    /// thumbnailers and the user would find them (APP-07). Only names of that
    /// exact shape are touched, and nothing at all while any receive is live.
    /// Returns how many were removed.
    pub fn sweep_orphaned_spools(&mut self) -> usize {
        if !self.incoming_files.is_empty() {
            return 0;
        }
        let Ok(entries) = std::fs::read_dir(&self.config.download_dir) else {
            return 0;
        };
        let is_spool = |name: &str| {
            name.strip_prefix("tmp_")
                .and_then(|rest| rest.get(..36).zip(rest.get(36..37)))
                .is_some_and(|(id, sep)| sep == "_" && Uuid::parse_str(id).is_ok())
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_str().is_some_and(is_spool)
                && entry.file_type().is_ok_and(|t| t.is_file())
                && std::fs::remove_file(entry.path()).is_ok()
            {
                removed += 1;
            }
        }
        if removed > 0 {
            tracing::info!(removed, "Removed spools left by an earlier run");
        }
        removed
    }

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

        // Placeholder seq: the session loop stamps the real monotonic wire
        // sequence onto every frame it writes. FileMeta rides the bounded `file`
        // lane so it stays ordered ahead of the chunks the stream task queues.
        if !self.chats.contains_key(&chat_id) {
            bail!("Chat not found for sending file");
        }
        // `try_send`, never `send().await`: the caller holds the manager lock,
        // and a lane still full from an earlier send behind a stalled peer
        // parked the await — freezing every command and the poll loop with it
        // (APP-02). A full lane is a moment's wait for the user, not a hang.
        file_tx
            .try_send(ProtocolMessage::FileMeta {
                filename: filename.clone(),
                size: file_size,
                seq: 0,
            })
            .map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => anyhow!(
                    "The previous file is still being flushed to this peer — try again in a moment."
                ),
                mpsc::error::TrySendError::Closed(_) => {
                    anyhow!("Session closed before file metadata was queued")
                }
            })?;
        if let Some(chat) = self.chats.get_mut(&chat_id) {
            chat.send_seq += 1;
        }

        let chat = self
            .chats
            .get_mut(&chat_id)
            .ok_or_else(|| anyhow::anyhow!("Chat not found for sending file"))?;

        // Add to local history.
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
                    && t.status.is_active()
                    // A held offer whose last byte arrived is complete on
                    // disk and waits only for the user; failing it deleted a
                    // finished download because the sender hung up (APP-05).
                    && !self.pending_file_end.contains_key(id)
                    // Every byte is in; only the write to disk is left.
                    && !self.finalizing.contains(id))
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
        // `finalize` is an fsync of up to `MAX_FILE_SIZE` and a rename: run
        // inline it held the manager lock — every command and the poll loop —
        // for as long as the disk took (PERF-07). Without a runtime (plain
        // unit tests) there is nothing to hand it to, so it stays inline.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            let result = incoming.finalize().map_err(|e| e.to_string());
            self.complete_finalized(Finalized {
                transfer_id,
                ack_seq,
                result,
            });
            return;
        };
        self.finalizing.insert(transfer_id);
        let done = self.finalized.tx.clone();
        runtime.spawn_blocking(move || {
            #[cfg(test)]
            if let Some(gate) = FINALIZE_GATE.lock().unwrap().take() {
                let _ = gate.recv();
            }
            let result = incoming.finalize().map_err(|e| e.to_string());
            let _ = done.send(Finalized {
                transfer_id,
                ack_seq,
                result,
            });
        });
    }

    /// Outcomes of background finalizes, applied on the poll that sees them.
    pub(super) fn drain_finalized(&mut self) {
        while let Ok(done) = self.finalized.rx.try_recv() {
            self.complete_finalized(done);
        }
    }

    /// Record a finished write: the file joins the conversation, or the
    /// transfer fails with the reason.
    fn complete_finalized(&mut self, done: Finalized) {
        let Finalized {
            transfer_id,
            ack_seq,
            result,
        } = done;
        self.finalizing.remove(&transfer_id);
        // Cancelled while it was being written: honour the cancel, and do not
        // leave a file behind that nothing in the app points at.
        let still_wanted = self
            .active_transfers
            .get(&transfer_id)
            .is_some_and(|t| t.status.is_active());
        if !still_wanted {
            if let Ok(path) = &result {
                std::fs::remove_file(path).ok();
            }
            return;
        }
        match result {
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
