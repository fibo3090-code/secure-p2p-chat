//! Establishing sessions: hosting (direct and via relay), connecting (by
//! address, relay code, or contact), and TOFU fingerprint confirmation.

use super::*;
use std::future::Future;

/// The session task's ends of the channels a session is driven through.
pub(super) struct SessionChannels {
    pub to_app_tx: mpsc::UnboundedSender<SessionEvent>,
    pub from_app_rx: mpsc::UnboundedReceiver<ProtocolMessage>,
    pub file_rx: mpsc::Receiver<ProtocolMessage>,
    pub confirm_rx: mpsc::UnboundedReceiver<bool>,
}

impl ChatManager {
    /// Send the user's accept/reject decision to the session task. `chat_id` is
    /// the SESSION id the prompt carried; accepting persists the fingerprint.
    pub fn confirm_fingerprint(&mut self, chat_id: Uuid, accept: bool) -> Result<()> {
        tracing::info!(chat_id = %chat_id, accept = %accept, "Confirming fingerprint");
        let Some(tx) = self.fingerprint_confirm_senders.get(&chat_id).cloned() else {
            tracing::error!("No confirmation channel for chat {}", chat_id);
            bail!("No confirmation channel for chat {}", chat_id);
        };

        // Remove *this session's* prompt (rejected ones too: that session dies).
        let pending = self.take_fingerprint_request(chat_id);

        // On accept, persist the fingerprint before confirming the session.
        if let Some(PendingFingerprint {
            fingerprint: fp,
            warning,
            ..
        }) = pending.filter(|_| accept)
        {
            // In host mode the session id is the placeholder's, while the
            // fingerprint belongs on the incoming chat the peer created.
            let target_chat_id = self.chat_for_session(chat_id);
            if let Some(chat) = self.chats.get_mut(&target_chat_id) {
                tracing::debug!("Storing verified fingerprint for chat {}", target_chat_id);
                chat.peer_fingerprint = Some(fp.clone());
            }
            // The user was told this is not the key in the contact's invite,
            // compared the safety code, and accepted it anyway. The contact
            // follows that decision: left on the invite's key, blocking it
            // later would block a key that never connects, and every
            // reconnect would warn about the key just accepted. Only ever on
            // an explicit accept *of a warning* — never silently.
            //
            // When the conversation's own key changed too, the prompt led with
            // `KeyChanged` (the stronger claim) and the invite mismatch was
            // not named in it — but the accepted key contradicts the invite
            // all the same, so the session's expected key decides here, not
            // just the warning that happened to be shown.
            let rebind = match &warning {
                Some(FingerprintWarning::InviteMismatch { contact_id, .. }) => Some(*contact_id),
                Some(FingerprintWarning::KeyChanged { .. }) => self
                    .expected_fingerprints
                    .get(&chat_id)
                    .filter(|e| !e.fingerprint.eq_ignore_ascii_case(&fp))
                    .map(|e| e.contact_id),
                // An address is a hint, not an identity claim: the contact
                // keeps its key, and this session keeps its own conversation.
                Some(FingerprintWarning::ContactAddressMismatch { .. }) | None => None,
            };
            if let Some(contact_id) = rebind {
                self.rebind_contact_fingerprint(contact_id, &fp);
            }
            // A user-confirmed fingerprint also verifies the matching contact.
            self.promote_contact_verified(target_chat_id, &fp);
        }

        tx.send(accept)
            .map_err(|e| anyhow!("Failed to send confirmation: {}", e))
    }

    /// The user refused a peer's fingerprint: end the session, and delete the
    /// conversation only if this connection created it (empty, unverified). A
    /// known conversation whose key changed keeps its history.
    pub fn reject_fingerprint(&mut self, session_id: Uuid) -> Result<()> {
        let conversation = self.resolve_display_chat_id(session_id);
        let created_by_this_connection = self
            .chats
            .get(&conversation)
            .is_some_and(|c| c.peer_fingerprint.is_none() && c.messages.is_empty());
        let answered = self.confirm_fingerprint(session_id, false);
        if created_by_this_connection {
            self.delete_chat(session_id);
        }
        answered
    }

