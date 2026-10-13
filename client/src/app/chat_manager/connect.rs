//! Establishing sessions: hosting (direct and via relay), connecting (by
//! address, relay code, or contact), and TOFU fingerprint confirmation.

use super::*;
use std::future::Future;

/// The session task's ends of the channels a session is driven through. The
/// manager keeps the other ends (see [`ChatManager::spawn_session`]).
pub(super) struct SessionChannels {
    pub to_app_tx: mpsc::UnboundedSender<SessionEvent>,
    pub from_app_rx: mpsc::UnboundedReceiver<ProtocolMessage>,
    pub file_rx: mpsc::Receiver<ProtocolMessage>,
    pub confirm_rx: mpsc::UnboundedReceiver<bool>,
}

impl ChatManager {
    /// Send the user's accept/reject decision for a fingerprint verification to
    /// the session task. `chat_id` is the SESSION id the prompt carried.
    pub fn confirm_fingerprint(&mut self, chat_id: Uuid, accept: bool) -> Result<()> {
        tracing::info!(chat_id = %chat_id, accept = %accept, "Confirming fingerprint");
        let Some(tx) = self.fingerprint_confirm_senders.get(&chat_id).cloned() else {
            tracing::error!("No confirmation channel for chat {}", chat_id);
            bail!("No confirmation channel for chat {}", chat_id);
        };

        // Take *this session's* prompt out of the queue — matching by session
        // id, not "whatever is pending", so answering one peer never resolves or
        // discards another's. A rejected prompt is removed too: that session is
        // about to die and its entry must not block the queue behind it.
        let pending = self.take_fingerprint_request(chat_id);

        // On accept, persist the fingerprint before confirming the session.
        if let Some(fp) = pending.filter(|_| accept).map(|p| p.fingerprint) {
            // In host mode the session id is the placeholder's, while the
            // fingerprint belongs on the incoming chat the peer created.
            let target_chat_id = self.chat_for_session(chat_id);
            if let Some(chat) = self.chats.get_mut(&target_chat_id) {
                tracing::debug!("Storing verified fingerprint for chat {}", target_chat_id);
                chat.peer_fingerprint = Some(fp.clone());
            }
            // A user-confirmed fingerprint also verifies the matching contact.
            self.promote_contact_verified(target_chat_id, &fp);
        }

        tx.send(accept)
            .map_err(|e| anyhow!("Failed to send confirmation: {}", e))
    }

    /// The user refused a peer's fingerprint: end the session, and discard the
    /// conversation only if this connection created it.
    ///
    /// A first contact's conversation is empty and unverified, and nothing
    /// should be left of it. But the same prompt appears when a *known*
    /// conversation's key changes — the possible-impostor case — and there the
    /// conversation and its history are the user's: refusing whoever answered
    /// must not delete them. (The TUI used to delete unconditionally; the
    /// desktop never did, leaving an empty dead conversation behind.)
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

    /// Wire up a session and spawn the task that runs it.
    ///
    /// `run` receives the task's ends of the channels and returns the session
    /// future. Everything else — the manager's ends, the task handle, and what
    /// happens when the session fails — is the same for every kind of session,
    /// so it lives here once.
    ///
    /// The chat and session handle are registered before the session even
    /// dials, so a failure that was only logged left a conversation reporting
    /// itself connected forever, any TOFU prompt it queued stuck at the head of
    /// the queue, and — for a listener that never accepted anyone — a
    /// placeholder that made `check_rehost_needed()` report "already hosting"
    /// for good. Every failure is therefore reported as `Error` (prefixed with
    /// `failure`) followed by `Disconnected`, which is the event that clears
    /// all of it. The message loop sends its own `Disconnected` on the normal
    /// path; this only fires when it never got that far.
    pub(super) fn spawn_session<F, Fut>(&mut self, chat_id: Uuid, failure: &'static str, run: F)
    where
        F: FnOnce(SessionChannels) -> Fut,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let (to_app_tx, to_app_rx) = mpsc::unbounded_channel();
        let (from_app_tx, from_app_rx) = mpsc::unbounded_channel();
        // Bounded lane for outgoing bulk file data (backpressure).
        let (file_tx, file_rx) = mpsc::channel(FILE_LANE_CAPACITY);
        // So the UI can accept/reject the peer's fingerprint.
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

    /// Before starting a listener: drop a placeholder whose session is already
    /// gone, and report one that is still live.
    ///
    /// A stale placeholder must be removed together with its task, or a
    /// listener still parked in `accept()` keeps the port and the next bind
    /// fails with EADDRINUSE.
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

        // Bind BEFORE any state is created. A bind that failed inside the
        // spawned task was only logged, while the chat and session handle below
        // had already been inserted — leaving a "Host on :port" conversation
        // that `is_connected()` reported as Connected, `is_hosting` stuck true,
        // and `my_addresses` advertising a port nothing was listening on.
        let listener = match crate::network::bind_host_listener(port).await {
            Ok(listener) => listener,
            Err(e) => {
                tracing::error!(port = %port, error = %e, "Could not bind host listener");
                // Auto-rehost is a timer. Retrying a port another process owns,
                // forever, logs every tick and tells the user nothing — so after
                // a few attempts stop claiming to host and say why, once.
                self.rehost_failures = self.rehost_failures.saturating_add(1);
                if self.rehost_failures >= MAX_CONSECUTIVE_REHOST_FAILURES {
                    self.stop_hosting();
                    self.add_toast(ToastLevel::Error, format!("Stopped hosting — {}", e));
                }
                return Err(e);
            }
        };
        self.rehost_failures = 0;
        // The port actually bound, which is not the requested one when the
        // caller asked for 0 (OS-assigned). Everything downstream — the chat
        // title, "share this address", the UPnP mapping, and the port
        // auto-rehost rebinds — must use this, or the app advertises `:0` and
        // every rehost lands somewhere new.
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
        let mut chat = Chat::new(
            chat_id,
            format!("Host on :{}", port),
            ChatKind::Dm,
            Transport::Direct,
        );
        chat.is_host_placeholder = true;
        self.chats.insert(chat_id, chat);

        self.is_hosting = true;
        self.hosting_port = Some(port);
        self.add_toast(ToastLevel::Info, format!("Listening on port {}", port));
        tracing::debug!(chat_count = %self.chats.len(), session_count = %self.sessions.len(), "Host session initialized");

        if self.config.enable_upnp && self.upnp_cancel.is_none() {
            self.start_port_mapping(port);
        }
        Ok(chat_id)
    }

