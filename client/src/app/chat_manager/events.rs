//! Session-event pump: reacting to everything the network layer reports
//! (messages, files, TOFU, lifecycle).

use super::*;

impl ChatManager {
    /// Feed one session event straight in, as the poll would. Lets the TUI's
    /// own tests drive a held file offer without standing up a peer.
    #[cfg(test)]
    pub(crate) fn inject_session_event(&mut self, chat_id: Uuid, event: SessionEvent) {
        self.handle_session_event(chat_id, event);
    }

    /// Poll and process all pending session events
    pub fn poll_session_events(&mut self) {
        self.drain_finalized();
        self.cleanup_stale_incoming_text_messages();
        self.sync_outgoing_transfer_progress();
        self.poll_upnp_result();
        let chat_ids: Vec<Uuid> = self.session_events.keys().copied().collect();
        tracing::trace!(tracked_sessions = %chat_ids.len(), "Polling session events");

        for chat_id in chat_ids {
            let mut events = Vec::new();
            if let Some(rx_mutex) = self.session_events.get(&chat_id) {
                if let Ok(mut rx) = rx_mutex.try_lock() {
                    while let Ok(event) = rx.try_recv() {
                        events.push(event);
                    }
                }
            }

            tracing::trace!(chat_id = %chat_id, events = %events.len(), "Processing session events for chat");
            for event in events {
                self.handle_session_event(chat_id, event);
            }
        }
    }

    /// Register a sent message as awaiting the peer's receipt. Bounded: a peer
    /// predating `Ack` never acknowledges, so a full backlog is simply dropped.
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

    /// Trust-On-First-Use: let the peer proceed, ask the user, or refuse.
    pub(super) fn handle_tofu_verification(
        &mut self,
        session_id: Uuid,
        fingerprint: &str,
        peer_name: &str,
        sas: &str,
    ) {
        self.handle_tofu_verification_from(session_id, fingerprint, peer_name, sas, None);
    }

    /// [`Self::handle_tofu_verification`] for an incoming connection, which
    /// also knows where the connection came from (`peer_addr`, the listener's
    /// label for it).
    fn handle_tofu_verification_from(
        &mut self,
        session_id: Uuid,
        fingerprint: &str,
        peer_name: &str,
        sas: &str,
        peer_addr: Option<&str>,
    ) {
        // On the host the fingerprint belongs to the incoming conversation.
        let chat_id = self.chat_for_session(session_id);

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
            warning: None,
        };

        // Dialled as a contact whose invite named a key: that key is what this
        // session should present. Only a *disagreement* changes anything — a
        // match falls through to the ordinary branches below, because an
        // invite is not verification and its key still gets the SAS prompt.
        let invite_mismatch = self
            .expected_fingerprints
            .get(&session_id)
            .filter(|e| !e.fingerprint.eq_ignore_ascii_case(fingerprint))
            .map(|e| FingerprintWarning::InviteMismatch {
                contact_id: e.contact_id,
                contact_name: e.contact_name.clone(),
                expected: e.fingerprint.clone(),
            });

