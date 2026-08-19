//! Session-event pump: polling the per-session event channels and reacting
//! to everything the network layer reports (messages, files, TOFU, lifecycle).

use super::*;

impl ChatManager {
    /// Poll and process all pending session events
    pub fn poll_session_events(&mut self) {
        self.cleanup_stale_incoming_text_messages();
        self.sync_outgoing_transfer_progress();
        self.poll_upnp_result();
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

    /// Register a sent message as waiting for the peer's delivery receipt,
    /// keeping the map bounded: a peer that predates `Ack` never acknowledges
    /// anything, so on a long-lived session the entries would otherwise grow
    /// forever. Hitting the cap means the peer isn't acking at all — dropping
    /// the backlog only costs ✓ marks that were never going to arrive.
    pub(super) fn register_awaiting_ack(
        &mut self,
        session_id: Uuid,
        seq: u64,
        chat_id: Uuid,
        message_id: Uuid,
    ) {
        const MAX_AWAITING_ACK: usize = 2048;
        if self.awaiting_ack.len() >= MAX_AWAITING_ACK {
            tracing::info!("awaiting_ack backlog full (peer not acking); dropping old entries");
            self.awaiting_ack.clear();
        }
        self.awaiting_ack
            .insert((session_id, seq), (chat_id, message_id));
    }

    /// Consolidate Trust-On-First-Use (TOFU) verification logic
    pub(super) fn handle_tofu_verification(
        &mut self,
        session_id: Uuid,
        fingerprint: &str,
        peer_name: &str,
        sas: &str,
    ) {
        // Resolve the actual chat ID (important for host mode where session_id != chat_id)
        let actual_chat_id = self
            .chat_id_mapping
            .iter()
            .find(|(_, &sid)| sid == session_id)
            .map(|(&cid, _)| cid)
            .unwrap_or(session_id);

        // A blocked contact's fingerprint is refused outright: no prompt, no
        // auto-accept, and the session is told to abort.
        if self.is_fingerprint_blocked(fingerprint) {
            tracing::warn!(peer = %peer_name, "Rejected connection from blocked contact");
            if let Some(tx) = self.fingerprint_confirm_senders.get(&session_id) {
                let _ = tx.send(false);
            }
            self.add_toast(
                ToastLevel::Warning,
                format!("Blocked contact {} tried to connect — refused", peer_name),
            );
            return;
        }

        // Have we already *confirmed* this fingerprint elsewhere — another chat
        // with this peer, or a contact whose fingerprint we actually verified?
        // Then it's a returning peer under TOFU and we accept without another
        // prompt. Computed before the mutable borrow below.
        //
        // A contact only counts when its trust state says the fingerprint was
        // confirmed. Matching *any* contact meant that pasting an invite link
        // pre-trusted whatever fingerprint it named: the peer then connected
        // with no SAS prompt at all, which is the one check the whole product
        // is built around. An imported contact starts `Unverified`, so it now
        // has to pass verification like anyone else — and becomes `Verified`
        // through `promote_contact_verified` once it does.
        let known_trusted = self.chats.iter().any(|(id, c)| {
            *id != actual_chat_id && c.peer_fingerprint.as_deref() == Some(fingerprint)
        }) || self.contacts.values().any(|c| {
            c.fingerprint.as_deref() == Some(fingerprint)
                && matches!(c.trust_state, TrustState::Verified | TrustState::Trusted)
        });

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
                    // The handshake confirmed this fingerprint; reflect it on
                    // the matching contact (returning peer stays consistent).
                    self.promote_contact_verified(actual_chat_id, fingerprint);
                } else {
                    // Request explicit user verification via UI.
                    // Note: the prompt is keyed by SESSION ID because the
                    // accept/reject decision must be sent to that session's task.
                    // Queued, not assigned: two peers dialing a host at once
                    // must each get their prompt rather than the second silently
                    // replacing the first and stranding that session.
                    self.queue_fingerprint_request(PendingFingerprint {
                        fingerprint: fingerprint.to_string(),
                        peer_name: peer_name.to_string(),
                        sas: sas.to_string(),
                        session_id,
                    });
                    let waiting = self.pending_fingerprint_count();
                    self.add_toast(
                        ToastLevel::Warning,
                        if waiting > 1 {
                            format!("Fingerprint verification required ({} waiting)", waiting)
                        } else {
                            "Fingerprint verification required".to_string()
                        },
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
                self.queue_fingerprint_request(PendingFingerprint {
                    fingerprint: fingerprint.to_string(),
                    peer_name: peer_name.to_string(),
                    sas: sas.to_string(),
                    session_id,
                });
                self.add_toast(
                    ToastLevel::Warning,
                    "SECURITY WARNING: Peer fingerprint has changed!".to_string(),
                );
            }
        }
    }

