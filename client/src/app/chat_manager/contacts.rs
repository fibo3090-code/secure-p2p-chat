//! Contacts: adding/importing, chat association, auto-reconnect on
//! startup, and group-chat creation.

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

    pub fn import_contact(&mut self, mut contact: Contact) -> Uuid {
        let id = Uuid::new_v4();
        contact.id = id;
        contact.created_at = chrono::Utc::now();
        self.contacts.insert(id, contact);
        id
    }

    /// Remove a contact
    pub fn remove_contact(&mut self, contact_id: Uuid) {
        tracing::info!(contact_id = %contact_id, "Removing contact");
        self.contacts.remove(&contact_id);
        self.contact_to_chat.remove(&contact_id);
        tracing::debug!(remaining_contacts = %self.contacts.len(), "Contact removed");
    }

    /// Get a contact
    pub fn get_contact(&self, contact_id: Uuid) -> Option<&Contact> {
        self.contacts.get(&contact_id)
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

    /// Create a group chat with given participants and optional title
    pub fn create_group_chat(&mut self, participants: Vec<Uuid>, title: Option<String>) -> Uuid {
        let chat_id = Uuid::new_v4();
        let default_title = title.unwrap_or_else(|| {
            if participants.is_empty() {
                "Group".to_string()
            } else {
                format!("Group ({})", participants.len())
            }
        });

        let chat = Chat {
            id: chat_id,
            title: default_title,
            kind: ChatKind::Group,
            transport: Transport::Direct,
            peer_fingerprint: None,
            participants,
            messages: Vec::new(),
            created_at: chrono::Utc::now(),
            peer_typing: false,
            typing_since: None,
            send_seq: 0,
            recv_seq: 0,
            is_host_placeholder: false,
        };

        self.chats.insert(chat_id, chat);
        chat_id
    }
}
