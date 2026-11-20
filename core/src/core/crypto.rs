use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use anyhow::{anyhow, Result};
use ed25519_dalek::{
    Signature, Signer, SigningKey as Ed25519SigningKey, VerifyingKey as Ed25519VerifyingKey,
};
use hkdf::Hkdf;
use rand::{rngs::OsRng, RngCore};
use rsa::{
    pkcs1::{DecodeRsaPrivateKey, EncodeRsaPrivateKey},
    pkcs8::{DecodePublicKey, EncodePublicKey},
    pss::{Signature as RsaSignature, SigningKey, VerifyingKey},
    signature::{RandomizedSigner, SignatureEncoding, Verifier as RsaVerifier},
    RsaPrivateKey, RsaPublicKey,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use x25519_dalek::{EphemeralSecret, PublicKey as X25519PublicKey};

use crate::AES_KEY_SIZE;

/// Default RSA key size used in tests and key generation
pub const RSA_KEY_BITS: usize = 2048;

/// Generate RSA keypair (blocking operation)
pub fn generate_rsa_keypair(bits: usize) -> Result<RsaPrivateKey> {
    RsaPrivateKey::new(&mut OsRng, bits).map_err(|e| anyhow!("RSA keygen failed: {}", e))
}

/// Generate RSA keypair asynchronously (non-blocking for GUI)
pub async fn generate_rsa_keypair_async(bits: usize) -> Result<RsaPrivateKey> {
    tokio::task::spawn_blocking(move || generate_rsa_keypair(bits))
        .await
        .map_err(|e| anyhow!("Task join error: {}", e))?
}

/// Export RSA public key to PEM format
pub fn pem_encode_public(pubkey: &RsaPublicKey) -> Result<String> {
    pubkey
        .to_public_key_pem(Default::default())
        .map_err(|e| anyhow!("PEM encode failed: {}", e))
}

const PUBLIC_KEY_BEGIN: &str = "-----BEGIN PUBLIC KEY-----";
const PUBLIC_KEY_END: &str = "-----END PUBLIC KEY-----";

/// Import an RSA public key from PEM, strictly: exactly one `PUBLIC KEY` block and
/// nothing around it but whitespace. RFC 7468 lets a parser skip a free-text preamble,
/// which can carry a second block that a marker scan would read instead; every
/// peer-supplied key passes through here, so a preamble or trailing text is refused.
pub fn pem_decode_public(pem: &str) -> Result<RsaPublicKey> {
    let trimmed = pem.trim();
    let single_block = trimmed.starts_with(PUBLIC_KEY_BEGIN)
        && trimmed.ends_with(PUBLIC_KEY_END)
        && trimmed.matches(PUBLIC_KEY_BEGIN).count() == 1
        && trimmed.matches(PUBLIC_KEY_END).count() == 1;
    if !single_block {
        return Err(anyhow!(
            "PEM decode failed: expected exactly one PUBLIC KEY block and nothing else"
        ));
    }
    RsaPublicKey::from_public_key_pem(trimmed).map_err(|e| anyhow!("PEM decode failed: {}", e))
}

/// Export RSA private key to PEM format
pub fn pem_encode_private(privkey: &RsaPrivateKey) -> Result<String> {
    privkey
        .to_pkcs1_pem(Default::default())
        .map(|pem| pem.to_string())
        .map_err(|e| anyhow!("Private PEM encode failed: {}", e))
}

/// Import RSA private key from PEM format
pub fn pem_decode_private(pem: &str) -> Result<RsaPrivateKey> {
    RsaPrivateKey::from_pkcs1_pem(pem).map_err(|e| anyhow!("Private PEM decode failed: {}", e))
}

// RSA-OAEP was removed on purpose: the handshake uses X25519 for key agreement and RSA
// only for PSS signatures, so RSA decryption (the Marvin attack surface,
// RUSTSEC-2023-0071) stays out of the product. Do not reintroduce it.

/// The fingerprint of a public key: lowercase hex SHA-256 of its SPKI DER encoding.
/// Computed from the parsed key, so it names the key signatures are verified with;
/// for an honest PEM it equals the hash of the base64 body (SPKI DER is canonical).
pub fn fingerprint_public_key(key: &RsaPublicKey) -> Result<String> {
    let der = key
        .to_public_key_der()
        .map_err(|e| anyhow!("DER encode failed: {}", e))?;
    Ok(hex::encode(Sha256::digest(der.as_bytes())))
}

/// The fingerprint of a PEM-encoded public key; see [`fingerprint_public_key`]. One
/// strict parse, so the fingerprint and signature checks can never see different keys,
/// and a malformed PEM is an error rather than a fingerprint of its bytes.
pub fn fingerprint_pubkey(pem_bytes: &[u8]) -> Result<String> {
    let pem = std::str::from_utf8(pem_bytes).map_err(|e| anyhow!("PEM is not UTF-8: {}", e))?;
    fingerprint_public_key(&pem_decode_public(pem)?)
}

/// Sign data with RSA-PSS (SHA-256, randomized), returning the raw signature bytes.
pub fn rsa_sign_pss(privkey: &RsaPrivateKey, data: &[u8]) -> Result<Vec<u8>> {
    let signing_key = SigningKey::<Sha256>::new(privkey.clone());
    Ok(signing_key
        .sign_with_rng(&mut OsRng, data)
        .to_bytes()
        .to_vec())
}

/// Verify an RSA-PSS (SHA-256) signature over `data`.
pub fn rsa_verify_pss(pubkey: &RsaPublicKey, data: &[u8], signature: &[u8]) -> Result<()> {
    let verifying_key = VerifyingKey::<Sha256>::new(pubkey.clone());
    let rsa_signature = RsaSignature::try_from(signature)
        .map_err(|e| anyhow!("Failed to parse RSA signature: {}", e))?;
    verifying_key
        .verify(data, &rsa_signature)
        .map_err(|e| anyhow!("RSA-PSS signature verification failed: {}", e))
}

/// Signature scheme for identity proofs (the algorithm that signs the ephemeral key).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SignatureScheme {
    /// RSA-PSS with SHA-256 (kept for older peers)
    RsaPss = 1,
    /// Ed25519 EdDSA (preferred)
    Ed25519 = 2,
}