        // A changed key outranks an invite mismatch when both apply: the
        // conversation's own history is the stronger claim about who this is.
        let key_changed = stored
            .as_deref()
            .filter(|known| *known != fingerprint)
            .map(|known| FingerprintWarning::KeyChanged {
                previous: known.to_string(),
                repoints: match &invite_mismatch {
                    Some(FingerprintWarning::InviteMismatch { contact_name, .. }) => {
                        Some(contact_name.clone())
                    }
                    _ => None,
                },
            });
        // Listener side: the only binding there is the key itself, so a
        // contact arriving with another key read as a stranger's first
        // connection. Its source address is the one other thing known.
        let address_mismatch =
            peer_addr.and_then(|addr| self.contact_address_mismatch(addr, fingerprint, chat_id));
        // Checked before every accepting branch on purpose: neither the
        // auto-trust setting nor the key being trusted in some *other*
        // conversation may wave through a key that contradicts what this
        // conversation, or the invite the user imported, says it should be.
        if let Some(warning) = key_changed.or(invite_mismatch).or(address_mismatch) {
            let toast = match &warning {
                FingerprintWarning::KeyChanged { previous, .. } => {
                    tracing::warn!(%chat_id, stored = %previous, new = %fingerprint, "FINGERPRINT MISMATCH");
                    "SECURITY WARNING: Peer fingerprint has changed!".to_string()
                }
                FingerprintWarning::InviteMismatch {
                    contact_name,
                    expected,
                    ..
                } => {
                    tracing::warn!(
                        %chat_id,
                        expected = %expected,
                        new = %fingerprint,
                        "INVITE FINGERPRINT MISMATCH: the dialled contact's invite named another key"
                    );
                    format!(
                        "SECURITY WARNING: {} answered with a key that is not the one in their invite",
                        contact_name
                    )
                }
                FingerprintWarning::ContactAddressMismatch {
                    contact_name,
                    expected,
                    ..
                } => {
                    tracing::warn!(
                        %chat_id,
                        expected = %expected,
                        new = %fingerprint,
                        "CONTACT KEY MISMATCH: a connection from a saved contact's address presented another key"
                    );
                    format!(
                        "SECURITY WARNING: a connection from {}'s address presented a key that is not theirs",
                        contact_name
                    )
                }
            };
            self.queue_fingerprint_request(PendingFingerprint {
                warning: Some(warning),
                ..prompt
            });
            self.add_toast(ToastLevel::Warning, toast);
            return;
        }