    /// Handle a single session event
    pub(super) fn handle_session_event(&mut self, chat_id: Uuid, event: SessionEvent) {
        tracing::debug!("Handling session event for {}: {:?}", chat_id, event);

        match event {
            SessionEvent::Listening { port } => {
                tracing::info!("Session {} listening on port {}", chat_id, port);
                self.add_toast(ToastLevel::Info, format!("Listening on port {}", port));
            }

            SessionEvent::Connected { peer } => {
                tracing::info!("Session {} connected to {}", chat_id, peer);
                // A "p2p:" label means the relay rendezvous hole punched a
                // direct socket, so the chat is no longer relay-transported.
                let hole_punched = peer.starts_with("p2p:");
                if hole_punched {
                    self.add_toast(
                        ToastLevel::Success,
                        format!("Direct connection established (hole punched): {}", peer),
                    );
                } else {
                    self.add_toast(ToastLevel::Success, format!("Connected to {}", peer));
                }

                if let Some(chat) = self.chats.get_mut(&chat_id) {
                    // Only re-derive the title from the peer label when the user
                    // has not named this conversation themselves — otherwise a
                    // reconnect silently replaced "Mum" with an IP:port.
                    if !chat.title_is_custom {
                        chat.title = peer;
                    }
                    chat.is_host_placeholder = false;
                    if hole_punched {
                        chat.transport = Transport::Direct;
                    }
                    // A fresh session restarts the wire sequence at 1, but this
                    // chat's high-water mark is left over from the previous
                    // one — everything at or below it would be rejected as a
                    // replay, silently swallowing the peer's first messages.
                    // Transport-layer replay protection is per session and
                    // still enforced in `run_message_loop`.
                    chat.recv_seq = 0;
                    chat.send_seq = 0;
                }
            }

            SessionEvent::NewConnection {
                peer_addr,
                fingerprint,
                sas,
                chat_id: incoming_chat_id,
            } => {
                tracing::info!(
                    "New incoming connection from {} with chat_id {}, session_chat_id={}",
                    peer_addr,
                    incoming_chat_id,
                    chat_id,
                );

                // A returning peer belongs in the conversation it already has.
                //
                // The host keys incoming chats by the *client's* chat id, which
                // is fresh whenever the client hasn't kept its own mapping — so
                // every reconnect used to open another "Peer ab12cd34" and the
                // history fragmented into a pile of near-identical threads. The
                // fingerprint arrives verified (the identity proof was checked
                // before this event), so it is the right key: prefer an existing
                // conversation with that fingerprint over the id we were handed.
                //
                // Only an idle one, though — a chat with a live session belongs
                // to a connection that is still running, and merging a second
                // peer into it would cross two conversations together.
                let incoming_chat_id = self
                    .chats
                    .iter()
                    .find(|(id, c)| {
                        c.peer_fingerprint.as_deref() == Some(fingerprint.as_str())
                            && !c.is_host_placeholder
                            && **id != chat_id
                            && !self.is_connected(id)
                    })
                    .map(|(id, _)| *id)
                    .unwrap_or(incoming_chat_id);

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
                // Relay chat, not a hardcoded Direct one. Exception: a "p2p:"
                // peer label means the rendezvous hole punched a direct socket,
                // so the chat is Direct even though it started via the relay.
                let inherited_transport = if peer_addr.starts_with("p2p:") {
                    Transport::Direct
                } else {
                    self.chats
                        .get(&chat_id)
                        .map(|c| c.transport)
                        .unwrap_or(Transport::Direct)
                };

                // Create a chat for this new connection, or (on reconnect, where
                // the entry already exists) normalize its transport so it isn't
                // left as a stale Direct — the same fix applied in connect_via_relay.
                self.chats
                    .entry(incoming_chat_id)
                    .and_modify(|c| {
                        c.transport = inherited_transport;
                        // Reconnect onto an existing conversation: the peer's
                        // wire sequence restarts at 1 for the new session, so a
                        // leftover high-water mark would reject everything it
                        // sends until it climbs past the old one.
                        c.recv_seq = 0;
                        c.send_seq = 0;
                    })
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
                        read_count: 0,
                        title_is_custom: false,
                    });

                // If this connection consumes a placeholder host chat, remove the placeholder
                // so the UI shows only the real chat. Auto-rehost will spawn a new listener.
                if let Some(placeholder) = self.chats.get(&chat_id) {
                    if placeholder.is_host_placeholder {
                        tracing::debug!("Removing consumed host placeholder chat {}", chat_id);
                        self.chats.remove(&chat_id);
                    }
                }

                self.handle_tofu_verification(chat_id, &fingerprint, &peer_addr, &sas);
            }

