/// Identity management module
///
/// This module handles user identity, including:
/// - Name and profile information
/// - RSA key pair generation and storage
/// - Fingerprint calculation
/// - Invite link generation
///
/// Identity is stored in a JSON file in the user's data directory.
/// Keys are currently stored in plaintext (TODO: encrypt with password).
use anyhow::Result;
use rsa::{
    pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePrivateKey, EncodePublicKey, LineEnding},
    RsaPrivateKey, RsaPublicKey,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use uuid::Uuid;

/// User identity with RSA key pair
#[derive(Serialize, Deserialize, Clone)]
pub struct Identity {
    pub id: Uuid,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,

    /// RSA private key in PEM format (PKCS#8)
    #[serde(skip_serializing_if = "Option::is_none")]
    private_key_pem: Option<String>,

    /// RSA public key in PEM format (PKCS#8)
    pub public_key_pem: String,

    /// SHA-256 fingerprint of public key (hex format)
    pub fingerprint: String,
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
            private_key_pem: Some(private_key_pem),
            public_key_pem,
            fingerprint,
        })
    }

    /// Calculate SHA-256 fingerprint of public key
    fn calculate_fingerprint(public_key_pem: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(public_key_pem.as_bytes());
        let result = hasher.finalize();
        hex::encode(result)
    }

    /// Get private key (if available)
    pub fn private_key(&self) -> Result<RsaPrivateKey> {
        let pem = self
            .private_key_pem
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Private key not available"))?;
        Ok(RsaPrivateKey::from_pkcs8_pem(pem)?)
    }

    /// Get public key
    pub fn public_key(&self) -> Result<RsaPublicKey> {
        Ok(RsaPublicKey::from_public_key_pem(&self.public_key_pem)?)
    }

    /// Generate invite link for this identity
    pub fn generate_invite_link(&self, address: Option<String>) -> Result<String> {
        use base64::Engine;
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
        assert!(identity.private_key_pem.is_some());
        assert!(identity
            .public_key_pem
            .starts_with("-----BEGIN PUBLIC KEY-----"));
    }

    #[test]
    fn test_identity_save_load() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("identity.json");

        let identity = Identity::new("Test User".to_string()).unwrap();
        identity.save(&path).unwrap();

        let loaded = Identity::load(&path).unwrap();
        assert_eq!(loaded.name, identity.name);
        assert_eq!(loaded.fingerprint, identity.fingerprint);
        assert_eq!(loaded.public_key_pem, identity.public_key_pem);
    }

    #[test]
    fn test_invite_link_generation() {
        let identity = Identity::new("Test User".to_string()).unwrap();
        let link = identity.generate_invite_link(None).unwrap();

        assert!(link.starts_with("chat-p2p://invite/"));
        assert!(link.len() > 50); // Should be a substantial base64 string
    }
}