    /// Wire up a session and spawn the task that runs it (`run` gets the task's
    /// channel ends). State is registered before the session dials, so any
    /// failure is reported as `Error` (prefixed with `failure`) then
    /// `Disconnected`, which is what clears that state.
    pub(super) fn spawn_session<F, Fut>(&mut self, chat_id: Uuid, failure: &'static str, run: F)
    where
        F: FnOnce(SessionChannels) -> Fut,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let (to_app_tx, to_app_rx) = mpsc::unbounded_channel();
        let (from_app_tx, from_app_rx) = mpsc::unbounded_channel();
        let (file_tx, file_rx) = mpsc::channel(FILE_LANE_CAPACITY);
        let (confirm_tx, confirm_rx) = mpsc::unbounded_channel();

        let failure_tx = to_app_tx.clone();
        let session = run(SessionChannels {
            to_app_tx,
            from_app_rx,
            file_rx,
            confirm_rx,
        });
        let task = tokio::spawn(async move {
            if let Err(e) = session.await {
                tracing::error!("{}: {}", failure, e);
                let _ = failure_tx.send(SessionEvent::Error(format!("{}: {}", failure, e)));
                let _ = failure_tx.send(SessionEvent::Disconnected);
            }
        });

        self.sessions.insert(
            chat_id,
            SessionHandle {
                from_app_tx,
                file_tx,
            },
        );
        self.register_session_task(chat_id, task);
        self.session_events
            .insert(chat_id, Arc::new(Mutex::new(to_app_rx)));
        self.fingerprint_confirm_senders.insert(chat_id, confirm_tx);
    }

    /// Before starting a listener: drop a placeholder whose session is gone
    /// (with its task, or it keeps the port bound), and report a live one.
    fn live_host_placeholder(&mut self) -> Option<Uuid> {
        let id = self.host_placeholder_id()?;
        if self.sessions.contains_key(&id) {
            return Some(id);
        }
        tracing::warn!(%id, "Removed stale placeholder host before restarting");
        self.remove_host_placeholder(id);
        None
    }

    /// Start hosting on specified port
    pub async fn start_host(&mut self, port: u16, privkey: RsaPrivateKey) -> Result<Uuid> {
        if self.live_host_placeholder().is_some() {
            self.add_toast(
                ToastLevel::Info,
                format!("Already listening on port {}", port),
            );
            tracing::info!(port = %port, "start_host skipped: active placeholder host exists");
            bail!("Already listening on port {}", port);
        }

        let chat_id = Uuid::new_v4();
        tracing::info!(chat_id = %chat_id, port = %port, "start_host called");

        // Bind BEFORE creating any state, so a failed bind cannot leave a
        // phantom "Host on :port" conversation that reports itself connected.
        let listener = match crate::network::bind_host_listener(port).await {
            Ok(listener) => listener,
            Err(e) => {
                tracing::error!(port = %port, error = %e, "Could not bind host listener");
                // Stop claiming to host after repeated failures (auto-rehost
                // is a timer).
                self.rehost_failures = self.rehost_failures.saturating_add(1);
                if self.rehost_failures >= MAX_CONSECUTIVE_REHOST_FAILURES {
                    self.stop_hosting();
                    self.add_toast(ToastLevel::Error, format!("Stopped hosting — {}", e));
                }
                return Err(e);
            }
        };
        self.rehost_failures = 0;
        // The port actually bound (the request may be 0), which everything
        // downstream must advertise and rebind.
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(port);

        let password = self.connection_password.clone();
        self.spawn_session(chat_id, "Hosting stopped", move |ch| {
            run_host_session(
                listener,
                privkey,
                ch.to_app_tx,
                ch.from_app_rx,
                ch.file_rx,
                ch.confirm_rx,
                chat_id,
                password,
            )
        });
        self.insert_host_placeholder(chat_id, format!("Host on :{}", port), Transport::Direct);

        self.is_hosting = true;
        self.hosting_port = Some(port);
        self.add_toast(ToastLevel::Info, format!("Listening on port {}", port));
        tracing::debug!(chat_count = %self.chats.len(), session_count = %self.sessions.len(), "Host session initialized");

        if self.config.enable_upnp && self.upnp_cancel.is_none() {
            self.start_port_mapping(port);
        }
        Ok(chat_id)
    }

