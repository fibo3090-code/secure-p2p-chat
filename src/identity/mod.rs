/// Identity management module
///
/// This module handles user identity, including:
/// - Name and profile information
/// - RSA key pair generation and storage
/// - Fingerprint calculation
/// - Invite link generation
///
/// Identity is stored in a JSON file in the user's data directory.
/// Keys are now encrypted with a password.
use anyhow::{anyhow, Result};
use argon2::Argon2;
use chacha20poly1305::{
    aead::{Aead, AeadCore, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use rsa::{
    pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePrivateKey, EncodePublicKey, LineEnding},
    RsaPrivateKey, RsaPublicKey,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use uuid::Uuid;
use zeroize::Zeroizing;
use base64::Engine;
use rand::RngCore;

// Constants for encryption
const KEY_SIZE: usize = 32; // 256-bit key

/// User identity with RSA key pair
#[derive(Serialize, Deserialize, Clone)]
pub struct Identity {
    pub id: Uuid,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,

    /// Encrypted RSA private key (ChaCha20-Poly1305)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_private_key: Option<Vec<u8>>,

    /// Salt for Argon2 key derivation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub salt: Option<Vec<u8>>,

    /// Nonce for ChaCha20-Poly1305
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonce: Option<Vec<u8>>,

    /// RSA public key in PEM format (PKCS#8)
    pub public_key_pem: String,

    /// SHA-256 fingerprint of public key (hex format)
    pub fingerprint: String,

    /// Plaintext private key, used temporarily after decryption.
    /// This field is NOT serialized.
    #[serde(skip)]
    private_key_pem_plaintext: Option<String>,
}

impl Identity {
    /// Create new identity with generated RSA key pair
    pub fn new(name: String) -> Result<Self> {
        use rand::rngs::OsRng;

        tracing::info!("Generating new identity for: {}", name);

        // Generate 2048-bit RSA key pair
        let mut rng = OsRng;
        let private_key = RsaPrivateKey::new(&mut rng, 2048)?;
        let public_key = RsaPublicKey::from(&private_key);

        // Encode to PEM
        let private_key_pem = private_key.to_pkcs8_pem(LineEnding::LF)?.to_string();
        let public_key_pem = public_key.to_public_key_pem(LineEnding::LF)?;

        // Calculate fingerprint
        let fingerprint = Self::calculate_fingerprint(&public_key_pem);

        Ok(Self {
            id: Uuid::new_v4(),
            name,
            created_at: chrono::Utc::now(),
            encrypted_private_key: None,
            salt: None,
            nonce: None,
            public_key_pem,
            fingerprint,
            private_key_pem_plaintext: Some(private_key_pem),
        })
    }

    /// Calculate SHA-256 fingerprint of public key
    fn calculate_fingerprint(public_key_pem: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(public_key_pem.as_bytes());
        let result = hasher.finalize();
        hex::encode(result)
    }

    /// Encrypt the private key with a password.
    pub fn encrypt(&mut self, password: &str) -> Result<()> {
        let plaintext_pem = self
            .private_key_pem_plaintext
            .as_ref()
            .ok_or_else(|| anyhow!("Plaintext private key is not available for encryption"))?;

        // Derive key with Argon2 using random salt bytes
        let mut salt = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut salt);
        let argon2 = Argon2::default();
        let mut key_bytes = Zeroizing::new([0u8; KEY_SIZE]);
        argon2
            .hash_password_into(password.as_bytes(), &salt, &mut key_bytes[..])
            .map_err(|e| anyhow!("Failed to derive key with Argon2: {}", e))?;

        let cipher = ChaCha20Poly1305::new((&key_bytes[..]).into());
        let nonce = ChaCha20Poly1305::generate_nonce(&mut rand::rngs::OsRng);
        let ciphertext = cipher
            .encrypt(&nonce, plaintext_pem.as_bytes())
            .map_err(|e| anyhow!("Encryption failed: {}", e))?;

        self.encrypted_private_key = Some(ciphertext);
        self.salt = Some(salt.to_vec());
        self.nonce = Some(nonce.to_vec());

        // Clear the plaintext key from memory
        self.private_key_pem_plaintext = None;

        Ok(())
    }

    /// Decrypt the private key with a password.
    pub fn decrypt(&mut self, password: &str) -> Result<()> {
        let salt_bytes = self
            .salt
            .as_ref()
            .ok_or_else(|| anyhow!("Salt not found"))?;
        let nonce_bytes = self
            .nonce
            .as_ref()
            .ok_or_else(|| anyhow!("Nonce not found"))?;
        let ciphertext = self
            .encrypted_private_key
            .as_ref()
            .ok_or_else(|| anyhow!("Encrypted private key not found"))?;

        let argon2 = Argon2::default();
        let mut key_bytes = Zeroizing::new([0u8; KEY_SIZE]);
        argon2
            .hash_password_into(password.as_bytes(), salt_bytes, &mut key_bytes[..])
            .map_err(|e| anyhow!("Failed to derive key with Argon2: {}", e))?;

        let cipher = ChaCha20Poly1305::new((&key_bytes[..]).into());
        let nonce = Nonce::from_slice(nonce_bytes);
        let plaintext = cipher
            .decrypt(nonce, ciphertext.as_ref())
            .map_err(|e| anyhow!("Decryption failed (likely wrong password): {}", e))?;

        self.private_key_pem_plaintext = Some(String::from_utf8(plaintext)?);

        Ok(())
    }

    /// Get private key (if available)
    pub fn private_key(&self) -> Result<RsaPrivateKey> {
        let pem = self
            .private_key_pem_plaintext
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Private key not available. Was the identity decrypted?"))?;
        Ok(RsaPrivateKey::from_pkcs8_pem(pem)?)
    }

    /// Get public key
    pub fn public_key(&self) -> Result<RsaPublicKey> {
        Ok(RsaPublicKey::from_public_key_pem(&self.public_key_pem)?)
    }

    /// Generate invite link for this identity
    pub fn generate_invite_link(&self, address: Option<String>) -> Result<String> {
        use serde_json::json;

        let payload = json!({
            "name": self.name,
            "address": address,
            "fingerprint": self.fingerprint,
            "public_key": self.public_key_pem,
        });

        let json = serde_json::to_string(&payload)?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(json);
        Ok(format!("chat-p2p://invite/{}", encoded))
    }

    /// Load identity from file
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let mut identity: Identity = serde_json::from_str(&content)?;
        tracing::info!("Loaded identity: {} ({})", identity.name, identity.id);

        // For backward compatibility, if the old plaintext field exists, use it.
        if let Ok(id_with_old_field) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(old_pem) = id_with_old_field
                .get("private_key_pem")
                .and_then(|v| v.as_str())
            {
                identity.private_key_pem_plaintext = Some(old_pem.to_string());
                tracing::warn!("Loaded an unencrypted identity file. Please set a password to encrypt it.");
            }
        }

        Ok(identity)
    }

    /// Save identity to file
    pub fn save(&self, path: &Path) -> Result<()> {
        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = serde_json::to_string_pretty(&self)?;
        std::fs::write(path, content)?;
        tracing::info!("Saved identity: {} to {}", self.name, path.display());
        Ok(())
    }

    /// Get or create identity from user data directory
    pub fn get_or_create(data_dir: &Path, default_name: &str) -> Result<Self> {
        let identity_path = data_dir.join("identity.json");

        if identity_path.exists() {
            // Load existing identity
            match Self::load(&identity_path) {
                Ok(identity) => {
                    tracing::info!("Using existing identity: {}", identity.name);
                    Ok(identity)
                }
                Err(e) => {
                    tracing::warn!("Failed to load identity, creating new one: {}", e);
                    let identity = Self::new(default_name.to_string())?;
                    identity.save(&identity_path)?;
                    Ok(identity)
                }
            }
        } else {
            // Create new identity
            tracing::info!("No existing identity found, creating new one");
            let identity = Self::new(default_name.to_string())?;
            identity.save(&identity_path)?;
            Ok(identity)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_identity_creation() {
        let identity = Identity::new("Test User".to_string()).unwrap();

        assert_eq!(identity.name, "Test User");
        assert_eq!(identity.fingerprint.len(), 64); // SHA-256 in hex
        assert!(identity.private_key_pem_plaintext.is_some());
        assert!(identity
            .public_key_pem
            .starts_with("-----BEGIN PUBLIC KEY-----"));
    }

    #[test]
    fn test_identity_save_load_unencrypted() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("identity.json");

        let identity = Identity::new("Test User".to_string()).unwrap();
        identity.save(&path).unwrap();

        let loaded = Identity::load(&path).unwrap();
        assert_eq!(loaded.name, identity.name);
        assert_eq!(loaded.fingerprint, identity.fingerprint);
        assert_eq!(loaded.public_key_pem, identity.public_key_pem);
        // The plaintext key should be loaded for backward compatibility
        assert!(loaded.private_key_pem_plaintext.is_some());
    }

    #[test]
    fn test_encryption_decryption_roundtrip() {
        let mut identity = Identity::new("Test User".to_string()).unwrap();
        let original_pem = identity.private_key_pem_plaintext.clone().unwrap();

        // Encrypt
        identity.encrypt("password123").unwrap();
        assert!(identity.private_key_pem_plaintext.is_none());
        assert!(identity.encrypted_private_key.is_some());
        assert!(identity.salt.is_some());
        assert!(identity.nonce.is_some());

        // Decrypt
        identity.decrypt("password123").unwrap();
        assert!(identity.private_key_pem_plaintext.is_some());
        assert_eq!(
            identity.private_key_pem_plaintext.unwrap(),
            original_pem
        );
    }

    #[test]
    fn test_decryption_with_wrong_password_fails() {
        let mut identity = Identity::new("Test User".to_string()).unwrap();
        identity.encrypt("password123").unwrap();
        let result = identity.decrypt("wrong-password");
        assert!(result.is_err());
    }

    #[test]
    fn test_save_load_encrypted() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("identity.json");

        let mut identity = Identity::new("Test User".to_string()).unwrap();
        let original_pem = identity.private_key().unwrap();

        // Encrypt and save
        identity.encrypt("password123").unwrap();
        identity.save(&path).unwrap();

        // Load and decrypt
        let mut loaded = Identity::load(&path).unwrap();
        assert!(loaded.private_key_pem_plaintext.is_none()); // Should not be available yet
        loaded.decrypt("password123").unwrap();

        assert_eq!(loaded.private_key().unwrap(), original_pem);
    }

    #[test]
    fn test_invite_link_generation() {
        let identity = Identity::new("Test User".to_string()).unwrap();
        let link = identity.generate_invite_link(None).unwrap();

        assert!(link.starts_with("chat-p2p://invite/"));
        assert!(link.len() > 50); // Should be a substantial base64 string
    }
}