impl SignatureScheme {
    /// The numeric value on the wire.
    pub fn to_u8(self) -> u8 {
        self as u8
    }

    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(SignatureScheme::RsaPss),
            2 => Some(SignatureScheme::Ed25519),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            SignatureScheme::RsaPss => "RSA-PSS",
            SignatureScheme::Ed25519 => "Ed25519",
        }
    }
}

impl std::fmt::Display for SignatureScheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Domain separator for the Ed25519 subkey derived from an RSA identity. Frozen:
/// changing it changes every derived key and every binding peers have seen.
const ED25519_SUBKEY_INFO: &[u8] = b"p2pem-ed25519-identity-subkey-v1";

/// Domain separator for the signature that binds a derived Ed25519 key to the
/// RSA identity it belongs to. Also frozen.
const ED25519_BINDING_LABEL: &[u8] = b"P2PEM_ED25519_BINDING_V1";

/// Derive this identity's Ed25519 signing subkey from its RSA private key.
///
/// Deterministic, so an existing identity gains one without touching `identity.json`
/// (the only key to the message history). The fingerprint is still derived from the
/// RSA key, so this is a signing key under that identity; verifiers cannot derive it,
/// which is what [`cross_sign_ed25519`] is for. Retiring RSA will need an independent
/// key, minted when fingerprints are re-verified anyway.
pub fn derive_ed25519_subkey(privkey: &RsaPrivateKey) -> Result<Ed25519SigningKey> {
    use rsa::pkcs8::EncodePrivateKey;
    // The private key's DER is the secret input (far more than 256 bits of entropy).
    let der = privkey
        .to_pkcs8_der()
        .map_err(|e| anyhow!("Failed to encode RSA private key for derivation: {}", e))?;
    let hkdf = Hkdf::<Sha256>::new(None, der.as_bytes());
    let mut seed = zeroize::Zeroizing::new([0u8; 32]);
    hkdf.expand(ED25519_SUBKEY_INFO, seed.as_mut())
        .map_err(|e| anyhow!("Failed to derive Ed25519 subkey: {}", e))?;
    Ok(Ed25519SigningKey::from_bytes(&seed))
}

/// The bytes a binding signature covers: the label and the Ed25519 public key.
fn ed25519_binding_digest(ed_public: &Ed25519VerifyingKey) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(ED25519_BINDING_LABEL);
    hasher.update(ed_public.to_bytes());
    hasher.finalize().to_vec()
}

/// Sign an Ed25519 public key with the RSA identity it belongs to, so a peer who trusts
/// the fingerprint can accept proofs from this subkey. Without it an attacker could pair
/// a stolen `public_key_pem` with a signing key of their choosing.
pub fn cross_sign_ed25519(
    privkey: &RsaPrivateKey,
    ed_public: &Ed25519VerifyingKey,
) -> Result<Vec<u8>> {
    let signing_key = SigningKey::<Sha256>::new(privkey.clone());
    Ok(signing_key
        .sign_with_rng(&mut OsRng, &ed25519_binding_digest(ed_public))
        .to_vec())
}

/// Verify that `ed_public` is bound to the RSA identity `rsa_public`.
pub fn verify_ed25519_binding(
    rsa_public: &RsaPublicKey,
    ed_public: &Ed25519VerifyingKey,
    signature: &[u8],
) -> Result<()> {
    let verifying_key = VerifyingKey::<Sha256>::new(rsa_public.clone());
    let sig = RsaSignature::try_from(signature)
        .map_err(|e| anyhow!("Malformed Ed25519 binding signature: {}", e))?;
    RsaVerifier::verify(&verifying_key, &ed25519_binding_digest(ed_public), &sig)
        .map_err(|e| anyhow!("Ed25519 key is not bound to this identity: {}", e))
}

/// Generate Ed25519 keypair
pub fn generate_ed25519_keypair() -> (Ed25519SigningKey, Ed25519VerifyingKey) {
    let mut seed = [0u8; 32];
    OsRng.fill_bytes(&mut seed);
    let signing_key = Ed25519SigningKey::from_bytes(&seed);
    let verifying_key = signing_key.verifying_key();
    (signing_key, verifying_key)
}