    /// Best-effort UPnP / NAT-PMP port mapping so peers outside the LAN can
    /// reach us. Runs in the background; the first result lands via
    /// `poll_session_events` so hosting is never delayed by a slow or absent
    /// gateway. The task then renews the lease until hosting stops (dropping
    /// `upnp_cancel`), and unmaps the port on the way out.
    fn start_port_mapping(&mut self, port: u16) {
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
                    // aborts even a slow (up-to-15s) remap in progress instead
                    // of waiting for it to finish first.
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
            )
            .await
        });
        let mut chat = Chat::new(
            chat_id,
            format!("Relay host via {}", relay_server),
            ChatKind::Dm,
            Transport::Relay,
        );
        chat.is_host_placeholder = true;
        self.chats.insert(chat_id, chat);

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
            .ok_or_else(|| anyhow!("No candidate addresses to connect to"))?;
        let chat_id = existing_chat_id.unwrap_or_else(Uuid::new_v4);
        tracing::info!(chat_id = %chat_id, host = %host, port = %port, candidates = %targets.len(), "connect_to_host called");

        let password = self.connection_password.clone();
        self.spawn_session(chat_id, "Connection failed", move |ch| async move {
            run_client_session_multi(
                &targets,
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

    /// The conversation an outgoing session is about to serve: created on
    /// first contact, otherwise brought in line with the new session.
    ///
    /// On a reconnect (or a chat pre-created elsewhere, e.g. by the contacts
    /// dialog) the route is normalized to the one actually being dialled, so a
    /// conversation is never persisted or rendered with a transport it no
    /// longer uses. Its sequence counters are reset because the new session's
    /// wire sequence restarts at 1, and the previous session's high-water mark
    /// would otherwise reject the peer's first messages as replays.
    fn prepare_outgoing_chat(&mut self, chat_id: Uuid, title: String, transport: Transport) {
        let chat = self
            .chats
            .entry(chat_id)
            .or_insert_with(|| Chat::new(chat_id, title, ChatKind::Dm, transport));
        chat.transport = transport;
        chat.reset_sequences();
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

    /// Connect to a saved contact, reusing whatever already reaches them.
    ///
    /// In order: a live session on the contact's own conversation; a live
    /// session with the same peer (matched by fingerprint) in another one;
    /// otherwise dial — direct addresses first, then the relay — into the
    /// conversation this contact already has, so history never fragments.
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

        // Reconnecting must land in the conversation this contact already has.
        // `contact_to_chat` is the fast path, but it is only populated once a
        // connection has been associated — a contact imported from an invite,
        // or one whose association was never recorded, would otherwise start a
        // brand-new chat every time and fragment the history. The contact's
        // fingerprint identifies the same peer across chats, which is how
        // returning peers are recognised everywhere else in the manager.
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

        // Direct-connect candidates in priority order (multi-address invites:
        // e.g. internet-reachable first, LAN second). Unparsable entries are
        // dropped rather than aborting, so a stale candidate can't block the
        // relay fallback.
        let candidates: Vec<(String, u16)> = contact
            .candidate_addresses()
            .iter()
            .filter_map(|a| Self::parse_address(a).ok())
            .collect();

        let chat_id = if !candidates.is_empty() {
            tracing::info!(%contact_id, candidates = candidates.len(), "Connecting to contact by address");
            self.connect_to_host_candidates(candidates, target, privkey.clone())
                .await?
        } else if let (Some(server), Some(token)) = (&contact.relay_server, &contact.relay_token) {
            tracing::info!(%contact_id, relay = %server, "Connecting to contact via relay");
            self.connect_via_relay(server, token, target, privkey.clone())
                .await?
        } else {
            tracing::error!(%contact_id, "Contact has no address and no active session");
            // Do not point at a contact editor: there isn't one. Telling
            // someone to use a feature that does not exist is worse than
            // telling them nothing.
            bail!(
                "This contact has no address or relay token to dial. Ask them for a fresh invite link, or have them connect to you first."
            );
        };
        self.associate_contact_with_chat(contact_id, chat_id);
        Ok(chat_id)
    }
}
