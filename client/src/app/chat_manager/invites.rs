//! Invite links: signed (v2/v3) and legacy (v1) generation, parsing, and QR codes.

use super::*;

impl ChatManager {
    /// Whether a link is the **signed** (v2+) invite format. An unsigned v1
    /// link carries no proof of who made it — anyone can mint one naming
    /// anybody — so callers surface that difference to the user instead of
    /// importing both silently.
    pub fn invite_link_is_signed(link: &str) -> bool {
        link.contains("/v2/")
    }

    /// Reject an invite whose `fingerprint` field does not match the key it
    /// ships. The two are separate fields on the wire and only the key is
    /// self-attesting; the fingerprint is what every trust decision in the app
    /// is made against, so a mismatch means one of them is a lie.
    fn check_fingerprint_binding(claimed: &str, public_key_pem: &str) -> Result<()> {
        let derived = crate::core::crypto::fingerprint_pubkey(public_key_pem.as_bytes());
        if !derived.eq_ignore_ascii_case(claimed.trim()) {
            tracing::warn!(
                claimed = %claimed,
                derived = %derived,
                "rejecting invite: fingerprint does not match its public key"
            );
            anyhow::bail!(
                "This invite is inconsistent: the fingerprint it shows does not belong to the key it carries. Ask the sender for a fresh link."
            );
        }
        Ok(())
    }

    /// Generate an invite link for sharing contact information
    /// Format: chat-p2p://invite/<base64_json>
    pub fn generate_invite_link(
        &self,
        name: &str,
        address: Option<String>,
        fingerprint: &str,
        public_key_pem: &str,
    ) -> Result<String> {
        use base64::Engine;
        use serde::{Deserialize, Serialize};

        #[derive(Serialize, Deserialize)]
        struct InvitePayload {
            name: String,
            address: Option<String>,
            fingerprint: String,
            public_key: String,
        }

        let payload = InvitePayload {
            name: name.to_string(),
            address,
            fingerprint: fingerprint.to_string(),
            public_key: public_key_pem.to_string(),
        };

        let json = serde_json::to_string(&payload)?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(json);
        Ok(format!("chat-p2p://invite/{}", encoded))
    }