/// Sign data with Ed25519
pub fn sign_ed25519(signing_key: &Ed25519SigningKey, data: &[u8]) -> Signature {
    signing_key.sign(data)
}

/// Verify Ed25519 signature
pub fn verify_ed25519(
    verifying_key: &Ed25519VerifyingKey,
    data: &[u8],
    signature: &Signature,
) -> Result<()> {
    use ed25519_dalek::Verifier as Ed25519VerifierTrait;
    verifying_key
        .verify(data, signature)
        .map_err(|e| anyhow!("Ed25519 signature verification failed: {}", e))
}

/// Parse a raw 32-byte Ed25519 public key, as carried on the wire.
pub fn ed25519_public_from_bytes(bytes: &[u8]) -> Result<Ed25519VerifyingKey> {
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow!("Ed25519 public key must be 32 bytes, got {}", bytes.len()))?;
    Ed25519VerifyingKey::from_bytes(&arr).map_err(|e| anyhow!("Invalid Ed25519 public key: {}", e))
}

/// Parse a raw 64-byte Ed25519 signature, as carried on the wire.
pub fn ed25519_signature_from_bytes(bytes: &[u8]) -> Result<Signature> {
    let arr: [u8; 64] = bytes
        .try_into()
        .map_err(|_| anyhow!("Ed25519 signature must be 64 bytes, got {}", bytes.len()))?;
    Ok(Signature::from_bytes(&arr))
}

/// Export Ed25519 public key to hex format (32 bytes)
pub fn ed25519_public_to_hex(vkey: &Ed25519VerifyingKey) -> String {
    hex::encode(vkey.to_bytes())
}

/// Import Ed25519 public key from hex format
pub fn ed25519_public_from_hex(hex_str: &str) -> Result<Ed25519VerifyingKey> {
    let bytes = hex::decode(hex_str).map_err(|e| anyhow!("Hex decode failed: {}", e))?;
    ed25519_public_from_bytes(&bytes)
}

/// Export Ed25519 private key to hex format (32 bytes) - CAREFUL: SENSITIVE DATA
pub fn ed25519_private_to_hex(skey: &Ed25519SigningKey) -> String {
    hex::encode(skey.to_bytes())
}

/// Import Ed25519 private key from hex format
pub fn ed25519_private_from_hex(hex_str: &str) -> Result<Ed25519SigningKey> {
    let bytes = hex::decode(hex_str).map_err(|e| anyhow!("Hex decode failed: {}", e))?;
    let key_bytes: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("Ed25519 private key must be 32 bytes, got {}", bytes.len()))?;
    Ok(Ed25519SigningKey::from_bytes(&key_bytes))
}

/// Pick the best scheme both sides support: Ed25519, then RSA-PSS, else `None`.
/// Both lists hold [`SignatureScheme`] wire values.
pub fn negotiate_signature_scheme(
    our_schemes: &[u8],
    their_schemes: &[u8],
) -> Option<SignatureScheme> {
    for &preferred in &[2u8, 1u8] {
        if our_schemes.contains(&preferred) && their_schemes.contains(&preferred) {
            return SignatureScheme::from_u8(preferred);
        }
    }
    None
}

/// Generate ephemeral X25519 keypair for forward secrecy
pub fn generate_ephemeral_keypair() -> (EphemeralSecret, X25519PublicKey) {
    let secret = EphemeralSecret::random();
    let public = X25519PublicKey::from(&secret);
    (secret, public)
}

/// ECDH key agreement, then HKDF-SHA256 (`salt`: the transcript hash) to an AES-256 key.
///
/// Rejects non-contributory exchanges: a low-order (or all-zero) peer key collapses
/// the shared secret to a predictable value (RFC 7748 §6.1).
pub fn derive_session_key(
    our_secret: EphemeralSecret,
    their_public: &X25519PublicKey,
    salt: Option<&[u8]>,
    info: &[u8],
) -> Result<[u8; AES_KEY_SIZE]> {
    let shared_secret = our_secret.diffie_hellman(their_public);
    if !shared_secret.was_contributory() {
        return Err(anyhow!(
            "peer sent a low-order X25519 public key (non-contributory key exchange refused)"
        ));
    }

    let hkdf = Hkdf::<Sha256>::new(salt, shared_secret.as_bytes());
    let mut session_key = [0u8; AES_KEY_SIZE];
    hkdf.expand(info, &mut session_key)
        .expect("HKDF expand should not fail with valid length");

    Ok(session_key)
}

/// Derive the next session key from the current one and a random nonce (HKDF-SHA256,
/// nonce as salt) for periodic key rotation.
pub fn rekey_session_key(current_key: &[u8; AES_KEY_SIZE], nonce: &[u8; 16]) -> [u8; AES_KEY_SIZE] {
    let hkdf = Hkdf::<Sha256>::new(Some(nonce), current_key);

    let mut next_key = [0u8; AES_KEY_SIZE];
    hkdf.expand(b"key-rotation", &mut next_key)
        .expect("HKDF expand should not fail with valid length");

    next_key
}

/// A random 16-byte nonce for rekeying.
pub fn generate_rekey_nonce() -> [u8; 16] {
    rand::random()
}

