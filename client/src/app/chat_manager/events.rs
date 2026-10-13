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

    /// Trust-On-First-Use (TOFU): decide whether a peer's verified identity
    /// may proceed on its own, must be confirmed by the user, or is refused.
    pub(super) fn handle_tofu_verification(
        &mut self,
        session_id: Uuid,
        fingerprint: &str,
        peer_name: &str,
        sas: &str,
    ) {
        // On the host the session id is the placeholder's; the fingerprint
        // belongs to the incoming conversation the peer created.
        let chat_id = self.chat_for_session(session_id);

        // A blocked contact's fingerprint is refused outright: no prompt, no
        // auto-accept, and the session is told to abort.
        if self.is_fingerprint_blocked(fingerprint) {
            tracing::warn!(peer = %peer_name, "Rejected connection from blocked contact");
            self.answer_fingerprint_check(session_id, false);
            self.add_toast(
                ToastLevel::Warning,
                format!("Blocked contact {} tried to connect — refused", peer_name),
            );
            return;
        }

        let Some(stored) = self.chats.get(&chat_id).map(|c| c.peer_fingerprint.clone()) else {
            tracing::error!("TOFU check failed: Chat {} not found", chat_id);
            return;
        };
        let prompt = PendingFingerprint {
            fingerprint: fingerprint.to_string(),
            peer_name: peer_name.to_string(),
            sas: sas.to_string(),
            session_id,
        };

        match stored.as_deref() {
            Some(known) if known == fingerprint => {
                tracing::debug!(%chat_id, "Fingerprint matches stored value; proceeding");
                self.answer_fingerprint_check(session_id, true);
            }
            Some(known) => {
                tracing::warn!(
                    %chat_id,
                    stored = %known,
                    new = %fingerprint,
                    "FINGERPRINT MISMATCH"
                );
                self.queue_fingerprint_request(prompt);
                self.add_toast(
                    ToastLevel::Warning,
                    "SECURITY WARNING: Peer fingerprint has changed!".to_string(),
                );
            }
            // First use, but the user opted into auto-trust, or this is a peer
            // already verified elsewhere — a returning peer under TOFU.
            None if self.config.auto_trust_on_first_use
                || self.fingerprint_known_trusted(fingerprint, chat_id) =>
            {
                tracing::info!(%chat_id, "Fingerprint trusted without a prompt");
                if let Some(chat) = self.chats.get_mut(&chat_id) {
                    chat.peer_fingerprint = Some(fingerprint.to_string());
                }
                self.answer_fingerprint_check(session_id, true);
                // Reflect it on the matching contact so a returning peer stays
                // consistent.
                self.promote_contact_verified(chat_id, fingerprint);
            }
            // First use: the user compares the safety code out of band. Keyed
            // by SESSION id, because the answer goes to that session's task;
            // queued, so two peers dialing a host at once each get a prompt.
            None => {
                tracing::info!(%chat_id, "Trust on first use: requesting user confirmation");
                self.queue_fingerprint_request(prompt);
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
    }

    /// Has this fingerprint already been *confirmed* somewhere other than
    /// `chat_id` — another conversation with this peer, or a contact whose
    /// fingerprint was actually verified?
    ///
    /// A contact only counts when its trust state says so. Matching *any*
    /// contact meant that pasting an invite link pre-trusted whatever
    /// fingerprint it named: the peer then connected with no SAS prompt at all,
    /// which is the one check the whole product is built around. An imported
    /// contact starts `Unverified` and becomes `Verified` through
    /// `promote_contact_verified` once the user accepts.
    fn fingerprint_known_trusted(&self, fingerprint: &str, chat_id: Uuid) -> bool {
        self.chats
            .iter()
            .any(|(id, c)| *id != chat_id && c.peer_fingerprint.as_deref() == Some(fingerprint))
            || self.contacts.values().any(|c| {
                c.fingerprint.as_deref() == Some(fingerprint)
                    && matches!(c.trust_state, TrustState::Verified | TrustState::Trusted)
            })
    }

    /// Tell a session task the outcome of its fingerprint check.
    fn answer_fingerprint_check(&self, session_id: Uuid, accept: bool) {
        if let Some(tx) = self.fingerprint_confirm_senders.get(&session_id) {
            if let Err(e) = tx.send(accept) {
                tracing::error!(%session_id, "Failed to answer the fingerprint check: {}", e);
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
                let hole_punched = is_hole_punched(&peer);
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
                    chat.reset_sequences();
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
                self.accept_incoming_connection(
                    chat_id,
                    incoming_chat_id,
                    &fingerprint,
                    &peer_addr,
                );
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

            SessionEvent::MessageReceived(frame) => self.handle_peer_frame(chat_id, frame),

            SessionEvent::FileSendComplete { seq } => {
                let Some(PendingFileSend {
                    filename,
                    transfer_id,
                    chat_id: target_chat,
                    message_id,
                }) = self
                    .pending_file_sends
                    .get_mut(&chat_id)
                    .and_then(|q| q.pop_front())
                else {
                    tracing::warn!(seq = %seq, "FileSendComplete with no pending file send");
                    return;
                };
                tracing::info!(file = %filename, seq = %seq, "File send complete (final frame on the wire)");
                // The stream finished; mark the tracked transfer done and retire
                // its live handle.
                self.outgoing_transfers.remove(&transfer_id);
                if let Some(state) = self.active_transfers.get_mut(&transfer_id) {
                    state.received = state.size;
                    state.status = TransferStatus::Completed;
                }
                // Wait for the peer's receipt to mark it delivered.
                self.register_awaiting_ack(chat_id, seq, target_chat, message_id);
                self.add_toast(ToastLevel::Success, format!("File sent: {}", filename));
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

                self.teardown_session(chat_id);
                self.chat_id_mapping.retain(|_, v| *v != chat_id);

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

    /// A host listener accepted a peer: route the session into a conversation.
    fn accept_incoming_connection(
        &mut self,
        session_id: Uuid,
        offered_chat_id: Uuid,
        fingerprint: &str,
        peer_addr: &str,
    ) {
        // A returning peer belongs in the conversation it already has.
        //
        // The host keys incoming chats by the *client's* chat id, which is fresh
        // whenever the client hasn't kept its own mapping — so every reconnect
        // used to open another "Peer ab12cd34" and the history fragmented into a
        // pile of near-identical threads. The fingerprint arrives verified (the
        // identity proof was checked before this event), so it is the right key:
        // prefer an existing conversation with that fingerprint over the id we
        // were handed.
        //
        // Only an idle one, though — a chat with a live session belongs to a
        // connection that is still running, and merging a second peer into it
        // would cross two conversations together.
        let chat_id = self
            .chats
            .iter()
            .find(|(id, c)| {
                c.peer_fingerprint.as_deref() == Some(fingerprint)
                    && !c.is_host_placeholder
                    && **id != session_id
                    && !self.is_connected(id)
            })
            .map(|(id, _)| *id)
            .unwrap_or(offered_chat_id);

        // Messages sent to the conversation are routed to the session that
        // accepted it (the placeholder host's).
        self.chat_id_mapping.insert(chat_id, session_id);
        tracing::debug!(
            "Mapped incoming chat {} to session chat {}",
            chat_id,
            session_id
        );

        // Inherit the transport of the host session that accepted this peer: a
        // relay-hosted listener must produce a Relay chat — unless the
        // rendezvous hole punched a direct socket.
        let transport = if is_hole_punched(peer_addr) {
            Transport::Direct
        } else {
            self.chats
                .get(&session_id)
                .map(|c| c.transport)
                .unwrap_or(Transport::Direct)
        };

        // Create the conversation, or (on reconnect) bring the existing one in
        // line with its new session: the peer's wire sequence restarts at 1, so
        // a leftover high-water mark would reject everything it sends until it
        // climbs past the old one.
        //
        // A new chat starts with no `peer_fingerprint`, so TOFU actually runs:
        // pre-filling the peer's own fingerprint made the check trivially
        // "match" and silently auto-trusted every caller.
        let title = format!(
            "Peer {}",
            crate::util::format_fingerprint_short(fingerprint)
        );
        self.chats
            .entry(chat_id)
            .and_modify(|c| {
                c.transport = transport;
                c.reset_sequences();
            })
            .or_insert_with(|| Chat::new(chat_id, title, ChatKind::Dm, transport));

        // The listener's placeholder is consumed: the UI shows only the real
        // conversation, and auto-rehost spawns a new listener.
        if self
            .chats
            .get(&session_id)
            .is_some_and(|c| c.is_host_placeholder)
        {
            tracing::debug!("Removing consumed host placeholder chat {}", session_id);
            self.chats.remove(&session_id);
        }
    }

    /// One application frame from the peer.
    ///
    /// The replay check runs here, once, before the frame is looked at: every
    /// sequenced frame must climb above the last one accepted on this
    /// conversation, whatever its kind. (The transport enforces the same rule
    /// per session; this is the app-layer half.)
    fn handle_peer_frame(&mut self, session_id: Uuid, frame: ProtocolMessage) {
        tracing::debug!("Session {} received message: {:?}", session_id, frame);
        let chat_id = self.chat_for_session(session_id);
        let Some(chat) = self.chats.get_mut(&chat_id) else {
            tracing::error!("Chat {} not found for received message", chat_id);
            return;
        };
        if let Some(seq) = frame.seq() {
            let last = chat.recv_seq;
            if !chat.accept_seq(seq) {
                tracing::warn!(
                    "Received {:?} with invalid sequence number for chat {}. Expected > {}, got {}. Discarding.",
                    frame, chat_id, last, seq
                );
                return;
            }
        }

        match frame {
            ProtocolMessage::Text { text, seq, .. } => {
                self.receive_text(chat_id, text, chrono::Utc::now(), seq);
            }
            ProtocolMessage::TextChunk {
                message_id,
                chunk_index,
                total_chunks,
                text_part,
                timestamp,
                seq,
            } => match self.register_incoming_text_chunk(
                chat_id,
                message_id,
                chunk_index,
                total_chunks,
                text_part,
                timestamp,
            ) {
                // Chunks arrive in order on the one stream, so the final
                // chunk's seq is the one the receipt acknowledges.
                Ok(Some((text, assembled_at))) => {
                    tracing::info!(
                        "Reassembled large text message {} for chat {}",
                        message_id,
                        chat_id
                    );
                    self.receive_text(chat_id, text, assembled_at, seq);
                }
                Ok(None) => tracing::trace!(
                    "Buffered text chunk {}/{} for chat {}",
                    chunk_index + 1,
                    total_chunks,
                    chat_id
                ),
                Err(e) => {
                    tracing::warn!(
                        "Discarding large text message {} for chat {}: {}",
                        message_id,
                        chat_id,
                        e
                    );
                    self.incoming_text_messages.remove(&(chat_id, message_id));
                    self.add_toast(
                        ToastLevel::Warning,
                        "A large incoming message could not be reconstructed.".to_string(),
                    );
                }
            },
            ProtocolMessage::FileMeta { filename, size, .. } => {
                self.receive_file_offer(chat_id, &filename, size);
            }
            ProtocolMessage::FileChunk { chunk, .. } => self.receive_file_chunk(chat_id, &chunk),
            ProtocolMessage::FileEnd { seq } => self.receive_file_end(chat_id, seq),
            ProtocolMessage::FileCancel { .. } => {
                tracing::info!("Peer cancelled the file transfer for chat {}", chat_id);
                self.handle_peer_file_cancel(chat_id);
            }
            ProtocolMessage::Ping { seq } => tracing::trace!("Received ping with seq {}", seq),
            ProtocolMessage::TypingStart { .. } => {
                if let Some(chat) = self.chats.get_mut(&chat_id) {
                    chat.peer_typing = true;
                    chat.typing_since = Some(std::time::Instant::now());
                }
            }
            ProtocolMessage::TypingStop { .. } => {
                if let Some(chat) = self.chats.get_mut(&chat_id) {
                    chat.clear_typing();
                }
            }
            // Keyed by the SESSION id: that is where the wire seq was stamped.
            ProtocolMessage::Ack { acked_seq, .. } => self.mark_delivered(session_id, acked_seq),
            other => {
                tracing::warn!(
                    "Received unhandled protocol message in message loop: {:?}",
                    other
                );
            }
        }
    }

    /// Record a text message from the peer, acknowledge the frame `ack_seq`
    /// that completed it, and notify (unless the conversation is on screen).
    fn receive_text(
        &mut self,
        chat_id: Uuid,
        text: String,
        timestamp: chrono::DateTime<chrono::Utc>,
        ack_seq: u64,
    ) {
        let preview = Self::preview_text_for_notification(&text);
        let Some(chat) = self.chats.get_mut(&chat_id) else {
            return;
        };
        chat.messages
            .push(Message::received(MessageContent::Text { text }, timestamp));
        chat.clear_typing();
        self.send_ack_for_chat(chat_id, ack_seq);
        self.notify_incoming_message(chat_id, &preview);
        tracing::info!("Added received message to chat {}", chat_id);
    }

    /// The peer offered a file: register the transfer and open its spool.
    fn receive_file_offer(&mut self, chat_id: Uuid, filename: &str, size: u64) {
        tracing::info!("Received file metadata: {} ({} bytes)", filename, size);
        let transfer_id = match self.start_receiving_file(chat_id, filename, size) {
            Ok(id) => id,
            Err(e) => {
                tracing::error!("Failed to start receiving file: {}", e);
                self.add_toast(ToastLevel::Error, format!("Failed to receive file: {}", e));
                return;
            }
        };
        // The directory is ours, the name is the peer's: passing them separately
        // is what lets the receiver sanitise the name itself rather than trusting
        // the decoder to have done it.
        match IncomingFileSync::new(&self.config.download_dir, filename, size) {
            Ok(incoming) => {
                self.incoming_files.insert(transfer_id, incoming);
            }
            Err(e) => {
                tracing::error!("Failed to create incoming file: {}", e);
                // Fail the transfer, or it stays "active" with no spool behind
                // it and holds the conversation's only incoming slot forever.
                self.set_transfer_status(transfer_id, TransferStatus::Failed(e.to_string()));
                self.add_toast(ToastLevel::Error, format!("Failed to receive file: {}", e));
            }
        }
    }

    /// Spool one chunk of the active incoming transfer.
    fn receive_file_chunk(&mut self, chat_id: Uuid, chunk: &[u8]) {
        let Some(transfer_id) = self.active_incoming_transfer_id_for_chat(chat_id) else {
            return;
        };
        // An offer the user has not accepted yet still spools to disk, because
        // the sender streams without waiting for a decision. Uncapped, that
        // means a peer can put up to MAX_FILE_SIZE (10 GiB) on the disk while
        // the accept/decline prompt is still on screen. Past the cap the offer
        // is declined for them.
        if self.unaccepted_spool_exceeded(transfer_id) {
            self.auto_decline_oversized_offer(transfer_id);
            return;
        }
        if let Some(transfer) = self.active_transfers.get_mut(&transfer_id) {
            transfer.seq += 1;
        }
        tracing::debug!("Received file chunk ({} bytes)", chunk.len());
        let Some(incoming) = self.incoming_files.get_mut(&transfer_id) else {
            return;
        };
        match incoming.write_chunk(chunk) {
            Ok(()) => {
                let bytes_received = incoming.bytes_received();
                self.update_transfer_progress(transfer_id, bytes_received);
            }
            Err(e) => {
                tracing::error!("Failed to write chunk: {}", e);
                self.set_transfer_status(transfer_id, TransferStatus::Failed(e.to_string()));
                self.discard_spool(transfer_id, "write failed");
                self.add_toast(ToastLevel::Error, format!("File transfer error: {}", e));
            }
        }
    }

    /// The peer finished sending: finalize, or hold until the user decides.
    fn receive_file_end(&mut self, chat_id: Uuid, seq: u64) {
        let Some(transfer_id) = self.active_incoming_transfer_id_for_chat(chat_id) else {
            return;
        };
        let awaiting = self
            .active_transfers
            .get(&transfer_id)
            .is_some_and(|t| t.status == TransferStatus::AwaitingAcceptance);
        if awaiting {
            // Fully received but not yet accepted: hold the spooled file (and
            // the FileEnd seq, for the eventual delivery receipt) until the user
            // decides.
            tracing::info!("File fully received; holding until the user accepts");
            self.pending_file_end.insert(transfer_id, seq);
        } else {
            tracing::info!("File transfer completed");
            self.finalize_incoming_file(transfer_id, Some(seq));
        }
    }

    /// The peer's delivery receipt for the frame we sent as `acked_seq`.
    fn mark_delivered(&mut self, session_id: Uuid, acked_seq: u64) {
        let Some((chat_id, message_id)) = self.awaiting_ack.remove(&(session_id, acked_seq)) else {
            tracing::debug!(
                "Ack for unknown seq {} on session {} (already acked, or sent before restart)",
                acked_seq,
                session_id
            );
            return;
        };
        if let Some(message) = self
            .chats
            .get_mut(&chat_id)
            .and_then(|c| c.messages.iter_mut().find(|m| m.id == message_id))
        {
            message.delivered = true;
            tracing::debug!(
                "Message {} in chat {} marked delivered",
                message_id,
                chat_id
            );
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

/// A `p2p:` peer label means the relay rendezvous hole punched a direct
/// socket, so the conversation is direct even though it began via the relay.
fn is_hole_punched(peer: &str) -> bool {
    peer.starts_with("p2p:")
}