        match stored {
            Some(_) => {
                tracing::debug!(%chat_id, "Fingerprint matches stored value; proceeding");
                self.answer_fingerprint_check(session_id, true);
            }
            // Auto-trust opted in, or a peer already verified elsewhere.
            None if self.config.auto_trust_on_first_use
                || self.fingerprint_known_trusted(fingerprint, chat_id) =>
            {
                tracing::info!(%chat_id, "Fingerprint trusted without a prompt");
                if let Some(chat) = self.chats.get_mut(&chat_id) {
                    chat.peer_fingerprint = Some(fingerprint.to_string());
                }
                self.answer_fingerprint_check(session_id, true);
                self.promote_contact_verified(chat_id, fingerprint);
            }
            // First use: the user compares the safety code out of band. Keyed by
            // session id (the answer goes to that task) and queued.
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

    /// The saved contact an incoming connection's source address belongs to,
    /// when the key it presents is not that contact's — as a warning.
    ///
    /// Deliberately narrow, because an address is a hint, not an identity:
    /// only a direct or hole-punched connection names a source (a relayed one
    /// comes from the relay); addresses are compared as IP literals (no DNS);
    /// exactly one contact may hold the address (several people behind one
    /// NAT, or test peers on loopback, prove nothing); and a key this app
    /// already trusts — in a conversation or on a contact — is never flagged,
    /// so the warning fires once per new key, not on every reconnect.
    fn contact_address_mismatch(
        &self,
        peer_addr: &str,
        fingerprint: &str,
        chat_id: Uuid,
    ) -> Option<FingerprintWarning> {
        let source = source_ip(peer_addr)?;
        // Trusted anywhere — this conversation included, which is where a
        // returning peer is routed — or held by any contact: not a stranger.
        let this_chat_trusts_it = self
            .chats
            .get(&chat_id)
            .is_some_and(|c| c.peer_fingerprint.as_deref() == Some(fingerprint));
        if this_chat_trusts_it
            || self.fingerprint_known_trusted(fingerprint, chat_id)
            || self
                .contacts
                .values()
                .any(|c| c.fingerprint.as_deref() == Some(fingerprint))
        {
            return None;
        }
        let mut at_address = self.contacts.values().filter(|c| {
            c.trust_state != TrustState::Blocked
                && c.candidate_addresses()
                    .iter()
                    .filter_map(|a| a.parse::<std::net::SocketAddr>().ok())
                    .any(|a| canonical_ip(a.ip()) == source)
        });
        let contact = at_address.next()?;
        if at_address.next().is_some() {
            return None;
        }
        let expected = contact.fingerprint.clone()?;
        Some(FingerprintWarning::ContactAddressMismatch {
            contact_id: contact.id,
            contact_name: contact.name.clone(),
            expected,
        })
    }

    /// Has this fingerprint been *confirmed* outside `chat_id` — in another
    /// conversation, or on a `Verified`/`Trusted` contact? An imported invite
    /// alone (`Unverified`) must not pre-trust it.
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
    pub(super) fn answer_fingerprint_check(&self, session_id: Uuid, accept: bool) {
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
                let relayed = peer.starts_with("relay:");
                if hole_punched {
                    self.add_toast(
                        ToastLevel::Success,
                        format!("Direct connection established (hole punched): {}", peer),
                    );
                } else {
                    self.add_toast(ToastLevel::Success, format!("Connected to {}", peer));
                }

                if let Some(chat) = self.chats.get_mut(&chat_id) {
                    // Keep a title the user chose.
                    if !chat.title_is_custom {
                        chat.title = peer;
                    }
                    chat.is_host_placeholder = false;
                    // The label says which route won; a dial that fell back
                    // from a dead direct address to the relay must not still
                    // read as Direct.
                    if hole_punched {
                        chat.transport = Transport::Direct;
                    } else if relayed {
                        chat.transport = Transport::Relay;
                    }
                    // A new session restarts the wire seq; `recv_seq` must too.
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
                self.handle_tofu_verification_from(
                    chat_id,
                    &fingerprint,
                    &peer_addr,
                    &sas,
                    Some(&peer_addr),
                );
            }

            SessionEvent::ShowFingerprintVerification {
                fingerprint,
                peer_name,
                sas,
                chat_id,
            } => {
                self.rejoin_existing_conversation(chat_id, &fingerprint);
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
                // The oldest queued text hit the wire with this seq (FIFO).
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
                // Fail receives before dropping `chat_id_mapping`, which is how a
                // host-side session id resolves to the transfer's conversation.
                self.fail_incoming_transfers(chat_id, "connection lost");

                // Incoming conversations this session opened that were never
                // verified and hold nothing: a caller that hung up before the
                // user accepted it. Kept, each throwaway key left a persisted
                // "Peer ab12…" behind forever (audit NET-05). Resolved before
                // the mapping goes — it is how they are found.
                let abandoned: Vec<Uuid> = self
                    .chat_id_mapping
                    .iter()
                    .filter(|(incoming, session)| **session == chat_id && **incoming != chat_id)
                    .map(|(incoming, _)| *incoming)
                    .filter(|incoming| {
                        self.chats.get(incoming).is_some_and(|c| {
                            c.peer_fingerprint.is_none()
                                && c.messages.is_empty()
                                && !c.title_is_custom
                                && !c.is_host_placeholder
                        }) && !self.contact_to_chat.values().any(|v| v == incoming)
                    })
                    .collect();

                self.teardown_session(chat_id);
                self.chat_id_mapping.retain(|_, v| *v != chat_id);
                for incoming in abandoned {
                    tracing::info!(%incoming, "Dropped an incoming conversation that was never verified");
                    self.chats.remove(&incoming);
                }

                // A dead listener's placeholder would make `check_rehost_needed`
                // think it is still hosting; drop it so auto-rehost rebinds.
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

    /// A host listener accepted a peer: route the session into a conversation —
    /// an idle existing one with the same (verified) fingerprint, so reconnects
    /// do not fragment history, else the client's offered chat id.
    fn accept_incoming_connection(
        &mut self,
        session_id: Uuid,
        offered_chat_id: Uuid,
        fingerprint: &str,
        peer_addr: &str,
    ) {
        let chat_id = self
            .idle_conversation_with(fingerprint, session_id)
            .unwrap_or(offered_chat_id);
        self.chat_id_mapping.insert(chat_id, session_id);
        tracing::debug!(
            "Mapped incoming chat {} to session chat {}",
            chat_id,
            session_id
        );

        // Inherit the accepting listener's transport (a relay listener makes a
        // Relay chat) unless the rendezvous hole punched a direct socket.
        let transport = if is_hole_punched(peer_addr) {
            Transport::Direct
        } else {
            self.chats
                .get(&session_id)
                .map(|c| c.transport)
                .unwrap_or(Transport::Direct)
        };

        // Create the conversation, or reset an existing one for its new session.
        // A new chat starts with no `peer_fingerprint`, so TOFU actually runs:
        // pre-filling the peer's own made the check trivially "match" and
        // auto-trusted every caller.
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

        // The listener's placeholder is consumed; auto-rehost spawns a new one.
        if self
            .chats
            .get(&session_id)
            .is_some_and(|c| c.is_host_placeholder)
        {
            tracing::debug!("Removing consumed host placeholder chat {}", session_id);
            self.chats.remove(&session_id);
        }
    }

    /// The dialing side of "a returning peer belongs in ONE conversation": once
    /// the peer is verified, a fresh empty chat whose peer already has an idle
    /// conversation is folded into it via `chat_id_mapping`, like a host-side
    /// session. TOFU then runs against that conversation's stored fingerprint.
    fn rejoin_existing_conversation(&mut self, session_id: Uuid, fingerprint: &str) {
        let Some(fresh) = self.chats.get(&session_id) else {
            return;
        };
        // A chat that already has a peer or history is not a fresh dial.
        if fresh.peer_fingerprint.is_some() || !fresh.messages.is_empty() {
            return;
        }
        let transport = fresh.transport;
        let Some(existing) = self.idle_conversation_with(fingerprint, session_id) else {
            return;
        };
        tracing::info!(%session_id, conversation = %existing, "Reconnected peer routed into its existing conversation");
        self.chats.remove(&session_id);
        self.chat_id_mapping.insert(existing, session_id);
        if let Some(chat) = self.chats.get_mut(&existing) {
            chat.transport = transport;
            chat.reset_sequences();
        }
        for chat in self.contact_to_chat.values_mut() {
            if *chat == session_id {
                *chat = existing;
            }
        }
    }

    /// One application frame from the peer. The app-layer replay check runs
    /// first: every sequenced frame must climb above the last one accepted.
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
                // Chunks arrive in order, so the last one's seq gets the receipt.
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
            // Keyed by session id: that is where the wire seq was stamped.
            ProtocolMessage::Ack { acked_seq, .. } => self.mark_delivered(session_id, acked_seq),
            other => {
                tracing::warn!(
                    "Received unhandled protocol message in message loop: {:?}",
                    other
                );
            }
        }
    }

    /// Record a peer's text, acknowledge `ack_seq`, and notify.
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
        // One file at a time per sender: a new offer abandons the previous one.
        if let Some(abandoned) = self.streaming_transfer(chat_id) {
            tracing::warn!(%abandoned, "a new file offer arrived before the previous one finished");
            self.pending_file_end.remove(&abandoned);
            self.discard_spool(abandoned, "superseded");
            let filename = self.set_transfer_status(
                abandoned,
                TransferStatus::Failed("the sender started another file".to_string()),
            );
            // The new offer purges finished transfers from the list at once,
            // so the failed row is never seen: without this the half-received
            // file just vanished from the screen.
            self.add_toast(
                ToastLevel::Warning,
                format!("File transfer interrupted (the sender started another file): {filename}"),
            );
        }
        let transfer_id = match self.start_receiving_file(chat_id, filename, size) {
            Ok(id) => id,
            Err(e) => {
                tracing::error!("Failed to start receiving file: {}", e);
                self.refuse_file_offer(chat_id, &e.to_string());
                return;
            }
        };
        // Directory and peer-supplied name are passed separately so the receiver
        // sanitises the name itself.
        match IncomingFileSync::new(&self.config.download_dir, filename, size) {
            Ok(incoming) => {
                self.incoming_files.insert(transfer_id, incoming);
            }
            Err(e) => {
                tracing::error!("Failed to create incoming file: {}", e);
                // Fail it, or it blocks the conversation's next offer forever.
                self.set_transfer_status(transfer_id, TransferStatus::Failed(e.to_string()));
                self.refuse_file_offer(chat_id, &e.to_string());
            }
        }
    }

