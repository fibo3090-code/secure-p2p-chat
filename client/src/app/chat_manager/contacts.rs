//! Contacts: adding/importing, chat association, trust lifecycle
//! (verify/block), and auto-reconnect on startup.

use super::*;

impl ChatManager {
    /// Parse an address of the form host:port
    /// Returns (host, port) or an error if the format is invalid.
    pub fn parse_address(address: &str) -> Result<(String, u16)> {
        crate::util::parse_host_port(address, None).map_err(|e| {
            tracing::error!("Invalid address format for contact '{}': {}", address, e);
            anyhow::anyhow!("Invalid contact address: {}", e)
        })
    }

    /// Add a contact
    pub fn add_contact(
        &mut self,
        name: String,
        address: Option<String>,
        fingerprint: Option<String>,
        public_key: Option<String>,
    ) -> Uuid {
        let id = Uuid::new_v4();
        tracing::info!(id = %id, name = %name, has_address = %address.is_some(), has_fp = %fingerprint.is_some(), "Adding contact");
        let contact = Contact {
            id,
            name,
            address,
            addresses: Vec::new(),
            relay_server: None,
            relay_token: None,
            fingerprint,
            public_key,
            created_at: chrono::Utc::now(),
            trust_state: TrustState::Unverified,
            notes: String::new(),
            tags: Vec::new(),
            last_seen: None,
        };
        self.contacts.insert(id, contact);
        // no chat association by default
        tracing::debug!(id = %id, total_contacts = %self.contacts.len(), "Contact added");
        id
    }

    /// Save a contact parsed from an invite link.
    ///
    /// Importing the same link twice used to add a second identical card, and
    /// importing your *own* link added you to your own contacts. Both are
    /// resolved by identity, not by name: an invite for a fingerprint we already
    /// have refreshes that contact instead of duplicating it.
    pub fn import_contact(&mut self, mut contact: Contact) -> Result<Uuid> {
        if let Some(fp) = contact.fingerprint.as_deref() {
            if self
                .my_fingerprint
                .as_deref()
                .is_some_and(|mine| mine.eq_ignore_ascii_case(fp))
            {
                anyhow::bail!("That invite link is your own — share it with someone else instead.");
            }
            if let Some(existing) = self
                .contacts
                .values()
                .find(|c| c.fingerprint.as_deref() == Some(fp))
                .map(|c| c.id)
            {
                // Same peer, possibly a fresher address or relay token. Update
                // the reachability details and keep everything trust-related.
                let name = contact.name.clone();
                if let Some(current) = self.contacts.get_mut(&existing) {
                    current.name = name;
                    current.address = contact.address;
                    current.addresses = contact.addresses;
                    current.relay_server = contact.relay_server;
                    current.relay_token = contact.relay_token;
                }
                tracing::info!(contact = %existing, "invite refreshed an existing contact");
                return Ok(existing);
            }
        }
        let id = Uuid::new_v4();
        contact.id = id;
        contact.created_at = chrono::Utc::now();
        self.contacts.insert(id, contact);
        Ok(id)
    }

    /// Remove a contact **and the trust that came with it**.
    ///
    /// Deleting used to drop the contact record only, leaving the fingerprint on
    /// any chat that had verified it — so the peer still connected with no
    /// prompt, and a *blocked* contact silently became unblocked because the
    /// block lived nowhere else. Both of those made the confirmation dialog's
    /// promise ("you would have to compare the safety code again") untrue.
    /// Clearing the stored fingerprint restores it: the next connection from
    /// that peer is a first contact again.
    pub fn remove_contact(&mut self, contact_id: Uuid) {
        tracing::info!(contact_id = %contact_id, "Removing contact");
        let removed = self.contacts.remove(&contact_id);
        self.contact_to_chat.remove(&contact_id);

        if let Some(fp) = removed.as_ref().and_then(|c| c.fingerprint.clone()) {
            // Only if no *other* contact still vouches for this fingerprint.
            let still_known = self
                .contacts
                .values()
                .any(|c| c.fingerprint.as_deref() == Some(fp.as_str()));
            if !still_known {
                for chat in self.chats.values_mut() {
                    if chat.peer_fingerprint.as_deref() == Some(fp.as_str()) {
                        chat.peer_fingerprint = None;
                    }
                }
                tracing::info!(
                    "cleared the stored fingerprint of the deleted contact; \
                     their next connection will need verification again"
                );
            }
        }
        tracing::debug!(remaining_contacts = %self.contacts.len(), "Contact removed");
    }