/// Parse X25519 public key from 32 bytes.
///
/// Rejects the all-zero key outright; other low-order points are caught at agreement
/// time by [`derive_session_key`]'s contributory check.
pub fn parse_x25519_public(bytes: &[u8]) -> Result<X25519PublicKey> {
    let key_bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow!("X25519 public key must be 32 bytes, got {}", bytes.len()))?;
    if key_bytes == [0u8; 32] {
        return Err(anyhow!("X25519 public key must not be all zeros"));
    }
    Ok(X25519PublicKey::from(key_bytes))
}

/// 32 easy-to-name emoji for the SAS, indexed by 5-bit groups of the SAS material.
/// Part of the SAS format: never reorder or replace entries.
const SAS_EMOJI: [&str; 32] = [
    "🐶", "🐱", "🦊", "🐻", "🐼", "🦁", "🐮", "🐷", "🐸", "🐵", "🐔", "🐧", "🦉", "🦄", "🐝", "🦋",
    "🐢", "🐙", "🐬", "🐳", "🌹", "🌻", "🌵", "🍀", "🍎", "🍋", "🍇", "🍓", "🥕", "🍕", "🎈", "⚽",
];

/// The Short Authentication String both peers display during verification: six digits
/// plus three emoji (~35 bits), derived from the transcript material via HKDF. A MITM
/// runs two handshakes with different transcripts, so the victims see different codes.
///
/// One guess per connection attempt holds only with the v4 ephemeral-key commitment
/// (`network::session::ephemeral_commitment`); a v3 handshake has none and the session
/// warns that the code is weak (audit CRYPTO-02).
pub fn derive_sas(transcript_material: &[u8]) -> String {
    let hkdf = Hkdf::<Sha256>::new(None, transcript_material);
    let mut out = [0u8; 5];
    hkdf.expand(b"p2pem-sas-v1", &mut out)
        .expect("HKDF expand should not fail with valid length");

    let digits = u32::from_be_bytes([0, out[0], out[1], out[2]]) % 1_000_000;
    let e1 = SAS_EMOJI[(out[3] & 0x1f) as usize];
    let e2 = SAS_EMOJI[((out[3] >> 5) | ((out[4] & 0x03) << 3)) as usize];
    let e3 = SAS_EMOJI[((out[4] >> 2) & 0x1f) as usize];
    format!(
        "{:02}-{:02}-{:02} {} {} {}",
        digits / 10_000,
        (digits / 100) % 100,
        digits % 100,
        e1,
        e2,
        e3
    )
}

/// Which side of a session a cipher belongs to. Both peers derive the same AES-GCM key,
/// so the nonce's high bit encodes the role and the two sides use disjoint nonce spaces
/// even if their random session ids coincide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NonceRole {
    /// The connection initiator's listener side (`new` defaults to this).
    Host,
    /// The connecting side.
    Client,
}

impl NonceRole {
    /// The high bit of the first nonce byte reserved for this role.
    fn nonce_bit(self) -> u8 {
        match self {
            NonceRole::Host => 0x00,
            NonceRole::Client => 0x80,
        }
    }
}

/// AES-GCM cipher with counter-based nonces: session_id(4) || counter(8).
#[derive(Clone)]
pub struct AesCipher {
    cipher: Aes256Gcm,
    /// Retained for rekeying; zeroized on drop.
    key: zeroize::Zeroizing<[u8; AES_KEY_SIZE]>,
    nonce_counter: std::sync::Arc<std::sync::atomic::AtomicU64>,
    session_id: [u8; 4],
    role: NonceRole,
}

impl AesCipher {
    /// A cipher in the default [`NonceRole::Host`] role with a random session id. Use
    /// [`AesCipher::new_with_role`] so the two peers get disjoint nonce spaces.
    pub fn new(key: &[u8]) -> Result<Self> {
        Self::new_with_role(key, NonceRole::Host)
    }

    /// A cipher for a specific session role (encoded in the nonce's high bit).
    pub fn new_with_role(key: &[u8], role: NonceRole) -> Result<Self> {
        if key.len() != AES_KEY_SIZE {
            return Err(anyhow!(
                "AES key must be {} bytes, got {}",
                AES_KEY_SIZE,
                key.len()
            ));
        }

        let mut session_id = [0u8; 4];
        OsRng.fill_bytes(&mut session_id);
        // Reserve the nonce's high bit for the role: host and client share one key, and
        // equal random session ids (~2^-32) would otherwise reuse a key+nonce pair.
        session_id[0] = (session_id[0] & 0x7f) | role.nonce_bit();

        let mut key_array = [0u8; AES_KEY_SIZE];
        key_array.copy_from_slice(key);

        Ok(Self {
            cipher: Aes256Gcm::new_from_slice(key)
                .map_err(|e| anyhow!("Invalid AES key length: {}", e))?,
            key: zeroize::Zeroizing::new(key_array),
            nonce_counter: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            session_id,
            role,
        })
    }

    /// The session role this cipher encrypts under (preserved across rekeys).
    pub fn role(&self) -> NonceRole {
        self.role
    }

    /// The current session key (for rekeying).
    pub fn get_current_key(&self) -> [u8; AES_KEY_SIZE] {
        *self.key
    }

