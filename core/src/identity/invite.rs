//! The invite-link wire format: one definition, used to both mint and read.
//!
//! A signed invite is verified by **re-serializing** the payload and checking
//! the signature over those bytes, so the reader's struct must match the
//! writer's field for field, in order, with the same skip rules. That struct
//! used to be declared three times — in the generator, in the client's parser
//! and in a test helper — each with a comment begging the others to stay in
//! sync. The test helper had already drifted (it lacked `addresses`). Now there
//! is one, here.
//!
//! What this module decides is only *what the link says and whether it is
//! internally consistent*. It never decides trust: a contact made from a valid
//! invite is still `Unverified` until the user compares the SAS.

use anyhow::{anyhow, bail, Result};
use base64::Engine;
use rsa::RsaPrivateKey;
use serde::{Deserialize, Serialize};

use crate::core::crypto::{fingerprint_pubkey, pem_decode_public, rsa_sign_pss, rsa_verify_pss};

/// Every invite link starts with this.
pub const INVITE_PREFIX: &str = "chat-p2p://invite/";
/// A signed (v2+) invite link starts with this.
pub const SIGNED_INVITE_PREFIX: &str = "chat-p2p://invite/v2/";

/// The legacy (v1) payload: unsigned, standard base64.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct InvitePayload {
    pub name: String,
    pub address: Option<String>,
    pub fingerprint: String,
    pub public_key: String,
}

/// The signed payload. **Field order is part of the signature** — do not
/// reorder, rename or insert fields; append only, with
/// `#[serde(default, skip_serializing_if = ...)]` so links minted before the
/// new field re-serialize byte-identically and keep verifying.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SignedInvitePayload {
    pub version: u32,
    pub timestamp: u64,
    /// Random per-invite nonce, hex.
    pub nonce: String,
    pub name: String,
    pub address: Option<String>,
    pub relay_server: Option<String>,
    pub relay_token: Option<String>,
    pub fingerprint: String,
    pub public_key: String,
    /// The full ordered candidate list; omitted when it adds nothing beyond
    /// `address` (0 or 1 candidate). Must stay last with this exact skip rule.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub addresses: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct SignedInvite {
    payload: SignedInvitePayload,
    /// RSA-PSS over `serde_json::to_string(&payload)`.
    signature: Vec<u8>,
}

/// Whether a link is the **signed** (v2+) format. Decided by the prefix, not by
/// searching for `/v2/`: a v1 payload is standard base64, which contains `/`,
/// so an unsigned link could carry `/v2/` by chance.
pub fn is_signed(link: &str) -> bool {
    link.trim().starts_with(SIGNED_INVITE_PREFIX)
}

/// Sign `payload` with `privkey` and encode it as a v2 link.
pub fn encode_signed(payload: SignedInvitePayload, privkey: &RsaPrivateKey) -> Result<String> {
    // Deterministic for this app (serde_json, fixed field order), but not a
    // general-purpose canonicalization: the verifier re-serializes with the
    // same struct, which is what makes the bytes agree.
    let signature = rsa_sign_pss(privkey, serde_json::to_string(&payload)?.as_bytes())?;
    let json = serde_json::to_string(&SignedInvite { payload, signature })?;
    Ok(format!(
        "{SIGNED_INVITE_PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
    ))
}

/// Encode a legacy unsigned (v1) link.
pub fn encode_unsigned(payload: &InvitePayload) -> Result<String> {
    let json = serde_json::to_string(payload)?;
    Ok(format!(
        "{INVITE_PREFIX}{}",
        base64::engine::general_purpose::STANDARD.encode(json)
    ))
}

