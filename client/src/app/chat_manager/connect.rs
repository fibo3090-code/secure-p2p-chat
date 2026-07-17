//! Establishing sessions: hosting (direct and via relay), connecting (by
//! address, relay code, or contact), and TOFU fingerprint confirmation.

use super::*;

impl ChatManager {
    /// Send the user's accept/reject decision for a fingerprint verification to the session task
    pub fn confirm_fingerprint(&mut self, chat_id: Uuid, accept: bool) -> Result<()> {
        tracing::info!(chat_id = %chat_id, accept = %accept, "Confirming fingerprint");
        if let Some(tx) = self.fingerprint_confirm_senders.get(&chat_id) {
            // If the user accepted and we have a pending verification request matching this chat,
            // persist the fingerprint in the chat record before confirming the session.
            if accept {
                if let Some((fp, _peer_name, req_chat_id)) = &self.fingerprint_verification_request
                {
                    // IMPORTANT: In host mode, req_chat_id is the session ID (the host placeholder ID)
                    // but the fingerprint should be stored in the actual chat (the client's ID).
                    // However, confirm_fingerprint is called with the session ID.
                    if *req_chat_id == chat_id {
                        // Resolve the actual chat ID if this is a mapped session
                        let target_chat_id = self
                            .chat_id_mapping
                            .iter()
                            .find(|(_, &session_id)| session_id == chat_id)
                            .map(|(&incoming_id, _)| incoming_id)
                            .unwrap_or(chat_id);

                        if let Some(chat) = self.chats.get_mut(&target_chat_id) {
                            tracing::debug!(
                                "Storing verified fingerprint for chat {}",
                                target_chat_id
                            );
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

    /// Start hosting on specified port
    pub async fn start_host(&mut self, port: u16, privkey: RsaPrivateKey) -> Result<Uuid> {
        // Guard: if a placeholder host already exists, avoid spawning another.
        if let Some(existing_host) = self.chats.values().find(|c| c.is_host_placeholder) {
            // If the placeholder has no active session, clean it up so we can rehost.
            if !self.sessions.contains_key(&existing_host.id) {
                let id_to_remove = existing_host.id;
                self.chats.remove(&id_to_remove);
                self.sessions.remove(&id_to_remove);
                self.session_events.remove(&id_to_remove);
                self.fingerprint_confirm_senders.remove(&id_to_remove);
                tracing::warn!(port = %port, "Removed stale placeholder host before restarting");
            } else {
                self.add_toast(
                    ToastLevel::Info,
                    format!("Already listening on port {}", port),
                );
                tracing::info!(port = %port, "start_host skipped: active placeholder host exists");
                return Err(anyhow::anyhow!("Already listening on port {}", port));
            }
        }

        let chat_id = Uuid::new_v4();
        tracing::info!(chat_id = %chat_id, port = %port, "start_host called");

        // Create channels
        let (to_app_tx, to_app_rx) = mpsc::unbounded_channel();
        let (from_app_tx, from_app_rx) = mpsc::unbounded_channel();

        // Create confirmation channel so UI can accept/reject the fingerprint
        let (confirm_tx, confirm_rx) = mpsc::unbounded_channel();
        let connection_password = self.connection_password.clone();

        // Spawn session task
        tokio::spawn(async move {
            if let Err(e) = run_host_session(
                port,
                privkey,
                to_app_tx,
                from_app_rx,
                confirm_rx,
                chat_id,
                connection_password,
            )
            .await
            {
                tracing::error!("Host session error: {}", e);
            }
        });

        // Create chat entry
        let chat = Chat {
            id: chat_id,
            title: format!("Host on :{}", port),
            kind: ChatKind::Dm,
            transport: Transport::Direct,
            peer_fingerprint: None,
            participants: Vec::new(),
            messages: Vec::new(),
            created_at: chrono::Utc::now(),
            peer_typing: false,
            typing_since: None,
            send_seq: 0,
            recv_seq: 0,
            is_host_placeholder: true,
        };

        self.chats.insert(chat_id, chat);
        self.sessions.insert(chat_id, SessionHandle { from_app_tx });
        self.session_events
            .insert(chat_id, Arc::new(Mutex::new(to_app_rx)));
        self.fingerprint_confirm_senders.insert(chat_id, confirm_tx);

        self.is_hosting = true;
        self.add_toast(ToastLevel::Info, format!("Listening on port {}", port));
        tracing::debug!(chat_count = %self.chats.len(), session_count = %self.sessions.len(), "Host session initialized");

        // Best-effort UPnP / NAT-PMP port mapping so peers outside the LAN can
        // reach us. Runs in the background; the first result lands via
        // poll_session_events so hosting is never delayed by a slow or absent
        // gateway. The task then renews the lease until hosting stops, and
        // unmaps the port on cancellation.
        if self.config.enable_upnp && self.upnp_cancel.is_none() {
            use crate::network::nat;
            let (result_tx, result_rx) = tokio::sync::oneshot::channel();
            let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();
            self.pending_upnp = Some(result_rx);
            self.upnp_cancel = Some(cancel_tx);
            tokio::spawn(async move {
                // Initial mapping, cancellation-aware: if hosting stops while the
                // gateway call is still pending (up to 15s), abort instead of
                // creating a mapping after shutdown has begun.
                let first = tokio::select! {
                    m = nat::map_port(port) => m,
                    _ = &mut cancel_rx => return,
                };
                let protocol = first.as_ref().ok().map(|m| m.protocol);
                let _ = result_tx.send(first);
                // If the first mapping failed there's nothing to renew or clean.
                // `protocol` tracks the protocol currently in force so cleanup
                // unmaps the right one even if a renewal switches UPnP<->NAT-PMP.
                let Some(mut protocol) = protocol else { return };
                loop {
                    tokio::select! {
                        // Compose the wait + remap into one future so a cancel
                        // aborts even a slow (up-to-15s) remap in progress
                        // instead of waiting for it to finish first.
                        remapped = async {
                            tokio::time::sleep(nat::RENEW_AFTER).await;
                            // Re-map to refresh the lease; ignore transient errors,
                            // the existing mapping is still valid until it expires.
                            nat::map_port(port).await
                        } => {
                            if let Ok(m) = remapped {
                                protocol = m.protocol;
                            }
                        }
                        _ = &mut cancel_rx => {
                            nat::unmap_port(port, protocol).await;
                            return;
                        }
                    }
                }
            });
        }

        Ok(chat_id)
    }

    pub async fn start_host_via_relay(
        &mut self,
        relay_server: &str,
        token: Option<String>,
        privkey: RsaPrivateKey,
    ) -> Result<(Uuid, String)> {
        if let Some(existing_host) = self.chats.values().find(|c| c.is_host_placeholder) {
            if self.sessions.contains_key(&existing_host.id) {
                self.add_toast(
                    ToastLevel::Info,
                    "Already hosting a session. Disconnect it before starting relay hosting."
                        .to_string(),
                );
                return Err(anyhow!("Already hosting a session"));
            }
        }

        let relay_token = token.unwrap_or_else(generate_relay_token);
        let chat_id = Uuid::new_v4();
        let (to_app_tx, to_app_rx) = mpsc::unbounded_channel();
        let (from_app_tx, from_app_rx) = mpsc::unbounded_channel();
        let (confirm_tx, confirm_rx) = mpsc::unbounded_channel();
        let relay_server_owned = relay_server.to_string();
        let relay_token_owned = relay_token.clone();

        tokio::spawn(async move {
            if let Err(e) = run_host_session_via_relay(
                &relay_server_owned,
                &relay_token_owned,
                privkey,
                to_app_tx,
                from_app_rx,
                confirm_rx,
                chat_id,
            )
            .await
            {
                tracing::error!("Relay host session error: {}", e);
            }
        });

        self.chats.insert(
            chat_id,
            Chat {
                id: chat_id,
                title: format!("Relay host via {}", relay_server),
                kind: ChatKind::Dm,
                transport: Transport::Relay,
                peer_fingerprint: None,
                participants: Vec::new(),
                messages: Vec::new(),
                created_at: chrono::Utc::now(),
                peer_typing: false,
                typing_since: None,
                send_seq: 0,
                recv_seq: 0,
                is_host_placeholder: true,
            },
        );
        self.sessions.insert(chat_id, SessionHandle { from_app_tx });
        self.session_events
            .insert(chat_id, Arc::new(Mutex::new(to_app_rx)));
        self.fingerprint_confirm_senders.insert(chat_id, confirm_tx);
        self.is_hosting = true;
        self.add_toast(
            ToastLevel::Info,
            format!("Waiting for relay peer via {}", relay_server),
        );
        Ok((chat_id, relay_token))
    }

    /// Connect to a host
    pub async fn connect_to_host(
        &mut self,
        host: &str,
        port: u16,
        existing_chat_id: Option<Uuid>,
        privkey: RsaPrivateKey,
    ) -> Result<Uuid> {
        self.connect_to_host_candidates(vec![(host.to_string(), port)], existing_chat_id, privkey)
            .await
    }

    /// Connect trying each `(host, port)` candidate in priority order (e.g. a
    /// contact's internet-reachable address first, then the LAN one). The chat
    /// is labeled after the first candidate; the session runs over whichever
    /// address actually accepted the connection.
    pub async fn connect_to_host_candidates(
        &mut self,
        targets: Vec<(String, u16)>,
        existing_chat_id: Option<Uuid>,
        privkey: RsaPrivateKey,
    ) -> Result<Uuid> {
        let (host, port) = targets
            .first()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("No candidate addresses to connect to"))?;
        let host = host.as_str();
        let chat_id = existing_chat_id.unwrap_or_else(Uuid::new_v4);
        tracing::info!(chat_id = %chat_id, host = %host, port = %port, candidates = %targets.len(), "connect_to_host called");

        let (to_app_tx, to_app_rx) = mpsc::unbounded_channel();
        let (from_app_tx, from_app_rx) = mpsc::unbounded_channel();

        let (confirm_tx, confirm_rx) = mpsc::unbounded_channel();
        let connection_password = self.connection_password.clone();

        let targets_copy = targets.clone();
        tokio::spawn(async move {
            if let Err(e) = run_client_session_multi(
                &targets_copy,
                privkey,
                to_app_tx,
                from_app_rx,
                confirm_rx,
                chat_id,
                connection_password,
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
                kind: ChatKind::Dm,
                transport: Transport::Direct,
                peer_fingerprint: None,
                participants: Vec::new(),
                messages: Vec::new(),
                created_at: chrono::Utc::now(),
                peer_typing: false,
                typing_since: None,
                send_seq: 0,
                recv_seq: 0,
                is_host_placeholder: false,
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

    pub async fn connect_via_relay(
        &mut self,
        relay_server: &str,
        token: &str,
        existing_chat_id: Option<Uuid>,
        privkey: RsaPrivateKey,
    ) -> Result<Uuid> {
        let chat_id = existing_chat_id.unwrap_or_else(Uuid::new_v4);
        let (to_app_tx, to_app_rx) = mpsc::unbounded_channel();
        let (from_app_tx, from_app_rx) = mpsc::unbounded_channel();
        let (confirm_tx, confirm_rx) = mpsc::unbounded_channel();
        let relay_server_owned = relay_server.to_string();
        let relay_token_owned = token.to_string();

        tokio::spawn(async move {
            if let Err(e) = run_client_session_via_relay(
                &relay_server_owned,
                &relay_token_owned,
                privkey,
                to_app_tx,
                from_app_rx,
                confirm_rx,
                chat_id,
            )
            .await
            {
                tracing::error!("Relay client session error: {}", e);
            }
        });

        match self.chats.entry(chat_id) {
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(Chat {
                    id: chat_id,
                    title: format!("Relay via {}", relay_server),
                    kind: ChatKind::Dm,
                    transport: Transport::Relay,
                    peer_fingerprint: None,
                    participants: Vec::new(),
                    messages: Vec::new(),
                    created_at: chrono::Utc::now(),
                    peer_typing: false,
                    typing_since: None,
                    send_seq: 0,
                    recv_seq: 0,
                    is_host_placeholder: false,
                });
            }
            // Reconnecting or a chat pre-created elsewhere (e.g. the contacts
            // dialog defaults to Direct): normalize its route metadata to Relay so
            // it isn't persisted or rendered as a direct chat.
            std::collections::hash_map::Entry::Occupied(mut e) => {
                e.get_mut().transport = Transport::Relay;
            }
        }

        self.sessions.insert(chat_id, SessionHandle { from_app_tx });
        self.session_events
            .insert(chat_id, Arc::new(Mutex::new(to_app_rx)));
        self.fingerprint_confirm_senders.insert(chat_id, confirm_tx);
        self.add_toast(
            ToastLevel::Info,
            format!("Connecting via relay {}", relay_server),
        );
        Ok(chat_id)
    }

    pub async fn connect_to_contact(
        &mut self,
        contact_id: Uuid,
        existing_chat_id: Option<Uuid>,
        privkey: &RsaPrivateKey,
    ) -> Result<Uuid> {
        let contact = self
            .contacts
            .get(&contact_id)
            .ok_or_else(|| anyhow::anyhow!("Contact not found"))?
            .clone();
        // Direct-connect candidates in priority order (multi-address invites:
        // e.g. internet-reachable first, LAN second). Unparsable entries are
        // dropped rather than aborting, so a stale candidate can't block the
        // relay/fingerprint fallbacks below.
        let candidates: Vec<(String, u16)> = contact
            .candidate_addresses()
            .iter()
            .filter_map(|a| Self::parse_address(a).ok())
            .collect();
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
            // Otherwise, if the contact has direct addresses, start a connection using the mapped chat id
            if !candidates.is_empty() {
                tracing::info!(
                    "Connecting mapped chat {} via {} candidate address(es)",
                    mapped,
                    candidates.len()
                );
                let chat_id = self
                    .connect_to_host_candidates(candidates.clone(), Some(mapped), privkey.clone())
                    .await?;
                self.associate_contact_with_chat(contact_id, chat_id);
                return Ok(chat_id);
            }
            if let (Some(relay_server), Some(relay_token)) =
                (contact.relay_server.clone(), contact.relay_token.clone())
            {
                let chat_id = self
                    .connect_via_relay(&relay_server, &relay_token, Some(mapped), privkey.clone())
                    .await?;
                self.associate_contact_with_chat(contact_id, chat_id);
                return Ok(chat_id);
            }
            // No way to create a session yet; fall through to fingerprint/address logic below
        }

        tracing::debug!(
            "connect_to_contact: id={}, candidates={}, has_fp={}",
            contact_id,
            candidates.len(),
            contact.fingerprint.is_some()
        );
        if !candidates.is_empty() {
            tracing::info!(
                "Connecting to contact {} via {} candidate address(es)",
                contact_id,
                candidates.len()
            );
            let chat_id = self
                .connect_to_host_candidates(candidates, existing_chat_id, privkey.clone())
                .await?;
            self.associate_contact_with_chat(contact_id, chat_id);
            Ok(chat_id)
        } else if let (Some(relay_server), Some(relay_token)) =
            (contact.relay_server.clone(), contact.relay_token.clone())
        {
            tracing::info!(
                "Connecting to contact {} via relay {}",
                contact_id,
                relay_server
            );
            let chat_id = self
                .connect_via_relay(
                    &relay_server,
                    &relay_token,
                    existing_chat_id,
                    privkey.clone(),
                )
                .await?;
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
}
