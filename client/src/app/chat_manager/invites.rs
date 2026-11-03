//! Invite links: signed (v2/v3) and legacy (v1) generation, parsing, and QR codes.

use super::*;

use crate::identity::invite::{self, InvitePayload, SignedInvitePayload};

/// What an address field in an invite becomes on a contact: a normalized
/// `host:port`, or nothing for a blank, placeholder or unparsable value.
fn sanitize_invite_address(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("YOUR_IP:PORT") {
        return None;
    }
    ChatManager::parse_address(trimmed)
        .ok()
        .map(|(host, port)| crate::util::format_host_port(&host, port))
}

/// A fresh, unverified contact for whoever an invite names. An invite is never
/// verification — the SAS prompt on first connection is.
fn unverified_contact(name: String, fingerprint: String, public_key: String) -> Contact {
    Contact {
        id: Uuid::new_v4(),
        name,
        address: None,
        addresses: Vec::new(),
        relay_server: None,
        relay_token: None,
        fingerprint: Some(fingerprint),
        public_key: Some(public_key),
        created_at: chrono::Utc::now(),
        trust_state: TrustState::Unverified,
        notes: String::new(),
        tags: Vec::new(),
        last_seen: None,
    }
}

/// Contact from a verified signed payload.
fn contact_from_signed(payload: SignedInvitePayload) -> Contact {
    // Candidate direct addresses in priority order: the multi-address list
    // when present, otherwise the single legacy `address`. Sanitize each and
    // drop duplicates while keeping order, so a peer can try them in turn
    // (e.g. external then LAN).
    let raw = if payload.addresses.is_empty() {
        payload.address.into_iter().collect()
    } else {
        payload.addresses
    };
    let mut candidates: Vec<String> = Vec::new();
    for clean in raw.iter().filter_map(|a| sanitize_invite_address(a)) {
        if !candidates.contains(&clean) {
            candidates.push(clean);
        }
    }
    let relay_server = payload
        .relay_server
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| crate::util::parse_host_port(s, Some(crate::PORT_DEFAULT)).ok())
        .map(|(host, port)| crate::util::format_host_port(&host, port));
    let relay_token = payload.relay_token.filter(|t| !t.trim().is_empty());

    let mut contact = unverified_contact(payload.name, payload.fingerprint, payload.public_key);
    contact.address = candidates.first().cloned();
    // Only persist the extra list when it adds something beyond `address`.
    if candidates.len() > 1 {
        contact.addresses = candidates;
    }
    contact.relay_server = relay_server;
    contact.relay_token = relay_token;
    contact
}

/// Contact from a legacy unsigned payload.
fn contact_from_unsigned(payload: InvitePayload) -> Contact {
    let mut contact = unverified_contact(payload.name, payload.fingerprint, payload.public_key);
    contact.address = payload.address.as_deref().and_then(sanitize_invite_address);
    contact
}

impl ChatManager {
    /// Whether a link is the **signed** (v2+) invite format. An unsigned v1
    /// link carries no proof of who made it — anyone can mint one naming
    /// anybody — so callers surface that difference to the user instead of
    /// importing both silently.
    pub fn invite_link_is_signed(link: &str) -> bool {
        invite::is_signed(link)
    }

    /// Generate a legacy unsigned (v1) invite link:
    /// `chat-p2p://invite/<base64_json>`.
    pub fn generate_invite_link(
        &self,
        name: &str,
        address: Option<String>,
        fingerprint: &str,
        public_key_pem: &str,
    ) -> Result<String> {
        invite::encode_unsigned(&InvitePayload {
            name: name.to_string(),
            address,
            fingerprint: fingerprint.to_string(),
            public_key: public_key_pem.to_string(),
        })
    }

    /// Parse an invite link into an **unverified** contact. Signed links
    /// (`chat-p2p://invite/v2/…`) are verified — signature, fingerprint ↔ key
    /// binding, validity window — before anything is read from them; legacy
    /// unsigned links (`chat-p2p://invite/…`) only get the binding check. See
    /// [`crate::identity::invite`].
    pub fn parse_invite_link(&self, link: &str) -> Result<Contact> {
        if invite::is_signed(link) {
            invite::decode_signed(link)
                .inspect_err(|e| tracing::warn!(error = %e, "Rejected signed invite"))
                .map(contact_from_signed)
        } else {
            tracing::warn!(
                "Parsing legacy v1 unsigned invite link - prefer v2 signed format for security"
            );
            invite::decode_unsigned(link)
                .inspect_err(|e| tracing::warn!(error = %e, "Rejected unsigned invite"))
                .map(contact_from_unsigned)
        }
    }

    /// Generate a QR code for an invite link (as PNG bytes)
    pub fn generate_invite_qr(&self, invite_link: &str) -> Result<Vec<u8>> {
        use qrcode::QrCode;

        let code = QrCode::new(invite_link.as_bytes())
            .map_err(|e| anyhow::anyhow!("Failed to generate QR code: {}", e))?;

        let qr_image = code
            .render::<image::Luma<u8>>()
            .min_dimensions(200, 200)
            .build();

        let mut bytes = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut bytes);
        image::DynamicImage::ImageLuma8(qr_image)
            .write_to(&mut cursor, image::ImageFormat::Png)
            .map_err(|e| anyhow::anyhow!("Failed to encode QR code: {}", e))?;

        Ok(bytes)
    }
}