/// Reject an invite whose `fingerprint` field does not match the key it
/// ships. The two are separate fields on the wire and only the key is
/// self-attesting; the fingerprint is what every trust decision is made
/// against, so a mismatch means one of them is a lie.
pub fn check_fingerprint_binding(claimed: &str, public_key_pem: &str) -> Result<()> {
    let derived = fingerprint_pubkey(public_key_pem.as_bytes())
        .map_err(|e| anyhow!("Invalid public key in invite: {}", e))?;
    if !derived.eq_ignore_ascii_case(claimed.trim()) {
        tracing::warn!(
            claimed = %claimed,
            derived = %derived,
            "rejecting invite: fingerprint does not match its public key"
        );
        bail!(
            "This invite is inconsistent: the fingerprint it shows does not belong to the key it carries. Ask the sender for a fresh link."
        );
    }
    Ok(())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Decode and verify a signed link (with or without surrounding whitespace):
/// signature, fingerprint ↔ key binding, and the validity window. Returns the
/// payload only if all three hold, with its fingerprint in canonical form.
pub fn decode_signed(link: &str) -> Result<SignedInvitePayload> {
    decode_signed_at(link, now_secs())
}

fn decode_signed_at(link: &str, now: u64) -> Result<SignedInvitePayload> {
    let encoded = link
        .trim()
        .strip_prefix(SIGNED_INVITE_PREFIX)
        .ok_or_else(|| anyhow!("Not a signed invite link"))?;
    let json = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|e| anyhow!("Invalid v2 invite link: {}", e))?;
    let invite: SignedInvite =
        serde_json::from_slice(&json).map_err(|e| anyhow!("Invalid v2 invite data: {}", e))?;

    let payload = &invite.payload;
    let pubkey = pem_decode_public(&payload.public_key)
        .map_err(|e| anyhow!("Invalid public key in invite: {}", e))?;
    rsa_verify_pss(
        &pubkey,
        serde_json::to_string(payload)?.as_bytes(),
        &invite.signature,
    )
    .map_err(|e| {
        tracing::warn!(error = %e, "v2 invite signature verification failed");
        anyhow!("Invite signature verification failed: {}", e)
    })?;

    // The signature only proves that whoever built this invite holds the
    // private key for the `public_key` *inside it*. `fingerprint` is a separate
    // field, and it is the one that decides trust everywhere else — so bind it.
    check_fingerprint_binding(&payload.fingerprint, &payload.public_key)?;
    let fingerprint = crate::util::canonical_fingerprint(&payload.fingerprint);

    // The timestamp is covered by the signature, so a stale or future-dated
    // value cannot be forged without breaking verification above.
    let ts = payload.timestamp;
    if ts > now + crate::INVITE_TIMESTAMP_SKEW_SECS {
        tracing::warn!(timestamp = ts, now, "Rejecting invite dated in the future");
        bail!(
            "Invite timestamp is in the future - check the sender's clock and ask for a fresh invite"
        );
    }
    if now.saturating_sub(ts) > crate::INVITE_MAX_AGE_SECS {
        tracing::warn!(timestamp = ts, now, "Rejecting expired invite");
        bail!(
            "Invite has expired (older than {} days) - ask the sender for a fresh one",
            crate::INVITE_MAX_AGE_SECS / 86_400
        );
    }

    tracing::debug!(timestamp = ts, "Verified signed invite");
    Ok(SignedInvitePayload {
        fingerprint,
        ..invite.payload
    })
}

/// Decode a legacy unsigned link (the prefix is optional — bare base64 was
/// accepted historically). Nothing signs a v1 invite, so passing the binding
/// check does not make it trustworthy; it only rejects a link that is
/// internally inconsistent.
pub fn decode_unsigned(link: &str) -> Result<InvitePayload> {
    let link = link.trim();
    let encoded = link.strip_prefix(INVITE_PREFIX).unwrap_or(link);
    let json = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| anyhow!("Invalid invite link: {}", e))?;
    let mut payload: InvitePayload =
        serde_json::from_slice(&json).map_err(|e| anyhow!("Invalid invite data: {}", e))?;
    check_fingerprint_binding(&payload.fingerprint, &payload.public_key)?;
    payload.fingerprint = crate::util::canonical_fingerprint(&payload.fingerprint);
    Ok(payload)
}