    /// Parse an invite link and create a Contact
    /// Supports both v1 (unsigned) and v2 (signed) formats
    /// v1: chat-p2p://invite/<base64_json>
    /// v2: chat-p2p://invite/v2/<url_safe_base64_json>
    pub fn parse_invite_link(&self, link: &str) -> Result<Contact> {
        use serde::{Deserialize, Serialize};

        #[derive(Serialize, Deserialize)]
        struct InvitePayload {
            name: String,
            address: Option<String>,
            fingerprint: String,
            public_key: String,
        }

        #[derive(Serialize, Deserialize)]
        struct SignedInvitePayload {
            version: u32,
            timestamp: u64,
            nonce: String,
            name: String,
            address: Option<String>,
            relay_server: Option<String>,
            relay_token: Option<String>,
            fingerprint: String,
            public_key: String,
            // MUST mirror the generator exactly (last field, same skip rule) so
            // the payload re-serializes to the same bytes that were signed —
            // including older invites that predate this field.
            #[serde(default, skip_serializing_if = "Vec::is_empty")]
            addresses: Vec<String>,
        }

        #[derive(Serialize, Deserialize)]
        struct SignedInvite {
            payload: SignedInvitePayload,
            signature: Vec<u8>,
        }

        tracing::debug!("Parsing invite link");

        // Check if this is a v2 (signed) or v1 (unsigned) invite
        if link.contains("/v2/") {
            // V2: Signed invite with RSA-PSS signature
            let encoded = link
                .strip_prefix("chat-p2p://invite/v2/")
                .ok_or_else(|| anyhow::anyhow!("Invalid v2 invite link format"))?;

            // Decode URL-safe base64
            use base64::Engine;
            let json = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(encoded)
                .map_err(|e| {
                    tracing::warn!(error = %e, "Invalid v2 invite link during base64 decode");
                    anyhow::anyhow!("Invalid v2 invite link: {}", e)
                })?;
            let json_str = String::from_utf8(json).map_err(|e| {
                tracing::warn!(error = %e, "Invalid UTF-8 in v2 invite link");
                anyhow::anyhow!("Invalid UTF-8 in v2 invite link: {}", e)
            })?;

            // Parse signed invite structure
            let signed_invite: SignedInvite = serde_json::from_str(&json_str).map_err(|e| {
                tracing::warn!(error = %e, "Invalid v2 invite data JSON");
                anyhow::anyhow!("Invalid v2 invite data: {}", e)
            })?;

            // Serialize payload back to JSON for signature verification
            let payload_json = serde_json::to_string(&signed_invite.payload).map_err(|e| {
                tracing::warn!(error = %e, "Failed to serialize payload for verification");
                anyhow::anyhow!("Serialization error: {}", e)
            })?;

            // Verify RSA-PSS signature using the public key from the invite
            let pubkey_pem = &signed_invite.payload.public_key;
            let pubkey = crate::core::crypto::pem_decode_public(pubkey_pem).map_err(|e| {
                tracing::warn!(error = %e, "Failed to decode public key from invite");
                anyhow::anyhow!("Invalid public key in invite: {}", e)
            })?;

            crate::core::crypto::rsa_verify_pss(
                &pubkey,
                payload_json.as_bytes(),
                &signed_invite.signature,
            )
            .map_err(|e| {
                tracing::warn!(error = %e, "v2 invite signature verification failed");
                anyhow::anyhow!("Invite signature verification failed: {}", e)
            })?;

            // The signature only proves that whoever built this invite holds the
            // private key for the `public_key` *inside it*. `fingerprint` is a
            // separate field, and it is the one that decides trust everywhere
            // else in the app — so if the two disagree, the invite is lying and
            // nothing downstream would ever notice. Bind them here.
            Self::check_fingerprint_binding(&signed_invite.payload.fingerprint, pubkey_pem)?;

            tracing::debug!(
                timestamp = signed_invite.payload.timestamp,
                "Successfully verified v2 signed invite"
            );

            // Enforce expiry: the timestamp is covered by the signature, so a
            // stale or future-dated value cannot be forged without breaking
            // verification above.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let ts = signed_invite.payload.timestamp;
            if ts > now + crate::INVITE_TIMESTAMP_SKEW_SECS {
                tracing::warn!(timestamp = ts, now, "Rejecting invite dated in the future");
                anyhow::bail!(
                    "Invite timestamp is in the future - check the sender's clock and ask for a fresh invite"
                );
            }
            if now.saturating_sub(ts) > crate::INVITE_MAX_AGE_SECS {
                tracing::warn!(timestamp = ts, now, "Rejecting expired invite");
                anyhow::bail!(
                    "Invite has expired (older than {} days) - ask the sender for a fresh one",
                    crate::INVITE_MAX_AGE_SECS / 86_400
                );
            }

            let payload = &signed_invite.payload;

            // Sanitize address: ignore placeholder or clearly invalid addresses like "YOUR_IP:PORT"
            let sanitize_addr = |raw: &str| -> Option<String> {
                let trimmed = raw.trim();
                if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("YOUR_IP:PORT") {
                    None
                } else {
                    Self::parse_address(trimmed)
                        .ok()
                        .map(|(host, port)| crate::util::format_host_port(&host, port))
                }
            };

            // Collect candidate direct addresses in priority order. Prefer the
            // multi-address list when present, otherwise fall back to the single
            // legacy `address`. Sanitize each and drop duplicates while keeping
            // order, so a peer can try them in turn (e.g. external then LAN).
            let raw_candidates: Vec<&String> = if payload.addresses.is_empty() {
                payload.address.iter().collect()
            } else {
                payload.addresses.iter().collect()
            };
            let mut candidates: Vec<String> = Vec::new();
            for raw in raw_candidates {
                if let Some(clean) = sanitize_addr(raw) {
                    if !candidates.contains(&clean) {
                        candidates.push(clean);
                    }
                }
            }
            let address = candidates.first().cloned();
            // Only persist the extra list when it adds something beyond `address`.
            let addresses = if candidates.len() > 1 {
                candidates
            } else {
                Vec::new()
            };
            let relay_server = payload.relay_server.as_ref().and_then(|server| {
                let trimmed = server.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    crate::util::parse_host_port(trimmed, Some(crate::PORT_DEFAULT))
                        .ok()
                        .map(|(host, port)| crate::util::format_host_port(&host, port))
                }
            });
            let relay_token = payload
                .relay_token
                .clone()
                .filter(|token| !token.trim().is_empty());

            // Create contact from v2 invite
            let contact = Contact {
                id: Uuid::new_v4(),
                name: payload.name.clone(),
                address,
                addresses,
                relay_server,
                relay_token,
                fingerprint: Some(payload.fingerprint.clone()),
                public_key: Some(payload.public_key.clone()),
                created_at: chrono::Utc::now(),
                trust_state: TrustState::Unverified,
                notes: String::new(),
                tags: Vec::new(),
                last_seen: None,
            };

            Ok(contact)
        } else {
            // V1: Legacy unsigned invite
            tracing::warn!(
                "Parsing legacy v1 unsigned invite link - prefer v2 signed format for security"
            );

            // Remove prefix if present
            let encoded = link.strip_prefix("chat-p2p://invite/").unwrap_or(link);

            // Decode base64
            use base64::Engine;
            let json = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|e| {
                    tracing::warn!(error = %e, "Invalid invite link during base64 decode");
                    anyhow::anyhow!("Invalid invite link: {}", e)
                })?;
            let json_str = String::from_utf8(json).map_err(|e| {
                tracing::warn!(error = %e, "Invalid UTF-8 in invite link");
                anyhow::anyhow!("Invalid UTF-8 in invite link: {}", e)
            })?;

            // Parse JSON
            let payload: InvitePayload = serde_json::from_str(&json_str).map_err(|e| {
                tracing::warn!(error = %e, "Invalid invite data JSON");
                anyhow::anyhow!("Invalid invite data: {}", e)
            })?;

            // Nothing signs a v1 invite, so this check does not make it
            // trustworthy — it only rejects a link that is internally
            // inconsistent. A v1 contact is imported Unverified and still has
            // to pass the SAS prompt on first connection.
            Self::check_fingerprint_binding(&payload.fingerprint, &payload.public_key)?;

            // Sanitize address: ignore placeholder or clearly invalid addresses like "YOUR_IP:PORT"
            let address = payload.address.as_ref().and_then(|addr| {
                let trimmed = addr.trim();
                if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("YOUR_IP:PORT") {
                    None
                } else {
                    Self::parse_address(trimmed)
                        .ok()
                        .map(|(host, port)| crate::util::format_host_port(&host, port))
                }
            });

            // Create contact
            let contact = Contact {
                id: Uuid::new_v4(),
                name: payload.name,
                address,
                addresses: Vec::new(),
                relay_server: None,
                relay_token: None,
                fingerprint: Some(payload.fingerprint),
                public_key: Some(payload.public_key),
                created_at: chrono::Utc::now(),
                trust_state: TrustState::Unverified,
                notes: String::new(),
                tags: Vec::new(),
                last_seen: None,
            };

            Ok(contact)
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
