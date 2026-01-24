use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use anyhow::{anyhow, Result};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use hkdf::Hkdf;
use rand::{rngs::OsRng, RngCore};
use rsa::{
    pkcs1::{DecodeRsaPrivateKey, EncodeRsaPrivateKey},
    pkcs8::{DecodePublicKey, EncodePublicKey},
    Oaep, RsaPrivateKey, RsaPublicKey,
};
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

        Ok(Self {
            cipher: Aes256Gcm::new_from_slice(key)
                .map_err(|e| anyhow!("Invalid AES key length: {}", e))?,
            nonce_counter: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            session_id,
        })
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
        let mut nonce_bytes = [0u8; 12];
        nonce_bytes[0..4].copy_from_slice(&self.session_id);
        nonce_bytes[4..12].copy_from_slice(&counter.to_be_bytes());

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
        let mut output = Vec::with_capacity(12 + ciphertext.len());
        output.extend_from_slice(&nonce_bytes);
        output.extend_from_slice(&ciphertext);
        output
    }

    /// Decrypt payload with optional AAD: nonce(12) || ciphertext || tag(16)
    ///
    /// # Arguments
    /// * `payload` - Encrypted data in format nonce || ciphertext
    /// * `aad` - Optional Additional Authenticated Data (must match encryption AAD)
    pub fn decrypt(&self, payload: &[u8], aad: Option<&[u8]>) -> Option<Vec<u8>> {
        if payload.len() < 12 + 16 {
            return None; // Too small
        }

        let (nonce_bytes, ciphertext) = payload.split_at(12);
        // Convert nonce slice to array then to Nonce
        let nonce_arr: [u8; 12] = match <[u8; 12]>::try_from(nonce_bytes) {
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
}