    /// Best-effort UPnP / NAT-PMP mapping in the background (the result lands
    /// via `poll_session_events`). Renews the lease until `upnp_cancel` is
    /// dropped, then unmaps.
    fn start_port_mapping(&mut self, port: u16) {
        use crate::network::nat;
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();
        self.pending_upnp = Some(result_rx);
        self.upnp_cancel = Some(cancel_tx);
        tokio::spawn(async move {
            // Cancellation-aware: no mapping is created after hosting stopped.
            let first = tokio::select! {
                m = nat::map_port(port) => m,
                _ = &mut cancel_rx => return,
            };
            let protocol = first.as_ref().ok().map(|m| m.protocol);
            let _ = result_tx.send(first);
            // `protocol` tracks the one in force, since a renewal may switch.
            let Some(mut protocol) = protocol else { return };
            loop {
                tokio::select! {
                    // One future, so a cancel aborts even a slow remap.
                    remapped = async {
                        tokio::time::sleep(nat::RENEW_AFTER).await;
                        // Transient errors are ignored: the lease is still valid.
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

    pub async fn start_host_via_relay(
        &mut self,
        relay_server: &str,
        token: Option<String>,
        privkey: RsaPrivateKey,
    ) -> Result<(Uuid, String)> {
        if self.live_host_placeholder().is_some() {
            self.add_toast(
                ToastLevel::Info,
                "Already hosting a session. Disconnect it before starting relay hosting."
                    .to_string(),
            );
            bail!("Already hosting a session");
        }

        let relay_token = token.unwrap_or_else(generate_relay_token);
        let chat_id = Uuid::new_v4();
        let (server, session_token) = (relay_server.to_string(), relay_token.clone());
        let password = self.connection_password.clone();
        self.spawn_session(chat_id, "Relay hosting stopped", move |ch| async move {
            run_host_session_via_relay(
                &server,
                &session_token,
                privkey,
                ch.to_app_tx,
                ch.from_app_rx,
                ch.file_rx,
                ch.confirm_rx,
                chat_id,
                password,
            )
            .await
        });
        self.insert_host_placeholder(
            chat_id,
            format!("Relay host via {}", relay_server),
            Transport::Relay,
        );

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

    /// Connect trying each `(host, port)` candidate in priority order; the chat
    /// is labeled after the first.
    pub async fn connect_to_host_candidates(
        &mut self,
        targets: Vec<(String, u16)>,
        existing_chat_id: Option<Uuid>,
        privkey: RsaPrivateKey,
    ) -> Result<Uuid> {
        self.dial_candidates(targets, None, existing_chat_id, privkey)
            .await
    }

    /// Dial the direct candidates in order, falling back to `relay`
    /// (`(server, token)`) only when none of them accepts a connection.
    async fn dial_candidates(
        &mut self,
        targets: Vec<(String, u16)>,
        relay: Option<(String, String)>,
        existing_chat_id: Option<Uuid>,
        privkey: RsaPrivateKey,
    ) -> Result<Uuid> {
        let (host, port) = targets
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("No candidate addresses to connect to"))?;
        let chat_id = existing_chat_id.unwrap_or_else(Uuid::new_v4);
        tracing::info!(chat_id = %chat_id, host = %host, port = %port, candidates = %targets.len(), "connect_to_host called");

        let password = self.dial_password.clone();
        self.spawn_session(chat_id, "Connection failed", move |ch| async move {
            run_client_session_direct_or_relay(
                &targets,
                relay,
                privkey,
                ch.to_app_tx,
                ch.from_app_rx,
                ch.file_rx,
                ch.confirm_rx,
                chat_id,
                password,
            )
            .await
        });
        self.prepare_outgoing_chat(chat_id, format!("{}:{}", host, port), Transport::Direct);
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
        let (server, token) = (relay_server.to_string(), token.to_string());
        let password = self.dial_password.clone();
        self.spawn_session(chat_id, "Relay connection failed", move |ch| async move {
            run_client_session_via_relay(
                &server,
                &token,
                privkey,
                ch.to_app_tx,
                ch.from_app_rx,
                ch.file_rx,
                ch.confirm_rx,
                chat_id,
                password,
            )
            .await
        });
        self.prepare_outgoing_chat(
            chat_id,
            format!("Relay via {}", relay_server),
            Transport::Relay,
        );
        self.add_toast(
            ToastLevel::Info,
            format!("Connecting via relay {}", relay_server),
        );
        Ok(chat_id)
    }

    /// The conversation an outgoing session will serve: created on first
    /// contact, otherwise given the dialled transport and reset sequences.
    fn prepare_outgoing_chat(&mut self, chat_id: Uuid, title: String, transport: Transport) {
        let chat = self
            .chats
            .entry(chat_id)
            .or_insert_with(|| Chat::new(chat_id, title, ChatKind::Dm, transport));
        chat.transport = transport;
        chat.reset_sequences();
    }

    fn insert_host_placeholder(&mut self, chat_id: Uuid, title: String, transport: Transport) {
        let mut chat = Chat::new(chat_id, title, ChatKind::Dm, transport);
        chat.is_host_placeholder = true;
        self.chats.insert(chat_id, chat);
    }

    /// A conversation with a live session to the peer holding `fingerprint`.
    fn live_chat_with_fingerprint(&self, fingerprint: &str) -> Option<Uuid> {
        self.chats
            .iter()
            .find(|(id, c)| {
                c.peer_fingerprint.as_deref() == Some(fingerprint) && self.sessions.contains_key(id)
            })
            .map(|(id, _)| *id)
    }

    /// Connect to a saved contact, reusing whatever already reaches them: a live
    /// session on its conversation, or with the same fingerprint elsewhere;
    /// otherwise dial (addresses, then relay) into its existing conversation.
    pub async fn connect_to_contact(
        &mut self,
        contact_id: Uuid,
        existing_chat_id: Option<Uuid>,
        privkey: &RsaPrivateKey,
    ) -> Result<Uuid> {
        let contact = self
            .contacts
            .get(&contact_id)
            .ok_or_else(|| anyhow!("Contact not found"))?
            .clone();
        if contact.trust_state == TrustState::Blocked {
            bail!(
                "{} is blocked — unblock the contact to reconnect",
                contact.name
            );
        }

        let mapped = self.contact_to_chat.get(&contact_id).copied();
        if let Some(mapped) = mapped.filter(|id| self.sessions.contains_key(id)) {
            tracing::debug!(%contact_id, chat = %mapped, "connect_to_contact: already connected");
            return Ok(mapped);
        }
        // Dialing again would replace a session that is working.
        if let Some(active) = contact
            .fingerprint
            .as_deref()
            .and_then(|fp| self.live_chat_with_fingerprint(fp))
        {
            tracing::info!(%contact_id, chat = %active, "Re-associating contact to its active chat by fingerprint");
            self.associate_contact_with_chat(contact_id, active);
            return Ok(active);
        }

        // Land in the contact's existing conversation: the mapping, else one
        // matched by fingerprint (an imported contact has no mapping yet).
        let target = mapped.or(existing_chat_id).or_else(|| {
            let fp = contact.fingerprint.as_deref()?;
            let found = self
                .chats
                .iter()
                .find(|(_, c)| c.peer_fingerprint.as_deref() == Some(fp) && !c.is_host_placeholder)
                .map(|(id, _)| *id)?;
            tracing::info!(
                contact = %contact_id,
                chat = %found,
                "reusing this contact's existing conversation (matched by fingerprint)"
            );
            Some(found)
        });

        // Unparsable candidates are dropped so they cannot block the relay.
        let candidates: Vec<(String, u16)> = contact
            .candidate_addresses()
            .iter()
            .filter_map(|a| Self::parse_address(a).ok())
            .collect();

        let relay = contact
            .relay_server
            .clone()
            .zip(contact.relay_token.clone());
        let chat_id = if !candidates.is_empty() {
            tracing::info!(%contact_id, candidates = candidates.len(), relay = relay.is_some(), "Connecting to contact by address");
            // The relay rides along as the fallback: a saved address goes
            // stale when the peer changes networks (NET-12).
            self.dial_candidates(candidates, relay, target, privkey.clone())
                .await?
        } else if let (Some(server), Some(token)) = (&contact.relay_server, &contact.relay_token) {
            tracing::info!(%contact_id, relay = %server, "Connecting to contact via relay");
            self.connect_via_relay(server, token, target, privkey.clone())
                .await?
        } else {
            tracing::error!(%contact_id, "Contact has no address and no active session");
            bail!(
                "This contact has no address or relay token to dial. Ask them for a fresh invite link, or have them connect to you first."
            );
        };
        // The invite's fingerprint becomes what this session is *expected* to
        // present (CRYPTO-04). It is never trusted on that basis — an invite is
        // not verification — but a different key answering takes the warning
        // path instead of passing for an ordinary first use. Recording it after
        // the dial is safe: session events are only handled through `&mut
        // self`, which this call holds until it returns.
        if let Some(fingerprint) = contact.fingerprint.clone() {
            self.expected_fingerprints.insert(
                chat_id,
                ExpectedPeer {
                    contact_id,
                    contact_name: contact.name.clone(),
                    fingerprint,
                },
            );
        }
        self.associate_contact_with_chat(contact_id, chat_id);
        Ok(chat_id)
    }
}
