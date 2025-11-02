use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Key, Nonce,
};
use anyhow::{anyhow, Result};
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
    let mut hasher = Sha256::new();
    hasher.update(pem_bytes);
    let hash = hasher.finalize();
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
/// * `info` - Context string for HKDF (e.g., "p2p-messenger-v2")
/// 
/// # Returns
/// 32-byte AES-256 key derived from shared secret
pub fn derive_session_key(
    our_secret: EphemeralSecret,
    their_public: &X25519PublicKey,
    info: &[u8],
) -> [u8; AES_KEY_SIZE] {
    // Perform ECDH to get shared secret
    let shared_secret = our_secret.diffie_hellman(their_public);
    
    // Use HKDF-SHA256 to derive session key
    // Salt is None (uses zeros), which is acceptable for ephemeral keys
    let hkdf = Hkdf::<Sha256>::new(None, shared_secret.as_bytes());
    
    let mut session_key = [0u8; AES_KEY_SIZE];
    hkdf.expand(info, &mut session_key)
        .expect("HKDF expand should not fail with valid length");
    
    session_key
}

/// Parse X25519 public key from 32 bytes
pub fn parse_x25519_public(bytes: &[u8]) -> Result<X25519PublicKey> {
    if bytes.len() != 32 {
        return Err(anyhow!("X25519 public key must be 32 bytes, got {}", bytes.len()));
    }
    
    let mut key_bytes = [0u8; 32];
    key_bytes.copy_from_slice(bytes);
    Ok(X25519PublicKey::from(key_bytes))
}

/// AES-GCM cipher wrapper for encrypting/decrypting messages
#[derive(Clone)]
pub struct AesCipher {
    cipher: Aes256Gcm,
}

impl AesCipher {
    /// Create new cipher from 32-byte key
    pub fn new(key: &[u8]) -> Self {
        assert_eq!(key.len(), AES_KEY_SIZE, "AES key must be 32 bytes");
    // Use TryFrom to construct key from slice (avoids deprecated GenericArray::from_slice)
        let key = Key::<Aes256Gcm>::try_from(key).expect("Invalid AES key length");
        Self {
            // Aes256Gcm::new accepts a reference to the key array
            cipher: Aes256Gcm::new(&key),
        }
    }

    /// Encrypt plaintext, returns nonce(12) || ciphertext || tag(16)
    pub fn encrypt(&self, plaintext: &[u8]) -> Vec<u8> {
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        // Nonce::try_from accepts an array by value
        let nonce = Nonce::try_from(nonce_bytes).expect("Invalid nonce length");

        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext)
            .expect("AES-GCM encryption should not fail");

        // Format: nonce || ciphertext (includes tag)
        let mut output = Vec::with_capacity(12 + ciphertext.len());
        output.extend_from_slice(&nonce_bytes);
        output.extend_from_slice(&ciphertext);
        output
    }

    /// Decrypt payload: nonce(12) || ciphertext || tag(16)
    pub fn decrypt(&self, payload: &[u8]) -> Option<Vec<u8>> {
        if payload.len() < 12 + 16 {
            return None; // Too small
        }

        let (nonce_bytes, ciphertext) = payload.split_at(12);
        // Convert nonce slice to array then to Nonce
        let nonce_arr: [u8; 12] = match <[u8; 12]>::try_from(nonce_bytes) {
            Ok(a) => a,
            Err(_) => return None,
        };

        let nonce = Nonce::try_from(nonce_arr).expect("Invalid nonce length");

        self.cipher.decrypt(&nonce, ciphertext).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aes_roundtrip() {
        let key = [42u8; 32];
        let cipher = AesCipher::new(&key);

        let plaintext = b"Hello, secure world!";
        let encrypted = cipher.encrypt(plaintext);
        let decrypted = cipher.decrypt(&encrypted).unwrap();

        assert_eq!(plaintext, &decrypted[..]);
    }

    #[test]
    fn test_aes_nonce_randomness() {
        let key = [42u8; 32];
        let cipher = AesCipher::new(&key);

        let plaintext = b"Same message";
        let enc1 = cipher.encrypt(plaintext);
        let enc2 = cipher.encrypt(plaintext);

        // Ciphertexts should be different due to random nonces
        assert_ne!(enc1, enc2);

        // But both should decrypt correctly
        assert_eq!(cipher.decrypt(&enc1).unwrap(), plaintext);
        assert_eq!(cipher.decrypt(&enc2).unwrap(), plaintext);
    }

    #[test]
    fn test_aes_tamper_detection() {
        let key = [42u8; 32];
        let cipher = AesCipher::new(&key);

        let mut encrypted = cipher.encrypt(b"Test");
        if encrypted.len() > 20 {
            encrypted[20] ^= 1; // Tamper with ciphertext
        }

        assert!(cipher.decrypt(&encrypted).is_none());
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
    let alice_session_key = derive_session_key(alice_secret, &_bob_public, info);
    let bob_session_key = derive_session_key(bob_secret, &_alice_public, info);
        
        // Keys should match
        assert_eq!(alice_session_key, bob_session_key);
        assert_eq!(alice_session_key.len(), AES_KEY_SIZE);
    }

    #[test]
    fn test_ecdh_different_context() {
    let (alice_secret, _alice_public) = generate_ephemeral_keypair();
    let (_bob_secret, bob_public) = generate_ephemeral_keypair();
        
        // Different context strings produce different keys
        let key1 = derive_session_key(alice_secret, &bob_public, b"context1");
        
        let (alice_secret2, _) = generate_ephemeral_keypair();
        let key2 = derive_session_key(alice_secret2, &bob_public, b"context2");
        
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
        let alice_key = derive_session_key(alice_ephemeral_secret, &bob_public_parsed, info);
        let bob_key = derive_session_key(bob_ephemeral_secret, &alice_public_parsed, info);
        
        // 5. Keys should match
        assert_eq!(alice_key, bob_key);
        
        // 6. Use keys for encryption
        let alice_cipher = AesCipher::new(&alice_key);
        let bob_cipher = AesCipher::new(&bob_key);
        
        let plaintext = b"Forward secrecy test message";
        let encrypted = alice_cipher.encrypt(plaintext);
        let decrypted = bob_cipher.decrypt(&encrypted).unwrap();
        
        assert_eq!(plaintext, &decrypted[..]);
    }
}
