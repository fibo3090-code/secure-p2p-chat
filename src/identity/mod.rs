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
use argon2::{Algorithm, Argon2, Params, Version};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use chacha20poly1305::{
    aead::{Aead, AeadCore, KeyInit},
    ChaCha20Poly1305,
};
use rand::RngCore;
use rsa::{
    pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePrivateKey, EncodePublicKey, LineEnding},
    RsaPrivateKey, RsaPublicKey,
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

    /// Argon2 parameters used to derive the encryption key for the private key
    pub argon_params: Option<ArgonParams>,

    /// RSA public key in PEM format (PKCS#8)
    pub public_key_pem: String,

    /// SHA-256 fingerprint of public key (hex format)
    pub fingerprint: String,

    /// Plaintext private key, used temporarily after decryption.
    /// SECURITY: Wrapped in Zeroizing to wipe from memory on drop.
    private_key_pem_plaintext: Option<Zeroizing<String>>,
}

/// Serializable representation of Argon2 parameters
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ArgonParams {
    pub m_cost_kib: u32,
    pub t_cost: u32,
    pub p_cost: u32,
    pub output_len: u32,
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

        // SECURITY: CRITICAL! Never serialize the plaintext private key.
        // We only serialize the encrypted key. If it's not encrypted, we serializing
        // a broken identity (missing key) is better than leaking plaintext.
        // The save() method guards against saving unencrypted identities.
        if self.encrypted_private_key.is_some() {
            state.serialize_field("encrypted_private_key", &self.encrypted_private_key)?;
        }

        if self.salt.is_some() {
            state.serialize_field("salt", &self.salt)?;
        }
        if self.nonce.is_some() {
            state.serialize_field("nonce", &self.nonce)?;
        }
        if self.argon_params.is_some() {
            state.serialize_field("argon_params", &self.argon_params)?;
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
    argon_params: Option<ArgonParams>,
    public_key_pem: String,
    fingerprint: String,
    // We keep this solely for backward compatibility when loading old files,
    // but we won't write to it anymore.
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
            argon_params: helper.argon_params,
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

        // By default, do NOT retain plaintext private key in memory.
        // Callers that need immediate plaintext access should use
        // `Identity::new_with_plaintext(...)` explicitly.
        tracing::info!("Generating new identity for (no-plaintext): {}", name);

        // Generate 2048-bit RSA key pair
        let mut rng = OsRng;
        let private_key = RsaPrivateKey::new(&mut rng, 2048)?;
        let public_key = RsaPublicKey::from(&private_key);

        let public_key_pem = public_key.to_public_key_pem(LineEnding::LF)?;

        // Calculate fingerprint
        let fingerprint = Self::calculate_fingerprint(&public_key_pem);

        Ok(Self {
            id: Uuid::new_v4(),
            name,
            created_at: chrono::Utc::now(),
            encrypted_private_key: None,
            salt: None,
            argon_params: None,
            nonce: None,
            public_key_pem,
            fingerprint,
            // Do not keep plaintext by default
            private_key_pem_plaintext: None,
        })
    }

    /// Legacy constructor: create identity and retain plaintext private key in memory.
    /// Use only when caller needs immediate access to the private key (tests, setup flows).
    pub fn new_with_plaintext(name: String) -> Result<Self> {
        use rand::rngs::OsRng;

        tracing::info!("Generating new identity (with plaintext) for: {}", name);

        let mut rng = OsRng;
        let private_key = RsaPrivateKey::new(&mut rng, 2048)?;
        let public_key = RsaPublicKey::from(&private_key);

        let private_key_pem = private_key.to_pkcs8_pem(LineEnding::LF)?.to_string();
        let public_key_pem = public_key.to_public_key_pem(LineEnding::LF)?;

        let fingerprint = Self::calculate_fingerprint(&public_key_pem);

        Ok(Self {
            id: Uuid::new_v4(),
            name,
            created_at: chrono::Utc::now(),
            encrypted_private_key: None,
            salt: None,
            argon_params: None,
            nonce: None,
            public_key_pem,
            fingerprint,
            private_key_pem_plaintext: Some(Zeroizing::new(private_key_pem)),
        })
    }

    /// Calculate SHA-256 fingerprint of public key
    fn calculate_fingerprint(public_key_pem: &str) -> String {
        // Prefer hashing canonical DER (SubjectPublicKeyInfo) extracted from PEM
        if let Ok(s) = std::str::from_utf8(public_key_pem.as_bytes()) {
            if let (Some(begin), Some(end)) = (
                s.find("-----BEGIN PUBLIC KEY-----"),
                s.find("-----END PUBLIC KEY-----"),
            ) {
                let body = &s[begin + "-----BEGIN PUBLIC KEY-----".len()..end];
                let b64: String = body.chars().filter(|c| !c.is_whitespace()).collect();
                if let Ok(der) = BASE64_STANDARD.decode(&b64) {
                    let hash = Sha256::digest(&der);
                    return hex::encode(hash);
                }
            }
        }

        // Fallback: hash raw PEM bytes
        let result = Sha256::digest(public_key_pem.as_bytes());
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

        // Security Hardening: Use explicit Argon2id parameters and save them
        // m_cost: 64 MiB (65536 KiB), t_cost: 3, p_cost: 4
        let m_cost_kib: u32 = 65536;
        let t_cost: u32 = 3;
        let p_cost: u32 = 4;
        let params = Params::new(m_cost_kib, t_cost, p_cost, Some(KEY_SIZE))
            .map_err(|e| anyhow!("Invalid Argon2 parameters: {}", e))?;
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

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
        self.argon_params = Some(ArgonParams {
            m_cost_kib,
            t_cost,
            p_cost,
            output_len: KEY_SIZE as u32,
        });

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

        // Use same strict parameters as encryption
        // Ideally we should store params in the file, but for now we enforce defaults.
        // If we change params, we break old keys.
        // Note: Argon2::default() uses different params, so this changes format.
        // We assume new keys use new params. Old keys?
        // Wait, if we change params now, we might break existing identities on disk?
        // Yes. If user updates app, they can't decrypt old identity if params mismatch.
        // We need backward compatibility or migration?
        // Audit says: "Use explicit parameters".
        // Solution: Try strict params first, fallback to default?
        // Or store params in Identity struct.
        // Given this is a small project, specific breaking changes are noted in v1.6.0.
        // But breaking users' ability to login is bad.
        // I'll assume users will re-create identity or I should implement fallback.
        // I will attempt strictly first.
        // Try stored params first (if present)
        let mut key_bytes = Zeroizing::new([0u8; KEY_SIZE]);

        if let Some(ref ap) = self.argon_params {
            if let Ok(params) = Params::new(
                ap.m_cost_kib,
                ap.t_cost,
                ap.p_cost,
                Some(ap.output_len as usize),
            ) {
                let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
                if argon
                    .hash_password_into(password.as_bytes(), salt_bytes, &mut key_bytes[..])
                    .is_ok()
                {
                    let cipher = ChaCha20Poly1305::new((&key_bytes[..]).into());
                    let nonce = nonce_bytes.as_slice().into();
                    if let Ok(plaintext) = cipher.decrypt(nonce, ciphertext.as_ref()) {
                        self.private_key_pem_plaintext =
                            Some(Zeroizing::new(String::from_utf8(plaintext)?));
                        return Ok(());
                    }
                }
            }
        }

        // Fallback to strict default parameters (new scheme)
        if let Ok(params) = Params::new(65536, 3, 4, Some(KEY_SIZE)) {
            let argon2_strict = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
            if argon2_strict
                .hash_password_into(password.as_bytes(), salt_bytes, &mut key_bytes[..])
                .is_ok()
            {
                let cipher = ChaCha20Poly1305::new((&key_bytes[..]).into());
                let nonce = nonce_bytes.as_slice().into();
                if let Ok(plaintext) = cipher.decrypt(nonce, ciphertext.as_ref()) {
                    self.private_key_pem_plaintext =
                        Some(Zeroizing::new(String::from_utf8(plaintext)?));
                    return Ok(());
                }
            }
        }

        // Legacy fallback: Argon2::default()
        let argon2_default = Argon2::default();
        argon2_default
            .hash_password_into(password.as_bytes(), salt_bytes, &mut key_bytes[..])
            .map_err(|e| anyhow!("Argon2 hash failed: {}", e))?;

        let cipher = ChaCha20Poly1305::new((&key_bytes[..]).into());
        let nonce = nonce_bytes.as_slice().into();
        let plaintext = cipher
            .decrypt(nonce, ciphertext.as_ref())
            .map_err(|e| anyhow!("Decryption failed (likely wrong password): {}", e))?;

        self.private_key_pem_plaintext = Some(Zeroizing::new(String::from_utf8(plaintext)?));

        Ok(())
    }

    /// Removing password protection is intentionally unsupported.
    ///
    /// Identities must remain encrypted when persisted to disk.
    pub fn remove_password(&mut self, _password: &str) -> Result<()> {
        Err(anyhow!(
            "Removing password protection is not supported; identities must remain encrypted on disk"
        ))
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

    /// Migrate a legacy identity that currently holds a plaintext private key in memory
    /// by encrypting it with `password` and clearing the plaintext. Does nothing if the
    /// identity already has an encrypted private key.
    pub fn migrate_legacy_plaintext(&mut self, password: &str) -> Result<()> {
        if self.encrypted_private_key.is_some() {
            // Already encrypted; nothing to do
            return Ok(());
        }

        if self.private_key_pem_plaintext.is_none() {
            return Err(anyhow!("No plaintext private key available to migrate"));
        }

        // Use existing encrypt() implementation which clears plaintext on success
        self.encrypt(password)
    }

    /// Generate a signed invite link for this identity (v2, with RSA-PSS signature)
    ///
    /// This creates a cryptographically signed invite link that prevents tampering.
    /// The signature is computed over the invite payload and verified on import.
    pub fn generate_signed_invite_link(&self, address: Option<String>) -> Result<String> {
        self.generate_signed_invite_link_with_route(address, None, None)
    }

    pub fn generate_signed_invite_link_with_route(
        &self,
        address: Option<String>,
        relay_server: Option<String>,
        relay_token: Option<String>,
    ) -> Result<String> {
        use serde::{Deserialize, Serialize};
        use std::time::{SystemTime, UNIX_EPOCH};

        let mut invite_nonce = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut invite_nonce);
        let nonce = hex::encode(invite_nonce);

        // Create timestamp (current UTC Unix timestamp)
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        #[derive(Serialize, Deserialize, Clone)]
        struct SignedInvitePayload {
            version: u32,
            timestamp: u64,
            nonce: String, // Random per-invite nonce encoded as hex
            name: String,
            address: Option<String>,
            relay_server: Option<String>,
            relay_token: Option<String>,
            fingerprint: String,
            public_key: String,
        }

        let payload = SignedInvitePayload {
            version: if relay_server.is_some() || relay_token.is_some() {
                3
            } else {
                2
            },
            timestamp,
            nonce,
            name: self.name.clone(),
            address,
            relay_server,
            relay_token,
            fingerprint: self.fingerprint.clone(),
            public_key: self.public_key_pem.clone(),
        };

        // Serialize payload using the crate-local serde_json representation.
        // Verification uses the same byte representation; this is deterministic for this app,
        // but it is not a general-purpose RFC 8785 canonicalization scheme.
        let payload_json = serde_json::to_string(&payload)?;
        let payload_bytes = payload_json.as_bytes();

        // Sign the payload using the identity's RSA private key
        let privkey = self.private_key()?;
        let signature = crate::core::crypto::rsa_sign_pss(&privkey, payload_bytes)?;

        #[derive(Serialize, Deserialize)]
        struct SignedInvite {
            payload: SignedInvitePayload,
            signature: Vec<u8>, // RSA-PSS signature over payload JSON
        }

        let signed_invite = SignedInvite { payload, signature };
        let invite_json = serde_json::to_string(&signed_invite)?;

        // Use URL-safe base64 encoding (RFC 4648)
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(invite_json);
        Ok(format!("chat-p2p://invite/v2/{}", encoded))
    }

    /// Generate invite link for this identity (v1, unsigned - for backward compatibility)
    ///
    /// DEPRECATED: Use generate_signed_invite_link() instead for security.
    /// This is kept for backward compatibility with existing invites.
    pub fn generate_invite_link(&self, address: Option<String>) -> Result<String> {
        use serde_json::json;

        let payload = json!({
            "name": self.name,
            "address": address,
            "fingerprint": self.fingerprint,
            "public_key": self.public_key_pem,
        });

        let json = serde_json::to_string(&payload)?;
        // Use standard base64 for v1 (legacy)
        let encoded = base64::engine::general_purpose::STANDARD.encode(json);
        Ok(format!("chat-p2p://invite/{}", encoded))
    }

    /// Load identity from file
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let identity: Identity = serde_json::from_str(&content)?;
        tracing::info!("Loaded identity: {} ({})", identity.name, identity.id);

        // SECURITY: Warn if loading legacy file with plaintext private key on disk.
        if identity.private_key_pem_plaintext.is_some() {
            tracing::warn!(
                "Loaded identity has plaintext private key in file (legacy format). \
                 Consider encrypting with a password and saving to improve security."
            );
        }

        if identity.encrypted_private_key.is_none() && identity.private_key_pem_plaintext.is_none()
        {
            return Err(anyhow!(
                "Unencrypted identity loaded but private key plaintext is missing."
            ));
        }

        Ok(identity)
    }

    /// Save identity to file
    ///
    /// SECURITY: Enforces encryption and strictly secure file permissions (0600 on Unix).
    pub fn save(&self, path: &Path) -> Result<()> {
        // Guard against saving unencrypted identities
        if self.encrypted_private_key.is_none() {
            return Err(anyhow!(
                "SECURITY ERROR: Cannot save identity without encryption!"
            ));
        }

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = serde_json::to_string_pretty(&self)?;

        // Secure file creation
        #[cfg(unix)]
        {
            use std::fs::OpenOptions;
            use std::os::unix::fs::OpenOptionsExt;

            let mut options = OpenOptions::new();
            options.write(true).create(true).truncate(true).mode(0o600);

            let mut file = options.open(path)?;
            std::io::Write::write_all(&mut file, content.as_bytes())?;
        }

        // On Windows/Other, standard write for now (ACLs are complex in std)
        #[cfg(not(unix))]
        {
            std::fs::write(path, content)?;
        }

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
                    let identity = Self::new_with_plaintext(default_name.to_string())?;
                    Ok((identity, true))
                }
            }
        } else {
            // Create new identity
            tracing::info!("No existing identity found, creating new one");
            let identity = Self::new_with_plaintext(default_name.to_string())?;
            Ok((identity, true))
        }
    }

    /// Check if the identity's private key is encrypted and not currently decrypted.
    pub fn is_locked(&self) -> bool {
        self.encrypted_private_key.is_some() && self.private_key_pem_plaintext.is_none()
    }

    /// Derive a stable 32-byte history encryption key from the private key.
    /// Requires the identity to be unlocked (private key available).
    pub fn history_key(&self) -> Result<[u8; 32]> {
        let privkey = self.private_key()?;
        let der = privkey.to_pkcs8_der()?;
        let digest = Sha256::digest(der.as_bytes());
        let mut key = [0u8; 32];
        key.copy_from_slice(&digest);
        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_password() -> String {
        format!("pw-{}", Uuid::new_v4())
    }

    #[test]
    fn test_identity_creation() {
        let identity = Identity::new_with_plaintext("Test User".to_string()).unwrap();

        assert_eq!(identity.name, "Test User");
        assert_eq!(identity.fingerprint.len(), 64); // SHA-256 in hex
        assert!(identity.private_key_pem_plaintext.is_some());
        assert!(identity
            .public_key_pem
            .starts_with("-----BEGIN PUBLIC KEY-----"));
    }

    #[test]
    fn test_encryption_decryption_roundtrip() {
        let mut identity = Identity::new_with_plaintext("Test User".to_string()).unwrap();
        let original_pem = identity.private_key_pem_plaintext.clone().unwrap();
        let password = test_password();

        // Encrypt
        identity.encrypt(&password).unwrap();
        assert!(identity.private_key_pem_plaintext.is_none());
        assert!(identity.encrypted_private_key.is_some());
        assert!(identity.salt.is_some());
        assert!(identity.nonce.is_some());

        // Decrypt
        identity.decrypt(&password).unwrap();
        assert!(identity.private_key_pem_plaintext.is_some());
        assert_eq!(identity.private_key_pem_plaintext.unwrap(), original_pem);
    }

    #[test]
    fn test_decryption_with_wrong_password_fails() {
        let mut identity = Identity::new_with_plaintext("Test User".to_string()).unwrap();
        let password = test_password();
        identity.encrypt(&password).unwrap();
        let wrong_password = test_password();
        let result = identity.decrypt(&wrong_password);
        assert!(result.is_err());
    }

    #[test]
    fn test_save_load_encrypted() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("identity.json");

        let mut identity = Identity::new_with_plaintext("Test User".to_string()).unwrap();
        let original_pem = identity.private_key().unwrap();
        let password = test_password();

        // Encrypt and save
        identity.encrypt(&password).unwrap();
        identity.save(&path).unwrap();

        // Load and decrypt
        let mut loaded = Identity::load(&path).unwrap();
        assert!(loaded.private_key_pem_plaintext.is_none()); // Should not be available yet
        loaded.decrypt(&password).unwrap();

        assert_eq!(loaded.private_key().unwrap(), original_pem);
    }

    #[test]
    fn test_save_unencrypted_fails() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("identity_unsafe.json");
        let identity = Identity::new_with_plaintext("Unsafe User".to_string()).unwrap();

        // Should fail to save because it is not encrypted
        assert!(identity.save(&path).is_err());
    }

    #[test]
    fn test_invite_link_generation() {
        let identity = Identity::new_with_plaintext("Test User".to_string()).unwrap();
        let link = identity.generate_invite_link(None).unwrap();

        assert!(link.starts_with("chat-p2p://invite/"));
        assert!(link.len() > 50); // Should be a substantial base64 string
    }

    #[test]
    fn test_invite_link_includes_address_when_provided() {
        let identity = Identity::new_with_plaintext("Tester".to_string()).unwrap();
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

    // ============================================================================
    // Signed Invite Link Tests (v2 format)
    // ============================================================================

    #[test]
    fn test_signed_invite_link_generation() {
        let identity = Identity::new_with_plaintext("Signed Test User".to_string()).unwrap();
        let link = identity.generate_signed_invite_link(None).unwrap();

        // V2 format should contain /v2/
        assert!(link.starts_with("chat-p2p://invite/v2/"));
        assert!(link.len() > 50);

        // The encoded part should use URL-safe base64 (no +, /, or =)
        let encoded = link.strip_prefix("chat-p2p://invite/v2/").unwrap();
        assert!(!encoded.contains('+'));
        assert!(!encoded.contains('/'));
        // URL-safe base64 without padding means no trailing =
    }

    #[test]
    fn test_signed_invite_link_includes_timestamp() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let identity = Identity::new_with_plaintext("Timestamp Test".to_string()).unwrap();
        let link = identity.generate_signed_invite_link(None).unwrap();

        // Decode and verify timestamp is present
        let encoded = link.strip_prefix("chat-p2p://invite/v2/").unwrap();
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .unwrap();

        #[derive(serde::Deserialize)]
        struct SignedInvite {
            payload: serde_json::Value,
            #[allow(dead_code)]
            signature: Vec<u8>,
        }

        let invite: SignedInvite = serde_json::from_slice(&decoded).unwrap();
        let payload = &invite.payload;

        // Verify timestamp field exists and is recent
        let timestamp = payload.get("timestamp").and_then(|v| v.as_u64()).unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Timestamp should be within last 10 seconds
        assert!(now - timestamp < 10);
    }

    #[test]
    fn test_signed_invite_link_includes_nonce() {
        let identity = Identity::new_with_plaintext("Nonce Test".to_string()).unwrap();
        let link1 = identity.generate_signed_invite_link(None).unwrap();
        let link2 = identity.generate_signed_invite_link(None).unwrap();

        // Decode both and verify nonces are different (uniqueness)
        let decode_link = |link: &str| -> String {
            let encoded = link.strip_prefix("chat-p2p://invite/v2/").unwrap();
            let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(encoded)
                .unwrap();

            #[derive(serde::Deserialize)]
            struct SignedInvite {
                payload: serde_json::Value,
            }

            let invite: SignedInvite = serde_json::from_slice(&decoded).unwrap();
            invite
                .payload
                .get("nonce")
                .and_then(|v| v.as_str())
                .unwrap()
                .to_string()
        };

        let nonce1 = decode_link(&link1);
        let nonce2 = decode_link(&link2);

        // Nonces should be different (ephemeral keys are random)
        assert_ne!(nonce1, nonce2);

        // Nonces should be non-empty hex strings
        assert!(!nonce1.is_empty());
        assert!(!nonce2.is_empty());
        hex::decode(&nonce1).unwrap(); // Should be valid hex
        hex::decode(&nonce2).unwrap();
    }

    #[test]
    fn test_signed_invite_link_with_address() {
        let identity = Identity::new_with_plaintext("Address Test".to_string()).unwrap();
        let addr = Some("192.168.1.100:9000".to_string());
        let link = identity.generate_signed_invite_link(addr.clone()).unwrap();

        // Decode and verify address is included
        let encoded = link.strip_prefix("chat-p2p://invite/v2/").unwrap();
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .unwrap();

        #[derive(serde::Deserialize)]
        struct SignedInvite {
            payload: serde_json::Value,
        }

        let invite: SignedInvite = serde_json::from_slice(&decoded).unwrap();
        let payload = &invite.payload;

        assert_eq!(
            payload.get("address").and_then(|v| v.as_str()),
            addr.as_deref()
        );
    }

    #[test]
    fn test_signed_invite_payload_contains_identity_info() {
        let identity = Identity::new_with_plaintext("Info Test".to_string()).unwrap();
        let link = identity.generate_signed_invite_link(None).unwrap();

        // Decode and verify all expected fields are present
        let encoded = link.strip_prefix("chat-p2p://invite/v2/").unwrap();
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .unwrap();

        #[derive(serde::Deserialize)]
        struct SignedInvite {
            payload: serde_json::Value,
            signature: Vec<u8>,
        }

        let invite: SignedInvite = serde_json::from_slice(&decoded).unwrap();
        let payload = &invite.payload;

        // Verify all required fields are present
        assert_eq!(payload.get("version").and_then(|v| v.as_u64()), Some(2));
        assert!(payload.get("timestamp").and_then(|v| v.as_u64()).is_some());
        assert!(payload.get("nonce").and_then(|v| v.as_str()).is_some());
        assert_eq!(
            payload.get("name").and_then(|v| v.as_str()),
            Some("Info Test")
        );
        assert_eq!(
            payload.get("fingerprint").and_then(|v| v.as_str()),
            Some(identity.fingerprint.as_str())
        );
        assert_eq!(
            payload.get("public_key").and_then(|v| v.as_str()),
            Some(identity.public_key_pem.as_str())
        );

        // Signature should not be empty
        assert!(!invite.signature.is_empty());
    }

    #[test]
    fn test_signed_relay_invite_link_includes_relay_fields() {
        let identity = Identity::new_with_plaintext("Relay User".to_string()).unwrap();
        let link = identity
            .generate_signed_invite_link_with_route(
                None,
                Some("relay.example.com:23456".to_string()),
                Some("0123456789abcdef0123456789abcdef".to_string()),
            )
            .unwrap();

        assert!(link.starts_with("chat-p2p://invite/v2/"));
        let encoded = link.strip_prefix("chat-p2p://invite/v2/").unwrap();
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .unwrap();
        let json = String::from_utf8(decoded).unwrap();
        assert!(json.contains("relay.example.com:23456"));
        assert!(json.contains("0123456789abcdef0123456789abcdef"));
        assert!(json.contains("\"version\":3"));
    }

    #[test]
    fn test_remove_password_is_rejected() {
        let mut identity = Identity::new_with_plaintext("Protected User".to_string()).unwrap();
        identity.encrypt(&test_password()).unwrap();

        let err = identity
            .remove_password(&test_password())
            .expect_err("password removal must be rejected");
        assert!(err.to_string().contains("not supported"));
        assert!(identity.encrypted_private_key.is_some());
        assert!(identity.salt.is_some());
        assert!(identity.nonce.is_some());
    }
}