    /// Encrypt with optional AAD; returns nonce(12) || ciphertext || tag(16).
    pub fn encrypt(&self, plaintext: &[u8], aad: Option<&[u8]>) -> Vec<u8> {
        // Reserve the next counter value without wraparound. A manual CAS loop:
        // `fetch_update` is deprecated and `try_update` is newer than our MSRV.
        use std::sync::atomic::Ordering::SeqCst;
        let mut counter = self.nonce_counter.load(SeqCst);
        loop {
            let next = counter
                .checked_add(1)
                .expect("AES-GCM nonce counter exhausted; rekey required");
            match self
                .nonce_counter
                .compare_exchange_weak(counter, next, SeqCst, SeqCst)
            {
                Ok(_) => break,
                Err(actual) => counter = actual,
            }
        }

        let mut nonce_bytes = [0u8; crate::AES_NONCE_SIZE];
        nonce_bytes[0..4].copy_from_slice(&self.session_id);
        nonce_bytes[4..crate::AES_NONCE_SIZE].copy_from_slice(&counter.to_be_bytes());

        let nonce = Nonce::from(nonce_bytes);
        let payload = Payload {
            msg: plaintext,
            aad: aad.unwrap_or(b""),
        };

        let ciphertext = self
            .cipher
            .encrypt(&nonce, payload)
            .expect("AES-GCM encryption should not fail");

        let mut output = Vec::with_capacity(crate::AES_NONCE_SIZE + ciphertext.len());
        output.extend_from_slice(&nonce_bytes);
        output.extend_from_slice(&ciphertext);
        output
    }

    /// Decrypt a frame that must have been sent by the **other** side. Both directions
    /// share one key and AAD and only the nonce's role bit tells them apart, so without
    /// this a frame of ours echoed back on the path authenticated as the peer's (audit
    /// NET-01). Every tunnel receive goes through this.
    pub fn decrypt_from_peer(&self, payload: &[u8], aad: Option<&[u8]>) -> Option<Vec<u8>> {
        let first = *payload.first()?;
        if first & 0x80 == self.role.nonce_bit() {
            return None;
        }
        self.decrypt(payload, aad)
    }