    /// Whether deleting this contact would also lift a block on that peer, so
    /// the UI can say so before the user commits to it.
    pub fn deleting_contact_would_unblock(&self, contact_id: Uuid) -> bool {
        self.contacts
            .get(&contact_id)
            .is_some_and(|c| c.trust_state == TrustState::Blocked)
    }

    /// Get a contact
    pub fn get_contact(&self, contact_id: Uuid) -> Option<&Contact> {
        self.contacts.get(&contact_id)
    }

    /// Block a contact: refuse its future connections (TOFU auto-rejects its
    /// fingerprint) and drop any live session with it. History is kept.
    pub fn block_contact(&mut self, contact_id: Uuid) -> Result<()> {
        let contact = self
            .contacts
            .get_mut(&contact_id)
            .ok_or_else(|| anyhow::anyhow!("No such contact"))?;
        contact.trust_state = TrustState::Blocked;
        let name = contact.name.clone();
        let fingerprint = contact.fingerprint.clone();
        tracing::info!(%contact_id, name = %name, "Contact blocked");

        // Disconnect every live session with that fingerprint right away.
        // Tearing down ONLY the SessionHandle is not enough: the network task
        // would keep running and its events would keep being polled, so the
        // blocked peer could still deliver messages on an established session.
        // Dropping the event receiver too makes the task's next send fail,
        // which ends its loop and closes the socket.
        if let Some(fp) = fingerprint {
            let session_ids: Vec<Uuid> = self
                .chats
                .values()
                .filter(|c| c.peer_fingerprint.as_deref() == Some(fp.as_str()))
                .map(|c| *self.chat_id_mapping.get(&c.id).unwrap_or(&c.id))
                .collect();
            for sid in session_ids {
                // An in-flight receive from this peer must be failed *before*
                // the mapping is dropped — that mapping is how a session id
                // resolves back to its conversation. Otherwise blocking someone
                // mid-transfer leaves a progress row that never finishes and an
                // orphaned temp spool, exactly as a mid-receive disconnect used
                // to.
                self.fail_incoming_transfers(sid, "blocked this contact");
                let had_session = self.sessions.remove(&sid).is_some();
                self.session_events.remove(&sid);
                self.fingerprint_confirm_senders.remove(&sid);
                self.drop_fingerprint_requests_for_session(sid);
                self.chat_id_mapping.retain(|_, v| *v != sid);
                // Dropping the handle and the event receiver is not enough on
                // its own: a task parked on a socket read has nothing to send,
                // so it never notices and keeps the connection open for the
                // life of the process. Abort it.
                self.abort_session_task(sid);
                if had_session {
                    tracing::info!(session = %sid, "Disconnected session of blocked contact");
                }
            }
        }
        self.add_toast(ToastLevel::Info, format!("Blocked {}", name));
        Ok(())
    }

    /// Unblock a contact. Trust returns to Verified when its fingerprint was
    /// confirmed in some chat, otherwise back to Unverified.
    pub fn unblock_contact(&mut self, contact_id: Uuid) -> Result<()> {
        let confirmed = {
            let contact = self
                .contacts
                .get(&contact_id)
                .ok_or_else(|| anyhow::anyhow!("No such contact"))?;
            contact.fingerprint.as_deref().is_some_and(|fp| {
                self.chats
                    .values()
                    .any(|c| c.peer_fingerprint.as_deref() == Some(fp))
            })
        };
        let contact = self
            .contacts
            .get_mut(&contact_id)
            .ok_or_else(|| anyhow::anyhow!("No such contact"))?;
        contact.trust_state = if confirmed {
            TrustState::Verified
        } else {
            TrustState::Unverified
        };
        let name = contact.name.clone();
        tracing::info!(%contact_id, name = %name, state = ?contact.trust_state, "Contact unblocked");
        self.add_toast(ToastLevel::Info, format!("Unblocked {}", name));
        Ok(())
    }