            SessionEvent::ShowFingerprintVerification {
                fingerprint,
                peer_name,
                sas,
                chat_id,
            } => {
                self.handle_tofu_verification(chat_id, &fingerprint, &peer_name, &sas);
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
                                    delivered: false,
                                });

                                // Clear typing indicator
                                chat.peer_typing = false;
                                chat.typing_since = None;
                                // Delivery receipt for the sender.
                                self.send_ack_for_chat(actual_chat_id, seq);
                                // Desktop notification — suppressed when this
                                // conversation is already on screen and focused.
                                let preview = Self::preview_text_for_notification(&text);
                                self.notify_incoming_message(actual_chat_id, &preview);

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
                                        delivered: false,
                                    });
                                    chat.peer_typing = false;
                                    chat.typing_since = None;
                                }
                                // Delivery receipt: ack the final chunk's seq
                                // (chunks arrive in order on the one stream).
                                self.send_ack_for_chat(actual_chat_id, seq);

                                let preview = Self::preview_text_for_notification(&text);
                                self.notify_incoming_message(actual_chat_id, &preview);
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

                        let transfer_id = self.active_incoming_transfer_id_for_chat(actual_chat_id);
                        if let Some(transfer_id) = transfer_id {
                            // An offer the user has not accepted yet still
                            // spools to disk, because the sender streams
                            // without waiting for a decision. Uncapped, that
                            // means a peer can put up to MAX_FILE_SIZE (10 GiB)
                            // on the disk while the accept/decline prompt is
                            // still on screen. Past the cap the offer is
                            // declined for them.
                            if self.unaccepted_spool_exceeded(transfer_id) {
                                self.auto_decline_oversized_offer(transfer_id);
                                return;
                            }
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

                        let transfer_id = self.active_incoming_transfer_id_for_chat(actual_chat_id);
                        if let Some(transfer_id) = transfer_id {
                            let awaiting = self
                                .active_transfers
                                .get(&transfer_id)
                                .is_some_and(|t| t.status == TransferStatus::AwaitingAcceptance);
                            if awaiting {
                                // Fully received but not yet accepted: hold the
                                // spooled file (and the FileEnd seq, for the
                                // eventual delivery receipt) until the user decides.
                                tracing::info!(
                                    "File fully received; holding until the user accepts"
                                );
                                self.pending_file_end.insert(transfer_id, seq);
                            } else if self.active_transfers.contains_key(&transfer_id) {
                                tracing::info!("File transfer completed");
                                // Finalize only the matching transfer, not all incoming files.
                                self.finalize_incoming_file(transfer_id, Some(seq));
                            }
                        }
                    }

                    ProtocolMessage::FileCancel { seq } => {
                        let valid_seq = if let Some(chat) = self.chats.get_mut(&actual_chat_id) {
                            if seq > chat.recv_seq {
                                chat.recv_seq = seq;
                                true
                            } else {
                                tracing::warn!("Received FileCancel with invalid sequence number for chat {}. Expected > {}, got {}. Discarding.", actual_chat_id, chat.recv_seq, seq);
                                false
                            }
                        } else {
                            false
                        };
                        if valid_seq {
                            tracing::info!(
                                "Peer cancelled the file transfer for chat {}",
                                actual_chat_id
                            );
                            self.handle_peer_file_cancel(actual_chat_id);
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

                    ProtocolMessage::Ack { acked_seq, seq } => {
                        let valid_seq = if let Some(chat) = self.chats.get_mut(&actual_chat_id) {
                            if seq > chat.recv_seq {
                                chat.recv_seq = seq;
                                true
                            } else {
                                tracing::warn!("Received Ack with invalid sequence number for chat {}. Expected > {}, got {}. Discarding.", actual_chat_id, chat.recv_seq, seq);
                                false
                            }
                        } else {
                            false
                        };
                        if !valid_seq {
                            return;
                        }
                        // Mark the acknowledged message as delivered. The key is
                        // the SESSION id: that is where the wire seq was stamped.
                        if let Some((target_chat, message_id)) =
                            self.awaiting_ack.remove(&(chat_id, acked_seq))
                        {
                            if let Some(chat) = self.chats.get_mut(&target_chat) {
                                if let Some(message) =
                                    chat.messages.iter_mut().find(|m| m.id == message_id)
                                {
                                    message.delivered = true;
                                    tracing::debug!(
                                        "Message {} in chat {} marked delivered",
                                        message_id,
                                        target_chat
                                    );
                                }
                            }
                        } else {
                            tracing::debug!(
                                "Ack for unknown seq {} on session {} (already acked, or sent before restart)",
                                acked_seq,
                                chat_id
                            );
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

            SessionEvent::FileSendComplete { seq } => {
                match self
                    .pending_file_sends
                    .get_mut(&chat_id)
                    .and_then(|q| q.pop_front())
                {
                    Some((filename, transfer_id, target_chat, message_id)) => {
                        tracing::info!(file = %filename, seq = %seq, "File send complete (final frame on the wire)");
                        // The stream finished; mark the tracked transfer done
                        // and retire its live handle.
                        self.outgoing_transfers.remove(&transfer_id);
                        if let Some(state) = self.active_transfers.get_mut(&transfer_id) {
                            state.received = state.size;
                            state.status = TransferStatus::Completed;
                        }
                        // Wait for the peer's receipt to mark it delivered.
                        self.register_awaiting_ack(chat_id, seq, target_chat, message_id);
                        self.add_toast(ToastLevel::Success, format!("File sent: {}", filename));
                    }
                    None => {
                        tracing::warn!(seq = %seq, "FileSendComplete with no pending file send");
                    }
                }
            }

            SessionEvent::TextSendComplete { seq } => {
                // The frame carrying the oldest queued text message hit the wire
                // with this seq (FIFO per session): register it so the peer's
                // Ack can mark the right message delivered.
                match self
                    .pending_text_sends
                    .get_mut(&chat_id)
                    .and_then(|q| q.pop_front())
                {
                    Some((target_chat, message_id)) => {
                        self.register_awaiting_ack(chat_id, seq, target_chat, message_id);
                    }
                    None => {
                        tracing::warn!(seq = %seq, "TextSendComplete with no pending text send");
                    }
                }
            }

            SessionEvent::Disconnected => {
                tracing::warn!("Session {} disconnected", chat_id);
                self.add_toast(ToastLevel::Warning, "Connection lost".to_string());
                self.fail_pending_file_sends(chat_id, "connection lost");
                // Receives die with the session too. This must run before the
                // chat_id_mapping entries below are dropped — that mapping is
                // how a host-side session id resolves to the conversation the
                // transfer is tracked under.
                self.fail_incoming_transfers(chat_id, "connection lost");

                // Clean up session (unacked messages simply stay unmarked).
                self.sessions.remove(&chat_id);
                self.session_events.remove(&chat_id);
                self.chat_id_mapping.retain(|_, v| *v != chat_id);
                self.pending_text_sends.remove(&chat_id);
                self.awaiting_ack.retain(|(sid, _), _| *sid != chat_id);
                self.fingerprint_confirm_senders.remove(&chat_id);

                // A listener that died without ever accepting anyone leaves its
                // placeholder chat behind, and `check_rehost_needed()` then
                // reports "a placeholder exists, nothing to do" forever — so the
                // app shows a Host conversation with no listener under it and
                // never accepts another peer. Drop it and let auto-rehost rebind.
                if self
                    .chats
                    .get(&chat_id)
                    .is_some_and(|c| c.is_host_placeholder)
                {
                    tracing::info!(%chat_id, "Host placeholder removed: its listener is gone");
                    self.chats.remove(&chat_id);
                }
                // A pending TOFU prompt for a session that just died can never
                // be answered; leaving it queued blocks the next peer's prompt.
                self.drop_fingerprint_requests_for_session(chat_id);
            }

            SessionEvent::Error(err) => {
                tracing::error!("Session {} error: {}", chat_id, err);
                self.add_toast(ToastLevel::Error, format!("Connection error: {}", err));
                self.fail_pending_file_sends(chat_id, "connection error");
            }

            SessionEvent::Warning(msg) => {
                tracing::warn!("Session {} warning: {}", chat_id, msg);
                self.add_toast(ToastLevel::Warning, msg);
            }
        }
    }

    /// Resolve a background UPnP mapping attempt started by `start_host`.
    /// On success the external address is stored (and preferred by invite
    /// generation); on failure the host keeps working LAN/relay-only.
    fn poll_upnp_result(&mut self) {
        let Some(rx) = self.pending_upnp.as_mut() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(mapping)) => {
                let addr = mapping.to_host_port();
                let proto = match mapping.protocol {
                    crate::network::nat::Protocol::Upnp => "UPnP",
                    crate::network::nat::Protocol::NatPmp => "NAT-PMP",
                };
                tracing::info!(%addr, protocol = proto, "port mapping active");
                self.add_toast(
                    ToastLevel::Success,
                    format!("{}: reachable from the internet at {}", proto, addr),
                );
                self.external_address = Some(addr);
                self.pending_upnp = None;
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "UPnP mapping failed");
                self.add_toast(
                    ToastLevel::Warning,
                    format!("UPnP port mapping failed: {} (LAN/relay still work)", e),
                );
                self.pending_upnp = None;
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                self.pending_upnp = None;
            }
        }
    }
}
