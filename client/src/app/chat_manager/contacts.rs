//! Contacts: adding/importing, chat association, trust lifecycle
//! (verify/block), and auto-reconnect on startup.

use super::*;

/// Case-insensitive fingerprint equality for trust decisions: storage is
/// canonical, but a block that misses on case is the failure that matters.
fn same_fingerprint(stored: Option<&str>, fingerprint: &str) -> bool {
    stored.is_some_and(|fp| fp.eq_ignore_ascii_case(fingerprint))
}

impl ChatManager {
    /// Parse a `host:port` address.
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
        let fingerprint = fingerprint
            .as_deref()
            .map(crate::util::canonical_fingerprint);
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
        tracing::debug!(id = %id, total_contacts = %self.contacts.len(), "Contact added");
        id
    }

    /// Save a contact parsed from an invite link.
    ///
    /// Importing the same link twice used to add a second identical card, and
    /// importing your *own* link added you to your own contacts. Both are
    /// resolved by identity, not by name: an invite for a fingerprint we already
    /// have refreshes that contact instead of duplicating it.
    pub fn import_contact(&mut self, contact: Contact) -> Result<Uuid> {
        self.import_contact_from_link(contact, true)
    }

    /// [`Self::import_contact`] for a contact parsed from an invite link,
    /// saying whether the link was **signed** by the key it names.
    ///
    /// An unsigned (v1) link may add a new contact, but never change an
    /// existing one (APP-15): it proves nothing about who wrote it — the
    /// fingerprint and public key it carries are public — so letting it refresh
    /// a known contact let anyone repoint a verified contact's address and
    /// rename them. Only a signed link, which only that key's owner can make,
    /// refreshes one.
    pub fn import_contact_from_link(&mut self, mut contact: Contact, signed: bool) -> Result<Uuid> {
        contact.fingerprint = contact
            .fingerprint
            .as_deref()
            .map(crate::util::canonical_fingerprint);
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
                .find(|c| same_fingerprint(c.fingerprint.as_deref(), fp))
                .map(|c| c.id)
            {
                if !signed {
                    let known = self.contacts.get(&existing).map(|c| c.name.clone());
                    anyhow::bail!(
                        "This is an unsigned (old-style) invite for {}, who is already in your \
                         contacts. An unsigned link cannot change an existing contact — ask them \
                         for a new link from an up-to-date app.",
                        known.unwrap_or_default()
                    );
                }
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
        // An invite link is not verification, and this layer enforces it for
        // every caller (like `MIN_PASSWORD_LEN` in `Identity::encrypt`). New
        // contacts only, so re-pasting a link cannot un-verify anyone; `Blocked`
        // is kept, since only deletion may lift a block.
        contact.trust_state = match contact.trust_state {
            TrustState::Blocked => TrustState::Blocked,
            TrustState::Unverified | TrustState::Verified | TrustState::Trusted => {
                TrustState::Unverified
            }
        };
        self.contacts.insert(id, contact);
        Ok(id)
    }

    /// Remove a contact **and the trust that came with it**: its fingerprint is
    /// cleared from every chat (unless another contact vouches for it), so the
    /// peer must be verified again, and its block is lifted.
    pub fn remove_contact(&mut self, contact_id: Uuid) {
        tracing::info!(contact_id = %contact_id, "Removing contact");
        let removed = self.contacts.remove(&contact_id);
        self.contact_to_chat.remove(&contact_id);

        if let Some(fp) = removed.as_ref().and_then(|c| c.fingerprint.clone()) {
            // Only if no *other* contact still vouches for this fingerprint.
            let still_known = self
                .contacts
                .values()
                .any(|c| same_fingerprint(c.fingerprint.as_deref(), &fp));
            if !still_known {
                for chat in self.chats.values_mut() {
                    if same_fingerprint(chat.peer_fingerprint.as_deref(), &fp) {
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

    /// Whether deleting this contact would also lift a block (for the dialog).
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

        // Tear down every live session with that fingerprint, events receiver
        // included, so the blocked peer cannot keep delivering.
        if let Some(fp) = fingerprint {
            let session_ids: Vec<Uuid> = self
                .chats
                .values()
                .filter(|c| same_fingerprint(c.peer_fingerprint.as_deref(), &fp))
                .map(|c| self.session_for_chat(c.id))
                .collect();
            for sid in session_ids {
                // Fail receives before dropping the mapping that resolves them.
                self.fail_incoming_transfers(sid, "blocked this contact");
                let had_session = self.sessions.contains_key(&sid);
                // Aborts the task too (a task parked on a read never notices).
                self.teardown_session(sid);
                self.chat_id_mapping.retain(|_, v| *v != sid);
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
                    .any(|c| same_fingerprint(c.peer_fingerprint.as_deref(), fp))
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
            c.trust_state == TrustState::Blocked
                && same_fingerprint(c.fingerprint.as_deref(), fingerprint)
        })
    }

    /// Record a TOFU confirmation on the matching contact (bound to this chat,
    /// or sharing the fingerprint): it becomes Verified, gaining the fingerprint
    /// if it had none. Blocked contacts are never promoted.
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

    /// Replace a contact's fingerprint with one the user accepted over an
    /// invite-mismatch warning (CRYPTO-04), and reset it to `Unverified` so
    /// `promote_contact_verified` records the acceptance. Blocked contacts are
    /// left alone: a block is never lifted or moved by a connection attempt.
    pub(super) fn rebind_contact_fingerprint(&mut self, contact_id: Uuid, fingerprint: &str) {
        let Some(contact) = self.contacts.get_mut(&contact_id) else {
            return;
        };
        if contact.trust_state == TrustState::Blocked {
            return;
        }
        tracing::warn!(
            contact = %contact.name,
            old = ?contact.fingerprint,
            new = %fingerprint,
            "Contact rebound to a key the user accepted over its invite"
        );
        contact.fingerprint = Some(fingerprint.to_string());
        // The public key the invite carried belongs to the old fingerprint.
        contact.public_key = None;
        contact.trust_state = TrustState::Unverified;
    }

    /// Associate a contact with a one-to-one chat.
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

    /// Best-effort reconnect to previously mapped contacts.
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
