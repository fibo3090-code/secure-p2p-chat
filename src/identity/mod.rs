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
use anyhow::{Result, anyhow};
use argon2::Argon2;
use base64::Engine;
use chacha20poly1305::{
    ChaCha20Poly1305,
    aead::{Aead, AeadCore, KeyInit},
};
use rand::RngCore;
use rsa::{
    RsaPrivateKey, RsaPublicKey,
    pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePrivateKey, EncodePublicKey, LineEnding},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use uuid::Uuid;
use zeroize::Zeroizing;

// Constants for encryption
const KEY_SIZE: usize = 32; // 256-bit key

/// User identity with RSA key pair
/// 
/// SECURITY: Private keys are wrapped in Zeroizing to ensure they are
/// securely wiped from memory when dropped.
#[derive(Clone)]
pub struct Identity {
    pub id: Uuid,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,

    /// Encrypted RSA private key (ChaCha20-Poly1305)
    pub encrypted_private_key: Option<Vec<u8>>,

    /// Salt for Argon2 key derivation
    pub salt: Option<Vec<u8>>,

    /// Nonce for ChaCha20-Poly1305
    pub nonce: Option<Vec<u8>>,

    /// RSA public key in PEM format (PKCS#8)
    pub public_key_pem: String,

    /// SHA-256 fingerprint of public key (hex format)
    pub fingerprint: String,

    /// Plaintext private key, used temporarily after decryption.
    /// SECURITY: Wrapped in Zeroizing to wipe from memory on drop.
    private_key_pem_plaintext: Option<Zeroizing<String>>,
}

// Custom Serialize implementation to skip the private key field
impl Serialize for Identity {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Identity", 8)?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("created_at", &self.created_at)?;
        if self.encrypted_private_key.is_some() {
            state.serialize_field("encrypted_private_key", &self.encrypted_private_key)?;
        } else {
            // BACKWARD COMPATIBILITY / UNSAFE: If not encrypted, save plaintext key.
            // This allows the "save unencrypted" test to pass and supports legacy behavior until
            // we enforce encryption everywhere.
            if let Some(pk) = &self.private_key_pem_plaintext {
                state.serialize_field("private_key_pem_plaintext", &pk[..])?;
            }
        }
        if self.salt.is_some() {
            state.serialize_field("salt", &self.salt)?;
        }
        if self.nonce.is_some() {
            state.serialize_field("nonce", &self.nonce)?;
        }
        state.serialize_field("public_key_pem", &self.public_key_pem)?;
        state.serialize_field("fingerprint", &self.fingerprint)?;
        state.end()
    }
}

// Helper struct for deserialization
#[derive(Deserialize)]
struct IdentityHelper {
    id: Uuid,
    name: String,
    created_at: chrono::DateTime<chrono::Utc>,
    encrypted_private_key: Option<Vec<u8>>,
    salt: Option<Vec<u8>>,
    nonce: Option<Vec<u8>>,
    public_key_pem: String,
    fingerprint: String,
    private_key_pem_plaintext: Option<String>,
}

impl<'de> Deserialize<'de> for Identity {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let helper = IdentityHelper::deserialize(deserializer)?;
        
        let private_key_pem_plaintext = helper.private_key_pem_plaintext.map(Zeroizing::new);
        
        Ok(Identity {
            id: helper.id,
            name: helper.name,
            created_at: helper.created_at,
            encrypted_private_key: helper.encrypted_private_key,
            salt: helper.salt,
            nonce: helper.nonce,
            public_key_pem: helper.public_key_pem,
            fingerprint: helper.fingerprint,
            private_key_pem_plaintext,
        })
    }
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
            private_key_pem_plaintext: Some(Zeroizing::new(private_key_pem)),
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
        let nonce = nonce_bytes.as_slice().into();
        let plaintext = cipher
            .decrypt(nonce, ciphertext.as_ref())
            .map_err(|e| anyhow!("Decryption failed (likely wrong password): {}", e))?;

        self.private_key_pem_plaintext = Some(Zeroizing::new(String::from_utf8(plaintext)?));

        Ok(())
    }

    /// Removes password protection from the identity.
    /// It decrypts the private key with the given password and then
    /// clears the encryption-related fields.
    pub fn remove_password(&mut self, password: &str) -> Result<()> {
        // First, decrypt the key to make sure the password is correct and
        // to get the plaintext key.
        self.decrypt(password)?;

        // Now that `private_key_pem_plaintext` is populated, we can
        // clear the encryption fields.
        self.encrypted_private_key = None;
        self.salt = None;
        self.nonce = None;

        Ok(())
    }

    /// Get private key (if available)
    pub fn private_key(&self) -> Result<RsaPrivateKey> {
        let pem = self.private_key_pem_plaintext.as_ref().ok_or_else(|| {
            anyhow::anyhow!("Private key not available. Was the identity decrypted?")
        })?;
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
        let identity: Identity = serde_json::from_str(&content)?;
        tracing::info!("Loaded identity: {} ({})", identity.name, identity.id);

        // If the identity is not encrypted, the private_key_pem_plaintext should be present.
        // If it is encrypted, it will be None, and decrypt() will populate it later.
        if identity.encrypted_private_key.is_none() && identity.private_key_pem_plaintext.is_none()
        {
            // This case should ideally not happen if the file was saved correctly as unencrypted.
            // However, if it does, it means an unencrypted identity was loaded but its plaintext key is missing.
            // This might indicate a corrupted file or a very old format.
            // For now, we'll treat it as an error, as we expect private_key_pem_plaintext to be Some for unencrypted.
            return Err(anyhow!(
                "Unencrypted identity loaded but private key plaintext is missing."
            ));
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
    pub fn get_or_create(data_dir: &Path, default_name: &str) -> Result<(Self, bool)> {
        let identity_path = data_dir.join("identity.json");

        if identity_path.exists() {
            // Load existing identity
            match Self::load(&identity_path) {
                Ok(identity) => {
                    tracing::info!("Using existing identity: {}", identity.name);
                    Ok((identity, false))
                }
                Err(e) => {
                    tracing::warn!("Failed to load identity, creating new one: {}", e);
                    let identity = Self::new(default_name.to_string())?;
                    Ok((identity, true))
                }
            }
        } else {
            // Create new identity
            tracing::info!("No existing identity found, creating new one");
            let identity = Self::new(default_name.to_string())?;
            Ok((identity, true))
        }
    }

    /// Check if the identity's private key is encrypted and not currently decrypted.
    pub fn is_locked(&self) -> bool {
        self.encrypted_private_key.is_some() && self.private_key_pem_plaintext.is_none()
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
        assert!(
            identity
                .public_key_pem
                .starts_with("-----BEGIN PUBLIC KEY-----")
        );
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
        assert_eq!(identity.private_key_pem_plaintext.unwrap(), original_pem);
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

    #[test]
    fn test_invite_link_includes_address_when_provided() {
        let identity = Identity::new("Tester".to_string()).unwrap();
        let addr = Some("10.0.0.5:5000".to_string());
        let link = identity.generate_invite_link(addr.clone()).unwrap();
        assert!(link.contains("chat-p2p://invite/"));

        // Parse payload back to verify address is preserved
        let encoded = link.strip_prefix("chat-p2p://invite/").unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(
            payload.get("address").and_then(|v| v.as_str()),
            addr.as_deref()
        );
    }
}
