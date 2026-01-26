use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use anyhow::{anyhow, Result};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use ed25519_dalek::{
    Signature, Signer, SigningKey as Ed25519SigningKey, VerifyingKey as Ed25519VerifyingKey,
};
use hkdf::Hkdf;
use rand::{rngs::OsRng, RngCore};
use rsa::{
    pkcs1::{DecodeRsaPrivateKey, EncodeRsaPrivateKey},
    pkcs8::{DecodePublicKey, EncodePublicKey},
    pss::{SigningKey, VerifyingKey},
    signature::{RandomizedSigner, SignatureEncoding},
    Oaep, RsaPrivateKey, RsaPublicKey,
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

/// Import RSA public key from PEM format
pub fn pem_decode_public(pem: &str) -> Result<RsaPublicKey> {
    RsaPublicKey::from_public_key_pem(pem).map_err(|e| anyhow!("PEM decode failed: {}", e))
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

/// Encrypt data using RSA-OAEP with SHA-256
pub fn rsa_encrypt_oaep(pubkey: &RsaPublicKey, plaintext: &[u8]) -> Result<Vec<u8>> {
    let padding = Oaep::new::<Sha256>();
    pubkey
        .encrypt(&mut OsRng, padding, plaintext)
        .map_err(|e| anyhow!("RSA encryption failed: {}", e))
}

/// Decrypt data using RSA-OAEP with SHA-256
pub fn rsa_decrypt_oaep(privkey: &RsaPrivateKey, ciphertext: &[u8]) -> Result<Vec<u8>> {
    let padding = Oaep::new::<Sha256>();
    privkey
        .decrypt(padding, ciphertext)
        .map_err(|e| anyhow!("RSA decryption failed: {}", e))
}

/// Calculate SHA-256 fingerprint of public key PEM
pub fn fingerprint_pubkey(pem_bytes: &[u8]) -> String {
    // Try to extract DER bytes from PEM and hash the canonical DER (SubjectPublicKeyInfo)
    if let Ok(s) = std::str::from_utf8(pem_bytes) {
        if let (Some(begin), Some(end)) = (
            s.find("-----BEGIN PUBLIC KEY-----"),
            s.find("-----END PUBLIC KEY-----"),
        ) {
            let body = &s[begin + "-----BEGIN PUBLIC KEY-----".len()..end];
            // Remove whitespace/newlines from PEM body
            let b64: String = body.chars().filter(|c| !c.is_whitespace()).collect();
            if let Ok(der) = BASE64_STANDARD.decode(&b64) {
                let hash = Sha256::digest(&der);
                return hex::encode(hash);
            }
        }
    }

    // Fallback: hash raw bytes
    let hash = Sha256::digest(pem_bytes);
    hex::encode(hash)
}

// ============================================================================
// RSA-PSS Signing for Invite Links and Other Integrity-Protected Data
// ============================================================================

/// Sign data using RSA-PSS with SHA-256
///
/// Uses randomized signing for each call (security best practice)
///
/// # Arguments
/// * `privkey` - RSA private key
/// * `data` - Data to sign
///
/// # Returns
/// Raw RSA-PSS signature bytes
pub fn rsa_sign_pss(privkey: &RsaPrivateKey, data: &[u8]) -> Result<Vec<u8>> {
    let signing_key = SigningKey::<Sha256>::new(privkey.clone());
    let mut rng = OsRng;

    // Sign with randomized signing for enhanced security
    // SigningKey::<Sha256>::sign_with_rng automatically applies SHA-256 hashing
    let signature = signing_key
        .sign_with_rng(&mut rng, data)
        .to_bytes()
        .to_vec();

    Ok(signature)
}

/// Verify RSA-PSS signature with SHA-256
///
/// # Arguments
/// * `pubkey` - RSA public key
/// * `data` - Original data that was signed
/// * `signature` - Raw RSA-PSS signature bytes
///
/// # Returns
/// Ok(()) if signature is valid, Err if invalid
pub fn rsa_verify_pss(pubkey: &RsaPublicKey, data: &[u8], signature: &[u8]) -> Result<()> {
    use rsa::signature::Verifier as RsaVerifier;

    let verifying_key = VerifyingKey::<Sha256>::new(pubkey.clone());

    // Convert byte slice to RSA Signature type
    let rsa_signature = rsa::pss::Signature::try_from(signature)
        .map_err(|e| anyhow!("Failed to parse RSA signature: {}", e))?;

    // Verify signature
    // VerifyingKey::<Sha256>::verify automatically applies SHA-256 hashing
    verifying_key
        .verify(data, &rsa_signature)
        .map_err(|e| anyhow!("RSA-PSS signature verification failed: {}", e))
}

// ============================================================================
// Ed25519 EDDSA Signatures for Modern Key Exchanges
// ============================================================================

/// Signature scheme for identity proofs
/// Determines which algorithm is used to sign the ephemeral key during handshake
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SignatureScheme {
    /// RSA-PSS with SHA-256 (current default, will be deprecated)
    RsaPss = 1,
    /// Ed25519 EdDSA (recommended, future default)
    Ed25519 = 2,
}

impl SignatureScheme {
    /// Get the numeric value for wire protocol
    pub fn to_u8(self) -> u8 {
        self as u8
    }

    /// Parse from numeric value
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(SignatureScheme::RsaPss),
            2 => Some(SignatureScheme::Ed25519),
            _ => None,
        }
    }

    /// Human-readable name
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

/// Export Ed25519 public key to hex format (32 bytes)
pub fn ed25519_public_to_hex(vkey: &Ed25519VerifyingKey) -> String {
    hex::encode(vkey.to_bytes())
}

/// Import Ed25519 public key from hex format
pub fn ed25519_public_from_hex(hex_str: &str) -> Result<Ed25519VerifyingKey> {
    let bytes = hex::decode(hex_str).map_err(|e| anyhow!("Hex decode failed: {}", e))?;
    if bytes.len() != 32 {
        return Err(anyhow!(
            "Ed25519 public key must be 32 bytes, got {}",
            bytes.len()
        ));
    }
    let mut key_bytes = [0u8; 32];
    key_bytes.copy_from_slice(&bytes);
    Ed25519VerifyingKey::from_bytes(&key_bytes)
        .map_err(|e| anyhow!("Invalid Ed25519 public key: {}", e))
}

/// Export Ed25519 private key to hex format (32 bytes) - CAREFUL: SENSITIVE DATA
pub fn ed25519_private_to_hex(skey: &Ed25519SigningKey) -> String {
    hex::encode(skey.to_bytes())
}

/// Import Ed25519 private key from hex format
pub fn ed25519_private_from_hex(hex_str: &str) -> Result<Ed25519SigningKey> {
    let bytes = hex::decode(hex_str).map_err(|e| anyhow!("Hex decode failed: {}", e))?;
    if bytes.len() != 32 {
        return Err(anyhow!(
            "Ed25519 private key must be 32 bytes, got {}",
            bytes.len()
        ));
    }
    let mut key_bytes = [0u8; 32];
    key_bytes.copy_from_slice(&bytes);
    Ok(Ed25519SigningKey::from_bytes(&key_bytes))
}

/// Negotiate signature scheme from lists of supported schemes
///
/// Selects the highest priority common scheme:
/// - Returns Ed25519 (2) if both support it (preferred)
/// - Returns RsaPss (1) if both support it (fallback)
/// - Returns None if no common scheme found
///
/// Both lists should contain u8 values from SignatureScheme enum (1 or 2)
pub fn negotiate_signature_scheme(
    our_schemes: &[u8],
    their_schemes: &[u8],
) -> Option<SignatureScheme> {
    // Prefer Ed25519 first, fall back to RsaPss
    for &preferred in &[2u8, 1u8] {
        if our_schemes.contains(&preferred) && their_schemes.contains(&preferred) {
            return SignatureScheme::from_u8(preferred);
        }
    }
    None
}

// X25519 ECDH for Forward Secrecy
// ============================================================================

/// Generate ephemeral X25519 keypair for forward secrecy
pub fn generate_ephemeral_keypair() -> (EphemeralSecret, X25519PublicKey) {
    let secret = EphemeralSecret::random_from_rng(OsRng);
    let public = X25519PublicKey::from(&secret);
    (secret, public)
}

/// Perform ECDH key agreement and derive AES key using HKDF-SHA256
///
/// # Arguments
/// * `our_secret` - Our ephemeral private key
/// * `their_public` - Their ephemeral public key
/// * `salt` - Optional salt (e.g., hash of handshake transcript)
/// * `info` - Context string for HKDF (e.g., "p2p-messenger-v2")
///
/// # Returns
/// 32-byte AES-256 key derived from shared secret
pub fn derive_session_key(
    our_secret: EphemeralSecret,
    their_public: &X25519PublicKey,
    salt: Option<&[u8]>,
    info: &[u8],
) -> [u8; AES_KEY_SIZE] {
    // Perform ECDH to get shared secret
    let shared_secret = our_secret.diffie_hellman(their_public);

    // Use HKDF-SHA256 to derive session key
    // Salt is now supported (and recommended to be transcript hash)
    let hkdf = Hkdf::<Sha256>::new(salt, shared_secret.as_bytes());

    let mut session_key = [0u8; AES_KEY_SIZE];
    hkdf.expand(info, &mut session_key)
        .expect("HKDF expand should not fail with valid length");

    session_key
}

/// Generate a new session key by rekeying the current key with a nonce
///
/// Used for periodic key rotation to provide perfect forward secrecy.
/// Derives next key from current key + random nonce using HKDF-SHA256.
///
/// # Arguments
/// * `current_key` - Current 32-byte session key
/// * `nonce` - Random 16-byte nonce (should be cryptographically random)
///
/// # Returns
/// New 32-byte session key
pub fn rekey_session_key(current_key: &[u8; AES_KEY_SIZE], nonce: &[u8; 16]) -> [u8; AES_KEY_SIZE] {
    // Use HKDF to derive next key from current key and nonce
    // Current key acts as IKM (Input Keying Material)
    // Nonce acts as salt for additional entropy
    let hkdf = Hkdf::<Sha256>::new(Some(nonce), current_key);

    let mut next_key = [0u8; AES_KEY_SIZE];
    hkdf.expand(b"key-rotation", &mut next_key)
        .expect("HKDF expand should not fail with valid length");

    next_key
}

/// Generate a cryptographically random nonce for rekeying
///
/// # Returns
/// 16-byte random nonce suitable for key rotation
pub fn generate_rekey_nonce() -> [u8; 16] {
    let mut nonce = [0u8; 16];
    OsRng.fill_bytes(&mut nonce);
    nonce
}

/// Parse X25519 public key from 32 bytes
pub fn parse_x25519_public(bytes: &[u8]) -> Result<X25519PublicKey> {
    if bytes.len() != 32 {
        return Err(anyhow!(
            "X25519 public key must be 32 bytes, got {}",
            bytes.len()
        ));
    }

    let mut key_bytes = [0u8; 32];
    key_bytes.copy_from_slice(bytes);
    Ok(X25519PublicKey::from(key_bytes))
}

/// AES-GCM cipher wrapper for encrypting/decrypting messages
/// Uses counter-based nonces for guaranteed uniqueness
#[derive(Clone)]
pub struct AesCipher {
    cipher: Aes256Gcm,
    key: [u8; AES_KEY_SIZE],
    nonce_counter: std::sync::Arc<std::sync::atomic::AtomicU64>,
    session_id: [u8; 4],
}

impl AesCipher {
    /// Create new cipher from 32-byte key with random session ID
    pub fn new(key: &[u8]) -> Result<Self> {
        if key.len() != AES_KEY_SIZE {
            return Err(anyhow!(
                "AES key must be {} bytes, got {}",
                AES_KEY_SIZE,
                key.len()
            ));
        }

        let mut session_id = [0u8; 4];
        // Use OS RNG explicitly for cryptographic session IDs
        OsRng.fill_bytes(&mut session_id);

        let mut key_array = [0u8; AES_KEY_SIZE];
        key_array.copy_from_slice(key);

        Ok(Self {
            cipher: Aes256Gcm::new_from_slice(key)
                .map_err(|e| anyhow!("Invalid AES key length: {}", e))?,
            key: key_array,
            nonce_counter: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            session_id,
        })
    }

    /// Get the current session key (for rekeying purposes)
    pub fn get_current_key(&self) -> [u8; AES_KEY_SIZE] {
        self.key
    }

    /// Encrypt plaintext with optional AAD, returns nonce(12) || ciphertext || tag(16)
    /// Uses counter-based nonce: session_id(4) || counter(8) for guaranteed uniqueness
    ///
    /// # Arguments
    /// * `plaintext` - Data to encrypt
    /// * `aad` - Optional Additional Authenticated Data (binds authenticity context)
    pub fn encrypt(&self, plaintext: &[u8], aad: Option<&[u8]>) -> Vec<u8> {
        // Get next counter value atomically
        let counter = self
            .nonce_counter
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // Build nonce: session_id (4 bytes) || counter (8 bytes)
        let mut nonce_bytes = [0u8; crate::AES_NONCE_SIZE];
        nonce_bytes[0..4].copy_from_slice(&self.session_id);
        nonce_bytes[4..crate::AES_NONCE_SIZE].copy_from_slice(&counter.to_be_bytes());

        let nonce = Nonce::from(nonce_bytes);

        // Use aead::Payload to handle AAD
        let payload = if let Some(aad_bytes) = aad {
            Payload {
                msg: plaintext,
                aad: aad_bytes,
            }
        } else {
            Payload {
                msg: plaintext,
                aad: b"",
            }
        };

        let ciphertext = self
            .cipher
            .encrypt(&nonce, payload)
            .expect("AES-GCM encryption should not fail");

        // Format: nonce || ciphertext (includes tag)
        let mut output = Vec::with_capacity(crate::AES_NONCE_SIZE + ciphertext.len());
        output.extend_from_slice(&nonce_bytes);
        output.extend_from_slice(&ciphertext);
        output
    }

    /// Decrypt payload with optional AAD: nonce(AES_NONCE_SIZE) || ciphertext || tag(AES_GCM_TAG_SIZE)
    ///
    /// # Arguments
    /// * `payload` - Encrypted data in format nonce || ciphertext
    /// * `aad` - Optional Additional Authenticated Data (must match encryption AAD)
    pub fn decrypt(&self, payload: &[u8], aad: Option<&[u8]>) -> Option<Vec<u8>> {
        if payload.len() < crate::AES_NONCE_SIZE + crate::AES_GCM_TAG_SIZE {
            return None; // Too small
        }

        let (nonce_bytes, ciphertext) = payload.split_at(crate::AES_NONCE_SIZE);
        // Convert nonce slice to array then to Nonce
        let nonce_arr: [u8; crate::AES_NONCE_SIZE] = match <[u8; crate::AES_NONCE_SIZE]>::try_from(nonce_bytes) {
            Ok(a) => a,
            Err(_) => return None,
        };

        let nonce = Nonce::from(nonce_arr);

        // Use aead::Payload to handle AAD
        let aead_payload = if let Some(aad_bytes) = aad {
            Payload {
                msg: ciphertext,
                aad: aad_bytes,
            }
        } else {
            Payload {
                msg: ciphertext,
                aad: b"",
            }
        };

        self.cipher.decrypt(&nonce, aead_payload).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;

    #[test]
    fn test_aes_roundtrip() {
        let key: [u8; 32] = rand::thread_rng().gen();
        let cipher = AesCipher::new(&key).unwrap();

        let plaintext = b"Hello, secure world!";
        let encrypted = cipher.encrypt(plaintext, None);
        let decrypted = cipher.decrypt(&encrypted, None).unwrap();

        assert_eq!(plaintext, &decrypted[..]);
    }

    #[test]
    fn test_aes_roundtrip_with_aad() {
        let key: [u8; 32] = rand::thread_rng().gen();
        let cipher = AesCipher::new(&key).unwrap();

        let plaintext = b"Hello with AAD";
        let aad = b"additional_context";
        let encrypted = cipher.encrypt(plaintext, Some(aad));
        let decrypted = cipher.decrypt(&encrypted, Some(aad)).unwrap();

        assert_eq!(plaintext, &decrypted[..]);
    }

    #[test]
    fn test_aes_aad_mismatch_fails() {
        let key: [u8; 32] = rand::thread_rng().gen();
        let cipher = AesCipher::new(&key).unwrap();

        let plaintext = b"Secret message";
        let aad = b"original_context";
        let encrypted = cipher.encrypt(plaintext, Some(aad));

        // Try to decrypt with different AAD
        let wrong_aad = b"wrong_context";
        let result = cipher.decrypt(&encrypted, Some(wrong_aad));

        // Should fail due to AAD mismatch
        assert!(result.is_none());
    }

    #[test]
    fn test_aes_aad_stripped_fails() {
        let key: [u8; 32] = rand::thread_rng().gen();
        let cipher = AesCipher::new(&key).unwrap();

        let plaintext = b"Secret message";
        let aad = b"required_context";
        let encrypted = cipher.encrypt(plaintext, Some(aad));

        // Try to decrypt without AAD when it was used
        let result = cipher.decrypt(&encrypted, None);

        // Should fail because AAD is not provided
        assert!(result.is_none());
    }

    #[test]
    fn test_aes_nonce_randomness() {
        let key: [u8; 32] = rand::thread_rng().gen();
        let cipher = AesCipher::new(&key).unwrap();

        let plaintext = b"Same message";
        let enc1 = cipher.encrypt(plaintext, None);
        let enc2 = cipher.encrypt(plaintext, None);

        // Ciphertexts should be different due to random nonces
        assert_ne!(enc1, enc2);

        // But both should decrypt correctly
        assert_eq!(cipher.decrypt(&enc1, None).unwrap(), plaintext);
        assert_eq!(cipher.decrypt(&enc2, None).unwrap(), plaintext);
    }

    #[test]
    fn test_aes_tamper_detection() {
        let key: [u8; 32] = rand::thread_rng().gen();
        let cipher = AesCipher::new(&key).unwrap();

        let mut encrypted = cipher.encrypt(b"Test", None);
        if encrypted.len() > 20 {
            encrypted[20] ^= 1; // Tamper with ciphertext
        }

        assert!(cipher.decrypt(&encrypted, None).is_none());
    }

    #[test]
    fn test_rsa_roundtrip() {
        let privkey = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let pubkey = RsaPublicKey::from(&privkey);

        let plaintext = b"Secret AES key";
        let encrypted = rsa_encrypt_oaep(&pubkey, plaintext).unwrap();
        let decrypted = rsa_decrypt_oaep(&privkey, &encrypted).unwrap();

        assert_eq!(plaintext, &decrypted[..]);
    }

    #[test]
    fn test_rsa_pem_roundtrip() {
        let privkey = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let pubkey = RsaPublicKey::from(&privkey);

        let pem = pem_encode_public(&pubkey).unwrap();
        let decoded = pem_decode_public(&pem).unwrap();

        // Keys should be functionally equivalent
        let plaintext = b"Test";
        let enc1 = rsa_encrypt_oaep(&pubkey, plaintext).unwrap();
        let enc2 = rsa_encrypt_oaep(&decoded, plaintext).unwrap();

        assert_eq!(
            rsa_decrypt_oaep(&privkey, &enc1).unwrap(),
            rsa_decrypt_oaep(&privkey, &enc2).unwrap()
        );
    }

    #[test]
    fn test_fingerprint_consistency() {
        let privkey = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let pubkey = RsaPublicKey::from(&privkey);
        let pem = pem_encode_public(&pubkey).unwrap();

        let fp1 = fingerprint_pubkey(pem.as_bytes());
        let fp2 = fingerprint_pubkey(pem.as_bytes());

        assert_eq!(fp1, fp2);
        assert_eq!(fp1.len(), 64); // SHA-256 = 32 bytes = 64 hex chars
    }

    #[test]
    fn test_ephemeral_keypair_generation() {
        let (_secret1, public1) = generate_ephemeral_keypair();
        let (_secret2, public2) = generate_ephemeral_keypair();

        // Keys should be different
        assert_ne!(public1.as_bytes(), public2.as_bytes());

        // Public keys should be 32 bytes
        assert_eq!(public1.as_bytes().len(), 32);
        assert_eq!(public2.as_bytes().len(), 32);
    }

    #[test]
    fn test_ecdh_key_agreement() {
        // Alice generates keypair
        let (alice_secret, _alice_public) = generate_ephemeral_keypair();

        // Bob generates keypair
        let (bob_secret, _bob_public) = generate_ephemeral_keypair();

        // Both derive the same session key
        let info = b"test-context";
        let alice_session_key = derive_session_key(alice_secret, &_bob_public, None, info);
        let bob_session_key = derive_session_key(bob_secret, &_alice_public, None, info);

        // Keys should match
        assert_eq!(alice_session_key, bob_session_key);
        assert_eq!(alice_session_key.len(), AES_KEY_SIZE);
    }

    #[test]
    fn test_ecdh_different_context() {
        let (alice_secret, _alice_public) = generate_ephemeral_keypair();
        let (_bob_secret, bob_public) = generate_ephemeral_keypair();

        // Different context strings produce different keys
        let key1 = derive_session_key(alice_secret, &bob_public, None, b"context1");

        let (alice_secret2, _) = generate_ephemeral_keypair();
        let key2 = derive_session_key(alice_secret2, &bob_public, None, b"context2");

        // Keys should be different (different secrets)
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_x25519_public_key_parsing() {
        let (_, public) = generate_ephemeral_keypair();
        let bytes = public.as_bytes();

        let parsed = parse_x25519_public(bytes).unwrap();
        assert_eq!(parsed.as_bytes(), bytes);
    }

    #[test]
    fn test_x25519_invalid_length() {
        let invalid = vec![0u8; 16]; // Wrong length
        assert!(parse_x25519_public(&invalid).is_err());
    }

    #[test]
    fn test_forward_secrecy_full_flow() {
        // Simulate full handshake with forward secrecy

        // 1. Both parties generate ephemeral keys
        let (alice_ephemeral_secret, alice_ephemeral_public) = generate_ephemeral_keypair();
        let (bob_ephemeral_secret, bob_ephemeral_public) = generate_ephemeral_keypair();

        // 2. Exchange public keys (simulated)
        let alice_public_bytes = alice_ephemeral_public.as_bytes();
        let bob_public_bytes = bob_ephemeral_public.as_bytes();

        // 3. Parse received public keys
        let bob_public_parsed = parse_x25519_public(bob_public_bytes).unwrap();
        let alice_public_parsed = parse_x25519_public(alice_public_bytes).unwrap();

        // 4. Derive session keys
        let info = b"p2p-messenger-v2";
        let alice_key = derive_session_key(alice_ephemeral_secret, &bob_public_parsed, None, info);
        let bob_key = derive_session_key(bob_ephemeral_secret, &alice_public_parsed, None, info);

        // 5. Keys should match
        assert_eq!(alice_key, bob_key);

        // 6. Use keys for encryption
        let alice_cipher = AesCipher::new(&alice_key).unwrap();
        let bob_cipher = AesCipher::new(&bob_key).unwrap();

        let plaintext = b"Forward secrecy test message";
        let encrypted = alice_cipher.encrypt(plaintext, None);
        let decrypted = bob_cipher.decrypt(&encrypted, None).unwrap();

        assert_eq!(plaintext, &decrypted[..]);
    }

    // ========== Key Rotation (Rekeying) Tests ==========

    #[test]
    fn test_rekey_derives_correct_key() {
        // Current key (32 bytes)
        let current_key = [42u8; AES_KEY_SIZE];

        // Nonce for rekeying (16 bytes, random)
        let nonce = [99u8; 16];

        // Derive next key
        let next_key = rekey_session_key(&current_key, &nonce);

        // Next key should be 32 bytes (same size)
        assert_eq!(next_key.len(), AES_KEY_SIZE);

        // Next key should be different from current key
        assert_ne!(next_key, current_key);
    }

    #[test]
    fn test_rekey_deterministic() {
        // Rekeying with the same inputs should produce the same output
        let current_key = [111u8; AES_KEY_SIZE];
        let nonce = [222u8; 16];

        let key1 = rekey_session_key(&current_key, &nonce);
        let key2 = rekey_session_key(&current_key, &nonce);

        // Same inputs should produce identical output (deterministic)
        assert_eq!(key1, key2);
    }

    #[test]
    fn test_rekey_different_nonces() {
        // Different nonces should produce different keys
        let current_key = [123u8; AES_KEY_SIZE];
        let nonce1 = [1u8; 16];
        let nonce2 = [2u8; 16];

        let key1 = rekey_session_key(&current_key, &nonce1);
        let key2 = rekey_session_key(&current_key, &nonce2);

        // Different nonces should produce different keys
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_generate_rekey_nonce() {
        let nonce1 = generate_rekey_nonce();
        let nonce2 = generate_rekey_nonce();

        // Nonces should be 16 bytes
        assert_eq!(nonce1.len(), 16);
        assert_eq!(nonce2.len(), 16);

        // Nonces should be different (with overwhelming probability)
        assert_ne!(nonce1, nonce2);
    }

    #[test]
    fn test_rekey_encryption_flow() {
        // Test that rekeying doesn't break encryption/decryption
        let current_key = [77u8; AES_KEY_SIZE];
        let cipher1 = AesCipher::new(&current_key).unwrap();

        // Encrypt message with first key
        let plaintext = b"Message before rekeying";
        let encrypted = cipher1.encrypt(plaintext, None);

        // Decrypt with same cipher should work
        let decrypted = cipher1.decrypt(&encrypted, None).unwrap();
        assert_eq!(plaintext, &decrypted[..]);

        // Now rekey
        let nonce = generate_rekey_nonce();
        let next_key = rekey_session_key(&current_key, &nonce);
        let cipher2 = AesCipher::new(&next_key).unwrap();

        // Encrypt with new key
        let plaintext2 = b"Message after rekeying";
        let encrypted2 = cipher2.encrypt(plaintext2, None);

        // Decrypt should work with new key
        let decrypted2 = cipher2.decrypt(&encrypted2, None).unwrap();
        assert_eq!(plaintext2, &decrypted2[..]);

        // Old cipher can still decrypt its own ciphertexts
        let old_decrypt = cipher1.decrypt(&encrypted, None).unwrap();
        assert_eq!(plaintext, &old_decrypt[..]);
    }

    // ========== Ed25519 Signature Tests ==========

    #[test]
    fn test_ed25519_keypair_generation() {
        let (skey1, vkey1) = generate_ed25519_keypair();
        let (skey2, vkey2) = generate_ed25519_keypair();

        // Keys should be different
        assert_ne!(skey1.to_bytes(), skey2.to_bytes());
        assert_ne!(vkey1.to_bytes(), vkey2.to_bytes());

        // Keys should be correct size
        assert_eq!(skey1.to_bytes().len(), 32);
        assert_eq!(vkey1.to_bytes().len(), 32);
    }

    #[test]
    fn test_ed25519_sign_verify() {
        let (signing_key, verifying_key) = generate_ed25519_keypair();
        let message = b"Test message for Ed25519";

        // Sign
        let signature = sign_ed25519(&signing_key, message);

        // Verify should succeed
        let result = verify_ed25519(&verifying_key, message, &signature);
        assert!(result.is_ok());
    }

    #[test]
    fn test_ed25519_verify_wrong_message() {
        let (signing_key, verifying_key) = generate_ed25519_keypair();
        let message1 = b"Test message 1";
        let message2 = b"Test message 2";

        // Sign message1
        let signature = sign_ed25519(&signing_key, message1);

        // Verify against message2 should fail
        let result = verify_ed25519(&verifying_key, message2, &signature);
        assert!(result.is_err());
    }

    #[test]
    fn test_ed25519_verify_wrong_key() {
        let (signing_key1, _) = generate_ed25519_keypair();
        let (_, verifying_key2) = generate_ed25519_keypair();
        let message = b"Test message";

        // Sign with key1
        let signature = sign_ed25519(&signing_key1, message);

        // Verify with key2 should fail
        let result = verify_ed25519(&verifying_key2, message, &signature);
        assert!(result.is_err());
    }

    #[test]
    fn test_ed25519_hex_roundtrip() {
        let (_, vkey_orig) = generate_ed25519_keypair();

        // Export to hex
        let hex_str = ed25519_public_to_hex(&vkey_orig);
        assert_eq!(hex_str.len(), 64); // 32 bytes = 64 hex chars

        // Import from hex
        let vkey_imported = ed25519_public_from_hex(&hex_str).unwrap();

        // Should match
        assert_eq!(vkey_orig.to_bytes(), vkey_imported.to_bytes());
    }

    #[test]
    fn test_signature_scheme_enum() {
        // Test RsaPss
        assert_eq!(SignatureScheme::RsaPss.to_u8(), 1);
        assert_eq!(SignatureScheme::RsaPss.name(), "RSA-PSS");
        assert_eq!(SignatureScheme::from_u8(1), Some(SignatureScheme::RsaPss));

        // Test Ed25519
        assert_eq!(SignatureScheme::Ed25519.to_u8(), 2);
        assert_eq!(SignatureScheme::Ed25519.name(), "Ed25519");
        assert_eq!(SignatureScheme::from_u8(2), Some(SignatureScheme::Ed25519));

        // Test unknown
        assert_eq!(SignatureScheme::from_u8(99), None);

        // Test Display
        assert_eq!(format!("{}", SignatureScheme::RsaPss), "RSA-PSS");
        assert_eq!(format!("{}", SignatureScheme::Ed25519), "Ed25519");
    }

    #[test]
    fn test_ed25519_identity_proof_binding() {
        // Simulate the identity proof signing scenario
        let (signing_key, verifying_key) = generate_ed25519_keypair();

        // Ephemeral key bytes (simulated X25519 key)
        let ephemeral_bytes = [42u8; 32];

        // Create the message to sign (like in the handshake)
        let mut data = Vec::new();
        data.extend_from_slice(b"IDENTITY_PROOF");
        data.extend_from_slice(&ephemeral_bytes);

        // Sign
        let signature = sign_ed25519(&signing_key, &data);

        // Verify with correct data
        assert!(verify_ed25519(&verifying_key, &data, &signature).is_ok());

        // Verify with different ephemeral key should fail
        let mut wrong_data = Vec::new();
        wrong_data.extend_from_slice(b"IDENTITY_PROOF");
        wrong_data.extend_from_slice(&[99u8; 32]);
        assert!(verify_ed25519(&verifying_key, &wrong_data, &signature).is_err());
    }

    #[test]
    fn test_signature_scheme_negotiation() {
        // Test that negotiation prefers Ed25519 over RSA-PSS
        let our_schemes = vec![
            SignatureScheme::RsaPss.to_u8(),
            SignatureScheme::Ed25519.to_u8(),
        ];
        let their_schemes = vec![
            SignatureScheme::Ed25519.to_u8(),
            SignatureScheme::RsaPss.to_u8(),
        ];

        let negotiated = negotiate_signature_scheme(&our_schemes, &their_schemes);
        assert_eq!(negotiated, Some(SignatureScheme::Ed25519));

        // Test fallback to RSA-PSS when Ed25519 not available
        let our_schemes_rsa_only = vec![SignatureScheme::RsaPss.to_u8()];
        let their_schemes_rsa_only = vec![SignatureScheme::RsaPss.to_u8()];

        let negotiated = negotiate_signature_scheme(&our_schemes_rsa_only, &their_schemes_rsa_only);
        assert_eq!(negotiated, Some(SignatureScheme::RsaPss));

        // Test no common scheme
        let our_schemes_ed = vec![SignatureScheme::Ed25519.to_u8()];
        let their_schemes_rsa = vec![SignatureScheme::RsaPss.to_u8()];

        let negotiated = negotiate_signature_scheme(&our_schemes_ed, &their_schemes_rsa);
        assert_eq!(negotiated, None);

        // Test reverse order doesn't matter (Ed25519 still preferred)
        let our_schemes_rev = vec![SignatureScheme::Ed25519.to_u8()];
        let their_schemes_rev = vec![
            SignatureScheme::RsaPss.to_u8(),
            SignatureScheme::Ed25519.to_u8(),
        ];

        let negotiated = negotiate_signature_scheme(&our_schemes_rev, &their_schemes_rev);
        assert_eq!(negotiated, Some(SignatureScheme::Ed25519));
    }

    #[test]
    fn test_supported_signature_schemes_message() {
        // Test SupportedSignatureSchemes message encoding/decoding
        let schemes = vec![
            SignatureScheme::RsaPss.to_u8(),
            SignatureScheme::Ed25519.to_u8(),
        ];
        let msg = crate::core::ProtocolMessage::SupportedSignatureSchemes {
            schemes: schemes.clone(),
        };

        let bytes = msg.to_plain_bytes();
        assert!(!bytes.is_empty());

        let decoded = crate::core::ProtocolMessage::from_plain_bytes(&bytes);
        assert!(decoded.is_some());

        match decoded {
            Some(crate::core::ProtocolMessage::SupportedSignatureSchemes {
                schemes: decoded_schemes,
            }) => {
                assert_eq!(decoded_schemes, schemes);
            }
            _ => panic!("Expected SupportedSignatureSchemes message"),
        }
    }
}