    /// Refuse the file the peer just offered and tell the sender to stop.
    fn refuse_file_offer(&mut self, chat_id: Uuid, reason: &str) {
        self.incoming_streams.remove(&chat_id);
        self.send_control(chat_id, ProtocolMessage::FileCancel { seq: 0 });
        self.add_toast(
            ToastLevel::Error,
            format!("Failed to receive file: {}", reason),
        );
    }

    /// Spool one chunk of the active incoming transfer.
    fn receive_file_chunk(&mut self, chat_id: Uuid, chunk: &[u8]) {
        // Chunks of a refused, declined or failed offer have nowhere to go.
        let Some(transfer_id) = self.streaming_transfer(chat_id) else {
            return;
        };
        // An unaccepted offer still spools (the sender does not wait), so it is
        // capped and auto-declined past `MAX_UNACCEPTED_SPOOL_BYTES`.
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
                // Tell the sender, or it streams the rest of the file (up to
                // MAX_FILE_SIZE) into nothing and then reports it sent; and
                // close the stream, so its remaining chunks go nowhere
                // (APP-04).
                self.send_control(chat_id, ProtocolMessage::FileCancel { seq: 0 });
                self.incoming_streams.remove(&chat_id);
                self.set_transfer_status(transfer_id, TransferStatus::Failed(e.to_string()));
                self.discard_spool(transfer_id, "write failed");
                self.add_toast(ToastLevel::Error, format!("File transfer error: {}", e));
            }
        }
    }

    /// The peer finished sending: finalize, or hold until the user decides.
    fn receive_file_end(&mut self, chat_id: Uuid, seq: u64) {
        let Some(transfer_id) = self.incoming_streams.remove(&chat_id) else {
            return;
        };
        let Some(status) = self
            .active_transfers
            .get(&transfer_id)
            .map(|t| t.status.clone())
        else {
            return;
        };
        if !status.is_active() {
            return; // declined or failed while it was still streaming
        }
        if status == TransferStatus::AwaitingAcceptance {
            // Hold the spool and the FileEnd seq (for the receipt) until the
            // user decides.
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

    /// Resolve a background UPnP mapping attempt (on failure, hosting stays
    /// LAN/relay-only).
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

    /// An idle (no live session), non-placeholder conversation other than
    /// `exclude` that already holds `fingerprint`.
    fn idle_conversation_with(&self, fingerprint: &str, exclude: Uuid) -> Option<Uuid> {
        self.chats
            .iter()
            .find(|(id, c)| {
                **id != exclude
                    && c.peer_fingerprint.as_deref() == Some(fingerprint)
                    && !c.is_host_placeholder
                    && !self.is_connected(id)
            })
            .map(|(id, _)| *id)
    }
}

/// A `p2p:` peer label means the relay rendezvous hole punched a direct socket.
fn is_hole_punched(peer: &str) -> bool {
    peer.starts_with("p2p:")
}

/// The source IP a listener's peer label names: `a.b.c.d:port` for a direct
/// connection, `p2p:a.b.c.d:port` for a hole-punched one. A relayed
/// connection (`relay:<server>`) comes from the relay, so it names nobody.
fn source_ip(peer_label: &str) -> Option<std::net::IpAddr> {
    let addr = peer_label.strip_prefix("p2p:").unwrap_or(peer_label);
    let addr: std::net::SocketAddr = addr.parse().ok()?;
    Some(canonical_ip(addr.ip()))
}

/// An IPv4-mapped IPv6 address is the IPv4 address it names (a dual-stack
/// listener reports `::ffff:a.b.c.d` for an IPv4 caller).
fn canonical_ip(ip: std::net::IpAddr) -> std::net::IpAddr {
    match ip {
        std::net::IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, std::net::IpAddr::V4),
        v4 => v4,
    }
}
