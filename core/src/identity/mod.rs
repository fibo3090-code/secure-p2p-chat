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
use anyhow::{anyhow, bail, Result};
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

/// Upper bounds on the Argon2 cost parameters this app will honour from an
/// identity file. `Params::new` enforces only the RFC's own limits, which run
/// to terabytes of memory, so a corrupted or hostile `identity.json` could
/// otherwise turn an unlock into a multi-gigabyte allocation — and since the
/// recorded parameters are the only ones tried, nothing else would catch it.
///
/// These sit far above anything the app writes (64 MiB, t=3, p=4), so a future
/// release can raise its own costs without tripping them. There is deliberately
/// no *lower* bound: weak parameters only weaken the file that carries them,
/// and a floor would reject parameter sets a future release might legitimately
/// choose.
const MAX_ARGON_M_COST_KIB: u32 = 1024 * 1024; // 1 GiB
const MAX_ARGON_T_COST: u32 = 16;
const MAX_ARGON_P_COST: u32 = 16;

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
    ///
    /// Rejects anything shorter than [`crate::MIN_PASSWORD_LEN`]. The floor is
    /// enforced here rather than in each front-end because this password is the
    /// only thing standing between an attacker with the disk and the identity
    /// key — a UI-only check would be advisory, and one front-end forgetting it
    /// would silently weaken every user it onboards. `decrypt` has no such
    /// check, so identities created under an older, weaker floor still open.
    pub fn encrypt(&mut self, password: &str) -> Result<()> {
        if password.chars().count() < crate::MIN_PASSWORD_LEN {
            bail!(
                "Password must be at least {} characters",
                crate::MIN_PASSWORD_LEN
            );
        }
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

    /// Re-wrap the private key under a new password.
    ///
    /// `current` is verified against the stored wrapper first. That check is not
    /// a formality: without it anyone who reaches an unlocked machine could
    /// rotate the password and lock the owner out of the identity — and the
    /// identity is the only key to their message history.
    ///
    /// The **history stays readable**: [`Identity::history_key`] is derived from
    /// the private key, which a password change does not touch. Only the wrapper
    /// around that key is replaced (fresh Argon2 salt, fresh nonce).
    ///
    /// On success the identity is left *unlocked* under the new password. On any
    /// failure it is untouched and still opens with `current`, so a rejected new
    /// password (too short) or a mistyped current one costs nothing. Callers must
    /// still persist it — see [`Identity::save`] — and should not swap a shared
    /// identity for the re-wrapped one until that save has succeeded, or memory
    /// and disk end up wanting different passwords.
    pub fn change_password(&mut self, current: &str, new: &str) -> Result<()> {
        if !self.is_encrypted() {
            bail!("This identity has no password yet — set one instead of changing it");
        }
        // Verify against the stored wrapper, not against "are we unlocked".
        self.decrypt(current)
            .map_err(|_| anyhow!("Current password is incorrect"))?;
        // `encrypt` enforces the length floor and consumes the plaintext key.
        self.encrypt(new)?;
        // Re-open under the new password: proves the new wrapper works before
        // the caller writes it to disk, and keeps this session unlocked.
        self.decrypt(new)
            .map_err(|e| anyhow!("Re-encrypted identity did not open with the new password: {e}"))
    }

    /// Derive the wrapping key with `argon` and open the encrypted private key.
    /// `None` means these parameters do not open it — a wrong password, or the
    /// wrong configuration for this file. The two are deliberately
    /// indistinguishable to the caller.
    fn unwrap_private_key(
        argon: &Argon2<'_>,
        password: &str,
        salt: &[u8],
        nonce: &[u8],
        ciphertext: &[u8],
    ) -> Option<Zeroizing<String>> {
        let mut key_bytes = Zeroizing::new([0u8; KEY_SIZE]);
        argon
            .hash_password_into(password.as_bytes(), salt, &mut key_bytes[..])
            .ok()?;
        let cipher = ChaCha20Poly1305::new((&key_bytes[..]).into());
        let plaintext = cipher.decrypt(nonce.into(), ciphertext).ok()?;
        String::from_utf8(plaintext).ok().map(Zeroizing::new)
    }

    /// Decrypt the private key with a password.
    ///
    /// SECURITY: when the file records its Argon2 parameters, they are the
    /// *only* ones tried. Re-deriving with a different configuration on failure
    /// would make an unlock cost one, two or three Argon2 passes depending on
    /// how the file happened to be written (~1 s each), so anyone able to time
    /// the unlock would learn which configuration protects the key — useful
    /// context for an offline attack on a stolen identity file.
    pub fn decrypt(&mut self, password: &str) -> Result<()> {
        let salt = self
            .salt
            .as_ref()
            .ok_or_else(|| anyhow!("Salt not found"))?;
        let nonce = self
            .nonce
            .as_ref()
            .ok_or_else(|| anyhow!("Nonce not found"))?;
        let ciphertext = self
            .encrypted_private_key
            .as_ref()
            .ok_or_else(|| anyhow!("Encrypted private key not found"))?;
        let wrong_password = || anyhow!("Decryption failed (likely wrong password)");

        if let Some(ap) = &self.argon_params {
            if ap.m_cost_kib > MAX_ARGON_M_COST_KIB
                || ap.t_cost > MAX_ARGON_T_COST
                || ap.p_cost > MAX_ARGON_P_COST
            {
                return Err(anyhow!(
                    "Stored Argon2 parameters are out of range (m={} KiB, t={}, p={})",
                    ap.m_cost_kib,
                    ap.t_cost,
                    ap.p_cost
                ));
            }
            let params = Params::new(
                ap.m_cost_kib,
                ap.t_cost,
                ap.p_cost,
                Some(ap.output_len as usize),
            )
            .map_err(|e| anyhow!("Stored Argon2 parameters are unusable: {}", e))?;
            let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
            let pem = Self::unwrap_private_key(&argon, password, salt, nonce, ciphertext)
                .ok_or_else(wrong_password)?;
            self.private_key_pem_plaintext = Some(pem);
            return Ok(());
        }

        // No recorded parameters: the file predates `argon_params` and was
        // written by one of two earlier schemes. Trying both leaks nothing an
        // attacker holding the file does not already know — the absence of the
        // field is right there in the JSON. Every identity `encrypt()` writes
        // records its parameters, so this path only ever runs for files that
        // have not been re-encrypted since.
        let legacy_schemes = [
            Argon2::new(
                Algorithm::Argon2id,
                Version::V0x13,
                Params::new(65536, 3, 4, Some(KEY_SIZE))
                    .map_err(|e| anyhow!("Invalid Argon2 parameters: {}", e))?,
            ),
            Argon2::default(),
        ];
        for argon in &legacy_schemes {
            if let Some(pem) = Self::unwrap_private_key(argon, password, salt, nonce, ciphertext) {
                self.private_key_pem_plaintext = Some(pem);
                return Ok(());
            }
        }

        Err(wrong_password())
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
        self.generate_signed_invite_link_with_addresses(
            address.into_iter().collect(),
            relay_server,
            relay_token,
        )
    }

    /// Generate a signed invite carrying multiple direct-connect candidate
    /// addresses in priority order (e.g. an internet-reachable address first,
    /// then a LAN one), plus an optional relay route.
    ///
    /// Wire back-compat: `address` is set to the first candidate so older
    /// clients (which only read the single `address` field) still work, while
    /// the full ordered list travels in the `addresses` field, which is omitted
    /// from the signed bytes when it adds nothing beyond `address` (0 or 1
    /// candidate). Because the verifier mirrors the same `skip_serializing_if`,
    /// invites minted before this field existed re-serialize identically and
    /// their signatures keep verifying.
    pub fn generate_signed_invite_link_with_addresses(
        &self,
        addresses: Vec<String>,
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
            // MUST stay last with this exact skip rule so pre-existing invites
            // (no `addresses` key) re-serialize byte-identically and verify.
            #[serde(default, skip_serializing_if = "Vec::is_empty")]
            addresses: Vec<String>,
        }

        // First candidate is the primary `address` for old-client back-compat;
        // only carry the full list when it adds something beyond that.
        let primary = addresses.first().cloned();
        let multi = if addresses.len() > 1 {
            addresses
        } else {
            Vec::new()
        };

        let payload = SignedInvitePayload {
            version: if !multi.is_empty() {
                4
            } else if relay_server.is_some() || relay_token.is_some() {
                3
            } else {
                2
            },
            timestamp,
            nonce,
            name: self.name.clone(),
            address: primary,
            relay_server,
            relay_token,
            fingerprint: self.fingerprint.clone(),
            public_key: self.public_key_pem.clone(),
            addresses: multi,
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

    /// Change the display name (shown in invite links and the local UI).
    /// Validation only — persist with [`Identity::save`] afterwards.
    pub fn set_name(&mut self, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Err(anyhow!("Display name cannot be empty"));
        }
        if name.chars().count() > 48 {
            return Err(anyhow!("Display name is too long (max 48 characters)"));
        }
        self.name = name.to_string();
        Ok(())
    }

    /// Persist the identity.
    ///
    /// SECURITY: enforces encryption and strictly secure file permissions (0600
    /// on Unix). The write is **atomic** — see [`write_file_atomic`]. This file
    /// is the only key to the user's message history, so a half-written one is
    /// equivalent to destroying the account.
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
        crate::util::write_file_atomic(path, content.as_bytes())?;

        tracing::info!("Saved identity: {} to {}", self.name, path.display());
        Ok(())
    }

    /// Load the identity from `data_dir`, or create one if there is none.
    ///
    /// Returns `(identity, is_new)`.
    ///
    /// **A present-but-unreadable identity file is a hard error**, never a
    /// reason to mint a new one. Generating a fresh keypair there looks like a
    /// graceful fallback and is actually the worst outcome the app can produce:
    /// the history key is derived from the private key
    /// ([`Identity::history_key`]), so a new identity makes the existing
    /// encrypted history permanently unreadable, and the new fingerprint breaks
    /// TOFU trust with every contact who had verified the old one — all
    /// presented to the user as a blank, freshly-installed app. Failing loudly
    /// keeps the file intact so it can be restored from a backup.
    pub fn get_or_create(data_dir: &Path, default_name: &str) -> Result<(Self, bool)> {
        let identity_path = data_dir.join("identity.json");

        if !identity_path.exists() {
            tracing::info!("No existing identity found, creating new one");
            return Ok((Self::new_with_plaintext(default_name.to_string())?, true));
        }

        match Self::load(&identity_path) {
            Ok(identity) => {
                tracing::info!("Using existing identity: {}", identity.name);
                Ok((identity, false))
            }
            Err(e) => {
                tracing::error!(
                    path = %identity_path.display(),
                    error = %e,
                    "identity file exists but could not be read; refusing to create a new identity"
                );
                Err(anyhow!(
                    "Your identity file at {} exists but could not be read: {e}\n\n\
                     Refusing to create a new identity, because that would abandon this \
                     one permanently: your encrypted message history would become \
                     unreadable, and every contact who verified you would see a changed \
                     fingerprint.\n\n\
                     Restore the file from a backup (Settings > Identity backup), or move \
                     it aside if you really do want to start over as a new identity.",
                    identity_path.display()
                ))
            }
        }
    }

    /// Check if the identity's private key is encrypted and not currently decrypted.
    pub fn is_locked(&self) -> bool {
        self.encrypted_private_key.is_some() && self.private_key_pem_plaintext.is_none()
    }

    /// True if the private key has been encrypted with a password (persisted form).
    pub fn is_encrypted(&self) -> bool {
        self.encrypted_private_key.is_some()
    }

    /// Derive a stable 32-byte history encryption key from the private key.
    /// Requires the identity to be unlocked (private key available).
    ///
    /// Note the coupling this creates: **the identity file is the only key to
    /// the message history.** Losing or corrupting it is unrecoverable, which
    /// is why [`Identity::save`] writes atomically and
    /// [`Identity::get_or_create`] refuses to replace an unreadable one.
    ///
    /// ## This derivation is frozen
    ///
    /// The key is `SHA-256` over the PKCS#8 DER encoding of the private key.
    /// That means it depends on the *encoding* an external crate produces, not
    /// just on the key: if a future `rsa` release re-orders an optional field or
    /// changes how it writes the algorithm parameters, the same private key
    /// derives a different history key and every stored message silently stops
    /// decrypting — the same total loss as a replaced identity, arriving as a
    /// routine dependency bump.
    ///
    /// The derivation therefore cannot be changed, and it is pinned by a
    /// known-answer test (`history_key_derivation_is_frozen`) over a fixed key
    /// with a fixed expected output. A dependency that changes the encoding
    /// fails that test in CI instead of failing users' histories in the field.
    /// If it ever does fail, the fix is a migration that re-encrypts existing
    /// history under the new key — never a quiet change to this function.
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

    /// The RSA key the history-key known-answer test is pinned to, given as raw
    /// components rather than a PEM blob: this is a published test vector, and
    /// writing it as one avoids putting something shaped like a real private key
    /// in the repository. 1024-bit because the test only needs a stable
    /// encoding, not security.
    const EXPECTED_HISTORY_KEY: &str =
        "7ed689da4592916beda567f25fa36d746b2e2cccfa313ba8e08d0c2dbbdaa9f3";

    const KAT_N: &str = "bad7050643ee4c36bfa78df11a6c606b3bdf97f0889cd8e547a4a4c6b1c4d4a6a88b250ca40e836aaa0fdf03c4057af6192ccad1bb07d0b88ec1d0a74a457d0700160c36ff210ad27b2f6646e8eca14b5ec45c949d8cb78ee7bd69a3b36c56d738f0635247facf33cc2841666381a2a442607a3201c1b70ac8fbf6f436720ce7";
    const KAT_E: &str = "10001";
    const KAT_D: &str = "6dc05aaa3883256fcf9afc0d11c971c5ebf0c6cebb60ef2397b70637d53adaf35ef4057a6c703e100cffafb0059876875378755747b72a8b0f0898a97c3e5f5717bec35db9609ade4e98238c39ddac14895384c9635850e48d6284e185059a542e6c915aa6cd24bcddf87f7b7269520ab2dc5645a8c79b29eb873da9714e7761";
    const KAT_P: &str = "f529b9b1287b23fab001ce11821ae25e5c9e996c5362802a777541a2e6c4675c50ca7255352f604e9e7a8cdcc1a7f6cf4685418d867e3768acbff9c697bc1a11";
    const KAT_Q: &str = "c3194ed799ae0bd2cd9d9aa0711b7adb3b07b5f8a7e3df5b08a9c2e28cc1b025efbb10eede467df86cd2c15a2fa5a0574315350d35be821a7eb2a7954c5bff77";

    fn kat_identity() -> Identity {
        use rsa::traits::PublicKeyParts;
        use rsa::BigUint;

        let big = |h: &str| BigUint::parse_bytes(h.as_bytes(), 16).unwrap();
        let key = RsaPrivateKey::from_components(
            big(KAT_N),
            big(KAT_E),
            big(KAT_D),
            vec![big(KAT_P), big(KAT_Q)],
        )
        .unwrap();
        assert_eq!(key.size() * 8, 1024, "test vector key must parse intact");

        let public_key_pem = RsaPublicKey::from(&key)
            .to_public_key_pem(LineEnding::LF)
            .unwrap();
        let fingerprint = Identity::calculate_fingerprint(&public_key_pem);
        Identity {
            id: Uuid::nil(),
            name: "KAT".to_string(),
            created_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
            encrypted_private_key: None,
            salt: None,
            nonce: None,
            argon_params: None,
            public_key_pem,
            fingerprint,
            private_key_pem_plaintext: Some(Zeroizing::new(
                key.to_pkcs8_pem(LineEnding::LF).unwrap().to_string(),
            )),
        }
    }

    /// `history_key` hashes the PKCS#8 DER encoding of the private key, so its
    /// output is hostage to how the `rsa` crate serialises. If that encoding
    /// ever shifts, every existing history becomes undecryptable — and it would
    /// do so silently, on a dependency bump, looking exactly like data loss.
    ///
    /// This pins the derivation to a fixed answer. A failure here is not a test
    /// to update: it means shipping that dependency change would destroy users'
    /// message history, and it needs a migration.
    #[test]
    fn history_key_derivation_is_frozen() {
        let identity = kat_identity();
        let key = identity.history_key().unwrap();
        assert_eq!(
            hex::encode(key),
            EXPECTED_HISTORY_KEY,
            "the history-key derivation changed: existing encrypted histories \
             would no longer decrypt. This needs a migration, not a new expectation."
        );
    }

    /// The same key must derive the same history key every time it is asked —
    /// the property the encrypted history depends on across restarts.
    #[test]
    fn history_key_is_stable_across_calls() {
        let identity = kat_identity();
        assert_eq!(
            identity.history_key().unwrap(),
            identity.history_key().unwrap()
        );
    }

    /// Two identities must not share a history key, or one user's identity file
    /// would open another's history.
    #[test]
    fn history_key_differs_between_identities() {
        let a = Identity::new_with_plaintext("A".to_string()).unwrap();
        let b = Identity::new_with_plaintext("B".to_string()).unwrap();
        assert_ne!(a.history_key().unwrap(), b.history_key().unwrap());
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

    /// The single most destructive thing this app could do is replace an
    /// identity it merely failed to *read*: the history key is derived from the
    /// private key, so a new identity makes every stored message permanently
    /// unreadable and breaks TOFU trust with every contact — while looking to
    /// the user like a fresh install.
    #[test]
    fn unreadable_identity_is_an_error_never_a_new_identity() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("identity.json");

        // A truncated file: exactly what an interrupted write used to leave.
        std::fs::write(&path, b"").unwrap();
        // `Identity` deliberately has no `Debug` (it holds key material), so
        // unwrap the error by matching rather than with `expect_err`.
        let msg = match Identity::get_or_create(dir.path(), "User") {
            Ok(_) => panic!("an unreadable identity must not be silently replaced"),
            Err(e) => e.to_string(),
        };
        assert!(
            msg.contains("could not be read"),
            "error should explain the problem: {msg}"
        );
        assert!(
            msg.to_lowercase().contains("backup"),
            "error should point at recovery: {msg}"
        );

        // Critically, the unreadable file is left exactly as it was, so it can
        // still be restored or inspected.
        assert!(path.exists());
        assert_eq!(std::fs::read(&path).unwrap(), b"");

        // Garbage that is not even JSON behaves the same way.
        std::fs::write(&path, b"{\"partial\": tru").unwrap();
        assert!(Identity::get_or_create(dir.path(), "User").is_err());
    }

    /// The no-file case is a genuinely new install and must still work.
    #[test]
    fn absent_identity_creates_a_new_one() {
        let dir = tempdir().unwrap();
        let (identity, is_new) = Identity::get_or_create(dir.path(), "Fresh").unwrap();
        assert!(is_new);
        assert_eq!(identity.name, "Fresh");
    }

    /// A saved identity round-trips and is reported as *not* new.
    #[test]
    fn existing_identity_is_loaded_not_recreated() {
        let dir = tempdir().unwrap();
        let mut identity = Identity::new_with_plaintext("Original".to_string()).unwrap();
        identity.encrypt(&test_password()).unwrap();
        let fingerprint = identity.fingerprint.clone();
        identity.save(&dir.path().join("identity.json")).unwrap();

        let (loaded, is_new) = Identity::get_or_create(dir.path(), "Should Not Be Used").unwrap();
        assert!(!is_new);
        assert_eq!(loaded.name, "Original");
        assert_eq!(
            loaded.fingerprint, fingerprint,
            "the fingerprint contacts verified must be preserved"
        );
    }

    /// `save` must never leave a half-written file behind, and must leave no
    /// temporary files cluttering the data directory.
    #[test]
    fn save_replaces_atomically_and_leaves_no_temp_files() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("identity.json");

        let mut first = Identity::new_with_plaintext("First".to_string()).unwrap();
        first.encrypt(&test_password()).unwrap();
        first.save(&path).unwrap();

        let mut second = Identity::new_with_plaintext("Second".to_string()).unwrap();
        second.encrypt(&test_password()).unwrap();
        second.save(&path).unwrap();

        // Overwriting an existing identity yields exactly that identity...
        let reloaded = Identity::load(&path).unwrap();
        assert_eq!(reloaded.name, "Second");

        // ...and exactly one file.
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            entries,
            vec!["identity.json".to_string()],
            "stray temp file"
        );
    }

    /// The password floor is a core-level invariant, not a UI suggestion: a
    /// front-end that forgets to validate must still be unable to create a
    /// weakly-protected keystore.
    #[test]
    fn encrypt_rejects_passwords_below_the_floor() {
        let mut identity = Identity::new_with_plaintext("Test User".to_string()).unwrap();
        let short = "a".repeat(crate::MIN_PASSWORD_LEN - 1);
        let err = identity
            .encrypt(&short)
            .expect_err("a short password must be rejected");
        assert!(
            err.to_string().contains("at least"),
            "error should say what the floor is, got: {err}"
        );
        // Nothing was consumed on the rejected attempt: the identity is still
        // usable and can be encrypted with an acceptable password.
        assert!(identity.private_key_pem_plaintext.is_some());
        identity
            .encrypt(&"a".repeat(crate::MIN_PASSWORD_LEN))
            .unwrap();
        assert!(identity.encrypted_private_key.is_some());
    }

    /// Raising the floor must never lock out an identity created under an older,
    /// weaker one, so `decrypt` deliberately carries no length check: a short
    /// password reaches the KDF and fails (or succeeds) on the ciphertext alone,
    /// never on policy.
    #[test]
    fn decrypt_applies_no_length_policy() {
        let mut identity = Identity::new_with_plaintext("Legacy User".to_string()).unwrap();
        identity.encrypt(&test_password()).unwrap();

        let short = "a".repeat(crate::MIN_PASSWORD_LEN - 1);
        let err = identity
            .decrypt(&short)
            .expect_err("a wrong password must fail");
        assert!(
            !err.to_string().contains("at least"),
            "decrypt must fail on the ciphertext, not on the length floor: {err}"
        );
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

    /// The whole point of a password change: the wrapper changes, the key does
    /// not — so every stored message stays readable afterwards.
    #[test]
    fn change_password_rewraps_the_same_key_and_keeps_the_history_key() {
        let mut identity = Identity::new_with_plaintext("Test User".to_string()).unwrap();
        let old = test_password();
        let new = test_password();
        identity.encrypt(&old).unwrap();
        identity.decrypt(&old).unwrap();
        let history_key_before = identity.history_key().unwrap();
        let pem_before = identity.private_key_pem_plaintext.clone().unwrap();

        identity.change_password(&old, &new).unwrap();

        // Still unlocked, same private key, same history key.
        assert!(!identity.is_locked());
        assert_eq!(
            identity.private_key_pem_plaintext.clone().unwrap(),
            pem_before
        );
        assert_eq!(
            identity.history_key().unwrap(),
            history_key_before,
            "a password change must never make the stored history unreadable"
        );

        // The new password opens it; the old one no longer does.
        let mut reopened = identity.clone();
        reopened.private_key_pem_plaintext = None;
        assert!(reopened.decrypt(&new).is_ok());
        let mut stale = identity.clone();
        stale.private_key_pem_plaintext = None;
        assert!(stale.decrypt(&old).is_err());
    }

    /// A wrong current password must not rotate anything — otherwise anyone at
    /// an unlocked machine could lock the owner out of their own identity.
    #[test]
    fn change_password_rejects_a_wrong_current_password() {
        let mut identity = Identity::new_with_plaintext("Test User".to_string()).unwrap();
        let old = test_password();
        identity.encrypt(&old).unwrap();
        identity.decrypt(&old).unwrap();

        let err = identity
            .change_password("not-the-password", &test_password())
            .expect_err("a wrong current password must be refused");
        assert!(err.to_string().contains("Current password is incorrect"));

        // Untouched: the old password still opens it.
        let mut check = identity.clone();
        check.private_key_pem_plaintext = None;
        assert!(check.decrypt(&old).is_ok());
    }

    /// The length floor applies to a changed password exactly as it does to a
    /// new one, and a rejected change leaves the old wrapper in place.
    #[test]
    fn change_password_enforces_the_length_floor_without_destroying_the_old() {
        let mut identity = Identity::new_with_plaintext("Test User".to_string()).unwrap();
        let old = test_password();
        identity.encrypt(&old).unwrap();
        identity.decrypt(&old).unwrap();

        let err = identity
            .change_password(&old, "short")
            .expect_err("a too-short new password must be refused");
        assert!(err.to_string().contains("at least"));

        let mut check = identity.clone();
        check.private_key_pem_plaintext = None;
        assert!(
            check.decrypt(&old).is_ok(),
            "a refused change must leave the identity openable with the old password"
        );
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
    fn test_multi_address_invite_carries_ordered_list_and_primary() {
        let identity = Identity::new_with_plaintext("Multi Addr".to_string()).unwrap();
        let link = identity
            .generate_signed_invite_link_with_addresses(
                vec![
                    "203.0.113.7:12345".to_string(),
                    "192.168.1.20:12345".to_string(),
                ],
                None,
                None,
            )
            .unwrap();

        let encoded = link.strip_prefix("chat-p2p://invite/v2/").unwrap();
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .unwrap();
        let invite: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        let payload = &invite["payload"];

        // Primary `address` is the first candidate (old-client back-compat)…
        assert_eq!(
            payload.get("address").and_then(|v| v.as_str()),
            Some("203.0.113.7:12345")
        );
        // …and the full ordered list travels in `addresses`, bumping to v4.
        let addrs: Vec<&str> = payload["addresses"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(addrs, vec!["203.0.113.7:12345", "192.168.1.20:12345"]);
        assert_eq!(payload.get("version").and_then(|v| v.as_u64()), Some(4));
    }

    #[test]
    fn test_single_address_invite_omits_addresses_key() {
        // Byte-format regression guard: a single-address invite must not emit
        // an `addresses` key at all, so its signed payload is byte-identical
        // to invites minted before the field existed (and old invites keep
        // verifying through the mirrored skip rule on the parse side).
        let identity = Identity::new_with_plaintext("Single Addr".to_string()).unwrap();
        for link in [
            identity
                .generate_signed_invite_link(Some("192.168.1.20:12345".to_string()))
                .unwrap(),
            identity.generate_signed_invite_link(None).unwrap(),
        ] {
            let encoded = link.strip_prefix("chat-p2p://invite/v2/").unwrap();
            let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(encoded)
                .unwrap();
            let invite: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
            assert!(
                invite["payload"].get("addresses").is_none(),
                "single-address invite must omit the addresses key"
            );
            assert_ne!(
                invite["payload"].get("version").and_then(|v| v.as_u64()),
                Some(4)
            );
        }
    }

    // ── Argon2 key-derivation compatibility ─────────────────────────────────
    //
    // This app has written three on-disk shapes over its life: keys wrapped
    // with `Argon2::default()`, keys wrapped with explicit strict parameters
    // but no record of them, and (current) keys wrapped with strict parameters
    // recorded in `argon_params`. The tests below pin down which of those
    // `decrypt` may still open, and — more importantly — that a recorded
    // parameter set is the *only* thing tried when it is present.

    /// Wrap the identity's private key the way a given Argon2 configuration
    /// would, without recording the parameters — i.e. reproduce an identity
    /// file as written by a release that predates `argon_params`.
    fn wrap_private_key_with(identity: &mut Identity, password: &str, argon: &Argon2<'_>) {
        let pem = identity
            .private_key_pem_plaintext
            .clone()
            .expect("fixture needs a plaintext key");
        let mut salt = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut salt);
        let mut key_bytes = Zeroizing::new([0u8; KEY_SIZE]);
        argon
            .hash_password_into(password.as_bytes(), &salt, &mut key_bytes[..])
            .expect("fixture key derivation");
        let cipher = ChaCha20Poly1305::new((&key_bytes[..]).into());
        let nonce = ChaCha20Poly1305::generate_nonce(&mut rand::rngs::OsRng);

        identity.encrypted_private_key = Some(
            cipher
                .encrypt(&nonce, pem.as_bytes())
                .expect("fixture encryption"),
        );
        identity.salt = Some(salt.to_vec());
        identity.nonce = Some(nonce.to_vec());
        identity.argon_params = None;
        identity.private_key_pem_plaintext = None;
    }

    /// The explicit parameters this app has used since the hardening pass.
    fn strict_argon() -> Argon2<'static> {
        Argon2::new(
            Algorithm::Argon2id,
            Version::V0x13,
            Params::new(65536, 3, 4, Some(KEY_SIZE)).unwrap(),
        )
    }

    #[test]
    fn stored_argon_params_are_the_only_ones_tried() {
        let mut identity = Identity::new_with_plaintext("Mismatched User".to_string()).unwrap();
        let password = test_password();

        // A key wrapped with the oldest scheme...
        wrap_private_key_with(&mut identity, &password, &Argon2::default());
        // ...in a file that claims the current parameters.
        identity.argon_params = Some(ArgonParams {
            m_cost_kib: 65536,
            t_cost: 3,
            p_cost: 4,
            output_len: KEY_SIZE as u32,
        });

        // Opening this anyway — by quietly re-deriving with other parameters —
        // is what made unlock time reveal which configuration protects the key.
        // A recorded parameter set that does not work is a failure, full stop.
        assert!(
            identity.decrypt(&password).is_err(),
            "decrypt fell back to parameters other than the recorded ones"
        );
    }

    #[test]
    fn absurd_stored_argon_params_are_refused() {
        let mut identity = Identity::new_with_plaintext("Hostile File".to_string()).unwrap();
        let password = test_password();
        identity.encrypt(&password).unwrap();

        // 2 GiB of Argon2 memory. The RFC allows it, so `Params::new` is happy;
        // honouring it would have unlock allocate 2 GiB from a file the app
        // never wrote. Now that stored parameters are the only ones tried, they
        // are the only thing standing between a corrupt file and that
        // allocation.
        identity.argon_params.as_mut().unwrap().m_cost_kib = 2 * 1024 * 1024;

        let err = identity
            .decrypt(&password)
            .expect_err("out-of-range parameters must be refused");
        assert!(
            err.to_string().contains("out of range"),
            "expected a parameter-range error, got: {err}"
        );
    }

    #[test]
    fn identity_predating_stored_params_still_opens_default_scheme() {
        let mut identity = Identity::new_with_plaintext("Oldest User".to_string()).unwrap();
        let original_pem = identity.private_key_pem_plaintext.clone().unwrap();
        let password = test_password();
        wrap_private_key_with(&mut identity, &password, &Argon2::default());
        assert!(identity.argon_params.is_none());

        identity
            .decrypt(&password)
            .expect("legacy identity must open");
        assert_eq!(identity.private_key_pem_plaintext.unwrap(), original_pem);
    }

    #[test]
    fn identity_predating_stored_params_still_opens_strict_scheme() {
        let mut identity = Identity::new_with_plaintext("Interim User".to_string()).unwrap();
        let original_pem = identity.private_key_pem_plaintext.clone().unwrap();
        let password = test_password();
        wrap_private_key_with(&mut identity, &password, &strict_argon());
        assert!(identity.argon_params.is_none());

        identity
            .decrypt(&password)
            .expect("legacy identity must open");
        assert_eq!(identity.private_key_pem_plaintext.unwrap(), original_pem);
    }

    #[test]
    fn wrong_password_on_a_legacy_identity_still_fails() {
        let mut identity = Identity::new_with_plaintext("Oldest User".to_string()).unwrap();
        let password = test_password();
        wrap_private_key_with(&mut identity, &password, &Argon2::default());

        assert!(identity.decrypt(&test_password()).is_err());
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