/// Re-sign `link`'s payload after `edit` — test support for links the
/// generator would never produce (expired, future-dated, tampered).
#[doc(hidden)]
pub fn resign_for_test(
    link: &str,
    privkey: &RsaPrivateKey,
    edit: impl FnOnce(&mut SignedInvitePayload),
) -> Result<String> {
    let encoded = link
        .trim()
        .strip_prefix(SIGNED_INVITE_PREFIX)
        .ok_or_else(|| anyhow!("Not a signed invite link"))?;
    let json = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(encoded)?;
    let mut invite: SignedInvite = serde_json::from_slice(&json)?;
    edit(&mut invite.payload);
    encode_signed(invite.payload, privkey)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{generate_rsa_keypair, pem_encode_public};

    fn payload(key: &RsaPrivateKey, timestamp: u64) -> SignedInvitePayload {
        let pem = pem_encode_public(&rsa::RsaPublicKey::from(key)).unwrap();
        SignedInvitePayload {
            version: 2,
            timestamp,
            nonce: "00".into(),
            name: "Alice".into(),
            address: Some("10.0.0.1:12345".into()),
            relay_server: None,
            relay_token: None,
            fingerprint: fingerprint_pubkey(pem.as_bytes()).unwrap(),
            public_key: pem,
            addresses: Vec::new(),
        }
    }

    #[test]
    fn a_signed_link_round_trips() {
        let key = generate_rsa_keypair(2048).unwrap();
        let now = now_secs();
        let original = payload(&key, now);
        let link = encode_signed(original.clone(), &key).unwrap();
        assert!(is_signed(&link));
        assert_eq!(decode_signed_at(&link, now).unwrap(), original);
    }

    #[test]
    fn a_payload_edited_after_signing_is_refused() {
        let key = generate_rsa_keypair(2048).unwrap();
        let now = now_secs();
        let link = encode_signed(payload(&key, now), &key).unwrap();
        let json = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(link.strip_prefix(SIGNED_INVITE_PREFIX).unwrap())
            .unwrap();
        let mut invite: SignedInvite = serde_json::from_slice(&json).unwrap();
        invite.payload.address = Some("6.6.6.6:12345".into());
        let tampered = format!(
            "{SIGNED_INVITE_PREFIX}{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_string(&invite).unwrap())
        );
        let err = decode_signed_at(&tampered, now).unwrap_err();
        assert!(err.to_string().contains("signature"), "{err}");
    }

    #[test]
    fn the_validity_window_is_enforced_at_both_ends() {
        let key = generate_rsa_keypair(2048).unwrap();
        let now = 1_000_000_000;
        let old = encode_signed(payload(&key, now - crate::INVITE_MAX_AGE_SECS - 1), &key).unwrap();
        assert!(decode_signed_at(&old, now)
            .unwrap_err()
            .to_string()
            .contains("expired"));
        let future = encode_signed(
            payload(&key, now + crate::INVITE_TIMESTAMP_SKEW_SECS + 1),
            &key,
        )
        .unwrap();
        assert!(decode_signed_at(&future, now)
            .unwrap_err()
            .to_string()
            .contains("future"));
    }

    #[test]
    fn unsigned_links_round_trip_and_bind_their_fingerprint() {
        let key = generate_rsa_keypair(2048).unwrap();
        let signed = payload(&key, 0);
        let v1 = InvitePayload {
            name: signed.name,
            address: signed.address,
            fingerprint: signed.fingerprint,
            public_key: signed.public_key,
        };
        let link = encode_unsigned(&v1).unwrap();
        assert!(!is_signed(&link));
        assert_eq!(decode_unsigned(&link).unwrap(), v1);

        let lying = InvitePayload {
            fingerprint: "00".repeat(32),
            ..v1
        };
        let err = decode_unsigned(&encode_unsigned(&lying).unwrap()).unwrap_err();
        assert!(err.to_string().contains("inconsistent"), "{err}");
    }

    /// The binding check accepts any case; what comes out must be the one
    /// spelling a handshake produces, or comparisons against it silently fail.
    #[test]
    fn decoded_fingerprints_are_canonical() {
        let key = generate_rsa_keypair(2048).unwrap();
        let now = now_secs();
        let mut shouting = payload(&key, now);
        let canonical = shouting.fingerprint.clone();
        shouting.fingerprint = format!("  {}  ", canonical.to_ascii_uppercase());

        let signed = encode_signed(shouting.clone(), &key).unwrap();
        assert_eq!(
            decode_signed_at(&signed, now).unwrap().fingerprint,
            canonical
        );

        let v1 = InvitePayload {
            name: shouting.name,
            address: shouting.address,
            fingerprint: shouting.fingerprint,
            public_key: shouting.public_key,
        };
        assert_eq!(
            decode_unsigned(&encode_unsigned(&v1).unwrap())
                .unwrap()
                .fingerprint,
            canonical
        );
    }

    /// A signed invite whose `public_key` smuggles the victim's key in a
    /// preamble, signed by the attacker and claiming the victim's fingerprint,
    /// used to pass both the signature and the binding check — and then
    /// refreshed the victim's verified contact with the attacker's address.
    #[test]
    fn an_invite_carrying_two_keys_is_refused() {
        let victim = generate_rsa_keypair(2048).unwrap();
        let attacker = generate_rsa_keypair(2048).unwrap();
        let victim_pem = pem_encode_public(&rsa::RsaPublicKey::from(&victim)).unwrap();
        let crafted = format!(
            "x{}\n{}",
            victim_pem.trim_end(),
            pem_encode_public(&rsa::RsaPublicKey::from(&attacker)).unwrap()
        );
        let now = now_secs();
        let mut forged = payload(&victim, now);
        forged.public_key = crafted.clone();
        forged.fingerprint = fingerprint_pubkey(victim_pem.as_bytes()).unwrap();
        let link = encode_signed(forged.clone(), &attacker).unwrap();
        assert!(decode_signed_at(&link, now).is_err());

        let v1 = InvitePayload {
            name: forged.name,
            address: forged.address,
            fingerprint: forged.fingerprint,
            public_key: crafted,
        };
        assert!(decode_unsigned(&encode_unsigned(&v1).unwrap()).is_err());
    }

    /// A v1 invite is fingerprinted before any PEM parse; a key whose markers
    /// are out of order used to panic the whole app.
    #[test]
    fn a_malformed_v1_key_is_an_error_not_a_crash() {
        let v1 = InvitePayload {
            name: "x".into(),
            address: None,
            fingerprint: "00".repeat(32),
            public_key: "-----END PUBLIC KEY----------BEGIN PUBLIC KEY-----".into(),
        };
        assert!(decode_unsigned(&encode_unsigned(&v1).unwrap()).is_err());
    }
}