    /// Decrypt nonce || ciphertext || tag with optional AAD (must match encryption).
    pub fn decrypt(&self, payload: &[u8], aad: Option<&[u8]>) -> Option<Vec<u8>> {
        if payload.len() < crate::AES_NONCE_SIZE + crate::AES_GCM_TAG_SIZE {
            return None;
        }

        let (nonce_bytes, ciphertext) = payload.split_at(crate::AES_NONCE_SIZE);
        let nonce_arr: [u8; crate::AES_NONCE_SIZE] = nonce_bytes.try_into().ok()?;
        let aead_payload = Payload {
            msg: ciphertext,
            aad: aad.unwrap_or(b""),
        };
        self.cipher
            .decrypt(&Nonce::from(nonce_arr), aead_payload)
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn random_cipher() -> AesCipher {
        AesCipher::new(&rand::random::<[u8; 32]>()).unwrap()
    }

    fn rsa_public() -> RsaPublicKey {
        RsaPublicKey::from(&generate_rsa_keypair(RSA_KEY_BITS).unwrap())
    }

    /// Both peers derive the SAME key, so nonce uniqueness across directions must not
    /// rely on the random session ids differing: the role sits in the high bit.
    #[test]
    fn host_and_client_nonces_live_in_disjoint_spaces() {
        let key = [9u8; AES_KEY_SIZE];
        for _ in 0..64 {
            let host = AesCipher::new_with_role(&key, NonceRole::Host).unwrap();
            let client = AesCipher::new_with_role(&key, NonceRole::Client).unwrap();
            let host_nonce = host.encrypt(b"x", None);
            let client_nonce = client.encrypt(b"x", None);
            assert_eq!(host_nonce[0] & 0x80, 0x00, "host nonce high bit must be 0");
            assert_eq!(
                client_nonce[0] & 0x80,
                0x80,
                "client nonce high bit must be 1"
            );
        }
    }

    #[test]
    fn test_aes_roundtrip() {
        let cipher = random_cipher();
        for aad in [None, Some(&b"additional_context"[..])] {
            let encrypted = cipher.encrypt(b"Hello, secure world!", aad);
            assert_eq!(
                cipher.decrypt(&encrypted, aad).unwrap(),
                b"Hello, secure world!"
            );
        }
    }

    #[test]
    fn test_aes_aad_mismatch_or_stripped_fails() {
        let cipher = random_cipher();
        let encrypted = cipher.encrypt(b"Secret message", Some(b"original_context"));
        assert!(cipher.decrypt(&encrypted, Some(b"wrong_context")).is_none());
        assert!(cipher.decrypt(&encrypted, None).is_none());
    }

    #[test]
    fn test_aes_nonce_randomness() {
        let cipher = random_cipher();
        let enc1 = cipher.encrypt(b"Same message", None);
        let enc2 = cipher.encrypt(b"Same message", None);
        assert_ne!(enc1, enc2);
        assert_eq!(cipher.decrypt(&enc1, None).unwrap(), b"Same message");
        assert_eq!(cipher.decrypt(&enc2, None).unwrap(), b"Same message");
    }

    #[test]
    fn test_aes_tamper_detection() {
        let cipher = random_cipher();
        let mut encrypted = cipher.encrypt(b"Test", None);
        encrypted[20] ^= 1;
        assert!(cipher.decrypt(&encrypted, None).is_none());
    }

    #[test]
    #[should_panic(expected = "AES-GCM nonce counter exhausted; rekey required")]
    fn test_aes_nonce_counter_exhaustion_panics() {
        let cipher = random_cipher();
        cipher
            .nonce_counter
            .store(u64::MAX, std::sync::atomic::Ordering::SeqCst);
        let _ = cipher.encrypt(b"nonce exhaustion", None);
    }

    /// The decoded key verifies a PSS signature made with the private key (PSS is the
    /// only RSA operation the product uses).
    #[test]
    fn test_rsa_pem_roundtrip() {
        let privkey = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let pubkey = RsaPublicKey::from(&privkey);
        let decoded = pem_decode_public(&pem_encode_public(&pubkey).unwrap()).unwrap();
        let sig = rsa_sign_pss(&privkey, b"pem roundtrip").unwrap();
        assert!(rsa_verify_pss(&pubkey, b"pem roundtrip", &sig).is_ok());
        assert!(rsa_verify_pss(&decoded, b"pem roundtrip", &sig).is_ok());
    }

    #[test]
    fn test_fingerprint_consistency() {
        let pem = pem_encode_public(&rsa_public()).unwrap();
        let fp1 = fingerprint_pubkey(pem.as_bytes()).unwrap();
        assert_eq!(fp1, fingerprint_pubkey(pem.as_bytes()).unwrap());
        assert_eq!(fp1.len(), 64);
    }

    #[test]
    fn test_ephemeral_keypair_generation() {
        let (_, public1) = generate_ephemeral_keypair();
        let (_, public2) = generate_ephemeral_keypair();
        assert_ne!(public1.as_bytes(), public2.as_bytes());
        assert_eq!(public1.as_bytes().len(), 32);
    }

    #[test]
    fn test_ecdh_different_secrets_give_different_keys() {
        let (alice, _) = generate_ephemeral_keypair();
        let (alice2, _) = generate_ephemeral_keypair();
        let (_, bob_public) = generate_ephemeral_keypair();
        let key1 = derive_session_key(alice, &bob_public, None, b"context1").unwrap();
        let key2 = derive_session_key(alice2, &bob_public, None, b"context2").unwrap();
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_x25519_public_key_parsing() {
        let (_, public) = generate_ephemeral_keypair();
        let parsed = parse_x25519_public(public.as_bytes()).unwrap();
        assert_eq!(parsed.as_bytes(), public.as_bytes());
        assert!(parse_x25519_public(&[0u8; 16]).is_err());
        assert!(parse_x25519_public(&[0u8; 32]).is_err());
    }

    #[test]
    fn test_sas_is_deterministic_and_transcript_bound() {
        let a = derive_sas(b"transcript-one");
        assert_eq!(a, derive_sas(b"transcript-one"));
        assert_ne!(a, derive_sas(b"transcript-two"));
        // Format: "NN-NN-NN" then three emoji.
        let digits = a.split(' ').next().unwrap();
        assert_eq!(digits.len(), 8);
        assert_eq!(digits.matches('-').count(), 2);
        assert_eq!(a.split(' ').count(), 4);
    }

    /// Both peers render the SAS independently, so the derivation and emoji table
    /// must never change.
    #[test]
    fn test_sas_known_answer_is_stable() {
        assert_eq!(derive_sas(b"p2pem-test-vector"), "20-19-64 🌻 🍎 🐔");
    }

    /// Canonical order-8 point (libsodium's blacklist): it parses, but DH against it
    /// collapses to the identity, so agreement must fail.
    #[test]
    fn test_x25519_rejects_low_order_public_key() {
        let low_order: [u8; 32] = [
            0xe0, 0xeb, 0x7a, 0x7c, 0x3b, 0x41, 0xb8, 0xae, 0x16, 0x56, 0xe3, 0xfa, 0xf1, 0x9f,
            0xc4, 0x6a, 0xda, 0x09, 0x8d, 0xeb, 0x9c, 0x32, 0xb1, 0xfd, 0x86, 0x62, 0x05, 0x16,
            0x5f, 0x49, 0xb8, 0x00,
        ];
        let malicious = parse_x25519_public(&low_order).expect("parses; caught at agreement");
        let (our_secret, _) = generate_ephemeral_keypair();
        assert!(derive_session_key(our_secret, &malicious, None, b"test").is_err());
    }

    /// Both sides derive the same key from parsed wire bytes and can talk with it.
    #[test]
    fn test_forward_secrecy_full_flow() {
        let (alice_secret, alice_public) = generate_ephemeral_keypair();
        let (bob_secret, bob_public) = generate_ephemeral_keypair();
        let bob_parsed = parse_x25519_public(bob_public.as_bytes()).unwrap();
        let alice_parsed = parse_x25519_public(alice_public.as_bytes()).unwrap();

        let info = b"p2p-messenger-v2";
        let alice_key = derive_session_key(alice_secret, &bob_parsed, None, info).unwrap();
        let bob_key = derive_session_key(bob_secret, &alice_parsed, None, info).unwrap();
        assert_eq!(alice_key, bob_key);
        assert_eq!(alice_key.len(), AES_KEY_SIZE);

        let encrypted = AesCipher::new(&alice_key)
            .unwrap()
            .encrypt(b"Forward secrecy test message", None);
        let decrypted = AesCipher::new(&bob_key)
            .unwrap()
            .decrypt(&encrypted, None)
            .unwrap();
        assert_eq!(decrypted, b"Forward secrecy test message");
    }

    #[test]
    fn test_rekey_derivation() {
        let current_key: [u8; AES_KEY_SIZE] = rand::random();
        let nonce1 = generate_rekey_nonce();
        let nonce2 = generate_rekey_nonce();
        assert_ne!(nonce1, nonce2);

        let next = rekey_session_key(&current_key, &nonce1);
        assert_ne!(next, current_key);
        assert_eq!(next, rekey_session_key(&current_key, &nonce1));
        assert_ne!(next, rekey_session_key(&current_key, &nonce2));
    }

    /// Rekeying yields a working cipher, and the old one still opens its own frames.
    #[test]
    fn test_rekey_encryption_flow() {
        let current_key: [u8; AES_KEY_SIZE] = rand::random();
        let cipher1 = AesCipher::new(&current_key).unwrap();
        let encrypted = cipher1.encrypt(b"Message before rekeying", None);
        assert_eq!(
            cipher1.decrypt(&encrypted, None).unwrap(),
            b"Message before rekeying"
        );

        let next_key = rekey_session_key(&current_key, &generate_rekey_nonce());
        let cipher2 = AesCipher::new(&next_key).unwrap();
        let encrypted2 = cipher2.encrypt(b"Message after rekeying", None);
        assert_eq!(
            cipher2.decrypt(&encrypted2, None).unwrap(),
            b"Message after rekeying"
        );
        assert_eq!(
            cipher1.decrypt(&encrypted, None).unwrap(),
            b"Message before rekeying"
        );
    }

    #[test]
    fn test_ed25519_keypair_generation() {
        let (skey1, vkey1) = generate_ed25519_keypair();
        let (skey2, vkey2) = generate_ed25519_keypair();
        assert_ne!(skey1.to_bytes(), skey2.to_bytes());
        assert_ne!(vkey1.to_bytes(), vkey2.to_bytes());
    }

    /// Deterministic per identity, so an existing identity gains a subkey without any
    /// change to `identity.json`.
    #[test]
    fn ed25519_subkey_is_deterministic_per_identity() {
        let a = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let b = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let public = |k: &RsaPrivateKey| derive_ed25519_subkey(k).unwrap().verifying_key();
        assert_eq!(
            public(&a),
            public(&a),
            "the same identity must always derive the same subkey"
        );
        assert_ne!(
            public(&a),
            public(&b),
            "different identities must derive different subkeys"
        );
    }

    #[test]
    fn the_binding_ties_a_subkey_to_exactly_one_identity() {
        let identity = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let other = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let rsa = RsaPublicKey::from(&identity);
        let public = derive_ed25519_subkey(&identity).unwrap().verifying_key();
        let binding = cross_sign_ed25519(&identity, &public).unwrap();

        assert!(verify_ed25519_binding(&rsa, &public, &binding).is_ok());
        assert!(
            verify_ed25519_binding(&RsaPublicKey::from(&other), &public, &binding).is_err(),
            "a binding must not verify under an identity that did not make it"
        );
        // The attack it exists to stop: a stolen PEM with an attacker's signing key.
        let attacker = derive_ed25519_subkey(&other).unwrap().verifying_key();
        assert!(
            verify_ed25519_binding(&rsa, &attacker, &binding).is_err(),
            "a binding must not transfer to another subkey"
        );
        let mut damaged = binding.clone();
        damaged[0] ^= 0xff;
        assert!(verify_ed25519_binding(&rsa, &public, &damaged).is_err());
    }

    /// Every contact's TOFU trust is pinned to the fingerprint.
    #[test]
    fn deriving_a_subkey_does_not_change_the_identity_fingerprint() {
        let identity = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let pem = pem_encode_public(&RsaPublicKey::from(&identity)).unwrap();
        let before = fingerprint_pubkey(pem.as_bytes()).unwrap();
        let _ = derive_ed25519_subkey(&identity).unwrap();
        assert_eq!(before, fingerprint_pubkey(pem.as_bytes()).unwrap());
    }

    /// Ed25519 is preferred whatever the order; an RSA-only (older) peer still
    /// negotiates; no overlap is `None`.
    #[test]
    fn test_signature_scheme_negotiation() {
        let (ed, rsa) = (
            SignatureScheme::Ed25519.to_u8(),
            SignatureScheme::RsaPss.to_u8(),
        );
        let cases: [(&[u8], &[u8], Option<SignatureScheme>); 6] = [
            (&[ed, rsa], &[ed, rsa], Some(SignatureScheme::Ed25519)),
            (&[rsa, ed], &[ed, rsa], Some(SignatureScheme::Ed25519)),
            (&[ed], &[rsa, ed], Some(SignatureScheme::Ed25519)),
            (&[ed, rsa], &[rsa], Some(SignatureScheme::RsaPss)),
            (&[ed], &[rsa], None),
            (&[ed, rsa], &[], None),
        ];
        for (ours, theirs, expected) in cases {
            assert_eq!(
                negotiate_signature_scheme(ours, theirs),
                expected,
                "{ours:?} vs {theirs:?}"
            );
        }
    }

    #[test]
    fn test_ed25519_sign_verify() {
        let (signing_key, verifying_key) = generate_ed25519_keypair();
        let (_, other_key) = generate_ed25519_keypair();
        let data = [&b"IDENTITY_PROOF"[..], &[42u8; 32]].concat();
        let signature = sign_ed25519(&signing_key, &data);
        assert!(verify_ed25519(&verifying_key, &data, &signature).is_ok());

        let wrong_data = [&b"IDENTITY_PROOF"[..], &[99u8; 32]].concat();
        assert!(verify_ed25519(&verifying_key, &wrong_data, &signature).is_err());
        assert!(verify_ed25519(&other_key, &data, &signature).is_err());
    }

    #[test]
    fn test_ed25519_hex_roundtrip() {
        let (_, vkey_orig) = generate_ed25519_keypair();
        let hex_str = ed25519_public_to_hex(&vkey_orig);
        assert_eq!(hex_str.len(), 64);
        assert_eq!(ed25519_public_from_hex(&hex_str).unwrap(), vkey_orig);
    }

    #[test]
    fn test_signature_scheme_enum() {
        for (scheme, value, name) in [
            (SignatureScheme::RsaPss, 1, "RSA-PSS"),
            (SignatureScheme::Ed25519, 2, "Ed25519"),
        ] {
            assert_eq!(scheme.to_u8(), value);
            assert_eq!(scheme.name(), name);
            assert_eq!(scheme.to_string(), name);
            assert_eq!(SignatureScheme::from_u8(value), Some(scheme));
        }
        assert_eq!(SignatureScheme::from_u8(99), None);
    }

    #[test]
    fn test_supported_signature_schemes_message() {
        use crate::core::ProtocolMessage;
        let schemes = vec![
            SignatureScheme::RsaPss.to_u8(),
            SignatureScheme::Ed25519.to_u8(),
        ];
        let msg = ProtocolMessage::SupportedSignatureSchemes {
            schemes: schemes.clone(),
        };
        match ProtocolMessage::from_plain_bytes(&msg.to_plain_bytes()) {
            Some(ProtocolMessage::SupportedSignatureSchemes { schemes: decoded }) => {
                assert_eq!(decoded, schemes)
            }
            other => panic!("Expected SupportedSignatureSchemes message, got {other:?}"),
        }
    }

    /// One key hidden in an RFC 7468 preamble and another in the block proper: a
    /// fingerprint read from the first with signatures checked against the second
    /// would let a peer present a victim's fingerprint.
    #[test]
    fn a_pem_with_a_second_key_in_its_preamble_is_refused() {
        let crafted = format!(
            "x{}\n{}",
            pem_encode_public(&rsa_public()).unwrap().trim_end(),
            pem_encode_public(&rsa_public()).unwrap()
        );
        assert!(pem_decode_public(&crafted).is_err());
        assert!(fingerprint_pubkey(crafted.as_bytes()).is_err());
    }

    #[test]
    fn text_around_a_single_pem_block_is_refused() {
        let key = rsa_public();
        // Normalise the platform line ending so the CRLF case is not `\r\r\n`.
        let pem = pem_encode_public(&key).unwrap().replace("\r\n", "\n");
        assert!(pem_decode_public(&format!("comment\n{pem}")).is_err());
        assert!(pem_decode_public(&format!("{pem}trailing")).is_err());
        // Surrounding whitespace and CRLF line endings are not an attack.
        assert_eq!(pem_decode_public(&format!("  \n{pem}\n\n")).unwrap(), key);
        assert_eq!(pem_decode_public(&pem.replace('\n', "\r\n")).unwrap(), key);
    }

    /// An END marker before BEGIN (e.g. a pasted v1 invite) is an error, not a panic.
    #[test]
    fn markers_in_the_wrong_order_are_an_error_not_a_panic() {
        let s = "-----END PUBLIC KEY----------BEGIN PUBLIC KEY-----";
        assert!(fingerprint_pubkey(s.as_bytes()).is_err());
        assert!(fingerprint_pubkey(b"not a key at all").is_err());
        assert!(fingerprint_pubkey(&[0xff, 0xfe]).is_err());
    }

    /// Stored fingerprints must not move: for an honest key the parsed-key fingerprint
    /// equals the hash of the PEM's base64 body.
    #[test]
    fn honest_fingerprints_are_unchanged() {
        use base64::Engine;
        for _ in 0..3 {
            let key = rsa_public();
            let pem = pem_encode_public(&key).unwrap();
            let body: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
            let der = base64::engine::general_purpose::STANDARD
                .decode(body)
                .unwrap();
            let legacy = hex::encode(Sha256::digest(&der));
            assert_eq!(fingerprint_pubkey(pem.as_bytes()).unwrap(), legacy);
            assert_eq!(fingerprint_public_key(&key).unwrap(), legacy);
        }
    }
}