    /// Whether a peer fingerprint belongs to a blocked contact.
    pub(super) fn is_fingerprint_blocked(&self, fingerprint: &str) -> bool {
        self.contacts.values().any(|c| {
            c.trust_state == TrustState::Blocked && c.fingerprint.as_deref() == Some(fingerprint)
        })
    }

    /// Record a successful TOFU confirmation on the matching contact: the
    /// contact bound to this chat (or one sharing the fingerprint) becomes
    /// Verified, filling in its fingerprint when it had none. Blocked contacts
    /// are never promoted.
    pub(super) fn promote_contact_verified(&mut self, chat_id: Uuid, fingerprint: &str) {
        let contact_id = self
            .contact_to_chat
            .iter()
            .find(|(_, &cid)| cid == chat_id)
            .map(|(&id, _)| id)
            .or_else(|| {
                self.contacts
                    .values()
                    .find(|c| c.fingerprint.as_deref() == Some(fingerprint))
                    .map(|c| c.id)
            });
        let Some(id) = contact_id else { return };
        let Some(contact) = self.contacts.get_mut(&id) else {
            return;
        };
        if contact.trust_state == TrustState::Blocked {
            return;
        }
        if contact.fingerprint.is_none() {
            contact.fingerprint = Some(fingerprint.to_string());
        }
        if contact.fingerprint.as_deref() == Some(fingerprint)
            && contact.trust_state == TrustState::Unverified
        {
            contact.trust_state = TrustState::Verified;
            tracing::info!(contact = %contact.name, "Contact promoted to Verified after TOFU confirmation");
        }
    }

    /// Associate a contact with a one-to-one chat (useful when a session is created for that contact)
    pub fn associate_contact_with_chat(&mut self, contact_id: Uuid, chat_id: Uuid) {
        tracing::debug!(
            "associate_contact_with_chat: contact_id={}, chat_id={}",
            contact_id,
            chat_id
        );
        self.contact_to_chat.insert(contact_id, chat_id);
        if let Some(chat) = self.chats.get_mut(&chat_id) {
            if !chat.participants.contains(&contact_id) {
                chat.participants.push(contact_id);
            }
        }
        tracing::info!("Associated contact {} -> chat {}", contact_id, chat_id);
    }

    /// Attempt to reconnect to previously mapped contacts based on persisted history.
    /// Best-effort: skips missing contacts and logs warnings instead of failing fast.
    pub async fn auto_reconnect_contacts(&mut self, privkey: &RsaPrivateKey) {
        if !self.config.auto_connect {
            tracing::info!("auto_connect disabled; skipping auto reconnect");
            return;
        }

        let mappings: Vec<(Uuid, Uuid)> = self
            .contact_to_chat
            .iter()
            .map(|(c, ch)| (*c, *ch))
            .collect();
        tracing::info!(
            count = mappings.len(),
            "Starting auto reconnect for mapped contacts"
        );

        for (contact_id, mapped_chat_id) in mappings {
            let Some(contact) = self.contacts.get(&contact_id) else {
                tracing::warn!(%contact_id, "Skipping reconnect: contact missing; removing stale mapping");
                self.contact_to_chat.remove(&contact_id);
                continue;
            };
            if contact.trust_state == TrustState::Blocked {
                tracing::info!(%contact_id, "Skipping reconnect: contact is blocked");
                continue;
            }

            tracing::debug!(
                %contact_id,
                mapped_chat_id = %mapped_chat_id,
                has_address = %contact.address.is_some(),
                has_fp = %contact.fingerprint.is_some(),
                "Auto reconnect attempt"
            );

            match self
                .connect_to_contact(contact_id, Some(mapped_chat_id), privkey)
                .await
            {
                Ok(chat_id) => {
                    tracing::info!(%contact_id, %chat_id, "Auto reconnect succeeded");
                }
                Err(e) => {
                    tracing::warn!(%contact_id, error = %e, "Auto reconnect failed");
                }
            }
        }
    }
}
