//! User identity: name, RSA key pair (private key encrypted at rest with
//! Argon2id + ChaCha20-Poly1305), fingerprint and invite links. Stored as
//! `identity.json` in the data directory.
pub mod invite;

use anyhow::{anyhow, bail, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    aead::{Aead, AeadCore, KeyInit},
    ChaCha20Poly1305,
};
use rand::RngCore;
use rsa::{
    pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePrivateKey, EncodePublicKey, LineEnding},
    PrivateKey as RsaPrivateKey, PublicKey as RsaPublicKey,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use uuid::Uuid;
use zeroize::Zeroizing;

const KEY_SIZE: usize = 32; // 256-bit key
const NONCE_SIZE: usize = 12; // ChaCha20-Poly1305, 96-bit

/// The Argon2id costs `encrypt` writes (and the unrecorded "strict" legacy
/// scheme): 64 MiB, t=3, p=4.
const ARGON_M_COST_KIB: u32 = 65536;
const ARGON_T_COST: u32 = 3;
const ARGON_P_COST: u32 = 4;

/// Upper bounds on Argon2 costs honoured from an identity file, so a corrupt or
/// hostile file cannot turn unlock into a huge allocation (`Params::new` allows
/// terabytes). Far above what the app writes; deliberately no lower bound.
const MAX_ARGON_M_COST_KIB: u32 = 1024 * 1024; // 1 GiB
const MAX_ARGON_T_COST: u32 = 16;
const MAX_ARGON_P_COST: u32 = 16;

/// User identity with RSA key pair.
#[derive(Clone, Serialize, Deserialize)]
pub struct Identity {
    pub id: Uuid,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,

    /// RSA private key encrypted with ChaCha20-Poly1305.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_private_key: Option<Vec<u8>>,
    /// Argon2 salt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub salt: Option<Vec<u8>>,
    /// ChaCha20-Poly1305 nonce.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonce: Option<Vec<u8>>,
    /// Argon2 parameters the wrapping key was derived with.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argon_params: Option<ArgonParams>,

    /// RSA public key, PKCS#8 PEM.
    pub public_key_pem: String,
    /// SHA-256 fingerprint of the public key (hex).
    pub fingerprint: String,

    /// Decrypted private key, wiped on drop. SECURITY: never serialized; only
    /// read from legacy files that stored it in plaintext.
    #[serde(
        skip_serializing,
        default,
        deserialize_with = "deserialize_plaintext_key"
    )]
    private_key_pem_plaintext: Option<Zeroizing<String>>,
}

fn deserialize_plaintext_key<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Zeroizing<String>>, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.map(Zeroizing::new))
}

/// Serializable representation of Argon2 parameters
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ArgonParams {
    pub m_cost_kib: u32,
    pub t_cost: u32,
    pub p_cost: u32,
    pub output_len: u32,
}

/// The one `decrypt` failure that a different password could fix: the stored
/// parameters are sound and the key simply did not open. Every other error
/// from `decrypt` means the file itself is damaged, and the UIs say so instead
/// of asking for the password again (TAURI-20). Test with
/// `err.is::<WrongPassword>()`.
#[derive(Debug)]
pub struct WrongPassword;

impl std::fmt::Display for WrongPassword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Decryption failed (likely wrong password)")
    }
}

impl std::error::Error for WrongPassword {}

/// Argon2id with the costs `encrypt` writes.
fn strict_argon() -> Result<Argon2<'static>> {
    let params = Params::new(ARGON_M_COST_KIB, ARGON_T_COST, ARGON_P_COST, Some(KEY_SIZE))
        .map_err(|e| anyhow!("Invalid Argon2 parameters: {}", e))?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

impl Identity {
    /// Create a new identity; the private key is not retained.
    pub fn new(name: String) -> Result<Self> {
        Self::generate(name, false)
    }

    /// Create an identity that keeps its plaintext private key in memory, for
    /// callers that need it right away (setup flows, tests).
    pub fn new_with_plaintext(name: String) -> Result<Self> {
        Self::generate(name, true)
    }

    fn generate(name: String, keep_plaintext: bool) -> Result<Self> {
        tracing::info!("Generating new identity (plaintext kept: {keep_plaintext}) for: {name}");
        let private_key = crate::core::crypto::generate_rsa_keypair(2048)?;
        let public_key_pem = RsaPublicKey::from(&private_key).to_public_key_pem(LineEnding::LF)?;
        let private_key_pem_plaintext = if keep_plaintext {
            Some(Zeroizing::new(
                private_key.to_pkcs8_pem(LineEnding::LF)?.to_string(),
            ))
        } else {
            None
        };

        Ok(Self {
            id: Uuid::new_v4(),
            name,
            created_at: chrono::Utc::now(),
            encrypted_private_key: None,
            salt: None,
            argon_params: None,
            nonce: None,
            fingerprint: Self::calculate_fingerprint(&public_key_pem)?,
            public_key_pem,
            private_key_pem_plaintext,
        })
    }

    /// See [`crate::core::crypto::fingerprint_pubkey`].
    fn calculate_fingerprint(public_key_pem: &str) -> Result<String> {
        crate::core::crypto::fingerprint_pubkey(public_key_pem.as_bytes())
    }

    /// Encrypt the private key with a password.
    ///
    /// Rejects anything shorter than [`crate::MIN_PASSWORD_LEN`] here, not in
    /// the UIs, so no front-end can create a weak keystore. `decrypt` has no
    /// such check, so identities made under an older floor still open.
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

        let (ciphertext, salt, nonce) =
            Self::wrap_private_key(&strict_argon()?, password, plaintext_pem)?;
        self.encrypted_private_key = Some(ciphertext);
        self.salt = Some(salt);
        self.nonce = Some(nonce);
        self.argon_params = Some(ArgonParams {
            m_cost_kib: ARGON_M_COST_KIB,
            t_cost: ARGON_T_COST,
            p_cost: ARGON_P_COST,
            output_len: KEY_SIZE as u32,
        });
        self.private_key_pem_plaintext = None;
        Ok(())
    }

    /// Encrypt `pem` under a key derived with `argon` from `password`, with a
    /// fresh salt and nonce. Returns `(ciphertext, salt, nonce)`.
    fn wrap_private_key(
        argon: &Argon2<'_>,
        password: &str,
        pem: &str,
    ) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
        let mut salt = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut salt);
        let mut key_bytes = Zeroizing::new([0u8; KEY_SIZE]);
        #[cfg(test)]
        tests::derivations::bump();
        argon
            .hash_password_into(password.as_bytes(), &salt, &mut key_bytes[..])
            .map_err(|e| anyhow!("Failed to derive key with Argon2: {}", e))?;

        let cipher = ChaCha20Poly1305::new((&key_bytes[..]).into());
        let nonce = ChaCha20Poly1305::generate_nonce(&mut rand::rngs::OsRng);
        let ciphertext = cipher
            .encrypt(&nonce, pem.as_bytes())
            .map_err(|e| anyhow!("Encryption failed: {}", e))?;
        Ok((ciphertext, salt.to_vec(), nonce.to_vec()))
    }

    /// Set the first password and **stay unlocked**: [`Identity::encrypt`]
    /// without discarding the key in memory. Setup used to `encrypt` and then
    /// `decrypt` just to get the key back, paying for Argon2 twice (PERF-12).
    /// The file written by [`Identity::save`] is the same either way — the
    /// plaintext key is never serialised.
    pub fn protect(&mut self, password: &str) -> Result<()> {
        let pem = self.private_key_pem_plaintext.clone();
        self.encrypt(password)?;
        self.private_key_pem_plaintext = pem;
        Ok(())
    }

    /// Re-wrap the private key under a new password after verifying `current`
    /// against the stored wrapper (so someone at an unlocked machine cannot
    /// lock the owner out). The key itself, and so [`Identity::history_key`],
    /// is unchanged.
    ///
    /// On success the identity is unlocked under the new password; on failure
    /// it is untouched. Callers must still [`Identity::save`] it, and should
    /// not swap a shared identity for this one until the save succeeds.
    pub fn change_password(&mut self, current: &str, new: &str) -> Result<()> {
        if !self.is_encrypted() {
            bail!("This identity has no password yet — set one instead of changing it");
        }
        self.decrypt(current)
            .map_err(|_| anyhow!("Current password is incorrect"))?;
        // `encrypt` enforces the floor; re-opening proves the new wrapper works.
        self.encrypt(new)?;
        self.decrypt(new)
            .map_err(|e| anyhow!("Re-encrypted identity did not open with the new password: {e}"))
    }

    /// Open the encrypted private key with `argon`. `None` means a wrong
    /// password or wrong parameters, deliberately indistinguishable.
    fn unwrap_private_key(
        argon: &Argon2<'_>,
        password: &str,
        salt: &[u8],
        nonce: &[u8],
        ciphertext: &[u8],
    ) -> Option<Zeroizing<String>> {
        let mut key_bytes = Zeroizing::new([0u8; KEY_SIZE]);
        #[cfg(test)]
        tests::derivations::bump();
        argon
            .hash_password_into(password.as_bytes(), salt, &mut key_bytes[..])
            .ok()?;
        let cipher = ChaCha20Poly1305::new((&key_bytes[..]).into());
        // The nonce is read from the file. `.into()` on an unchecked slice
        // asserts its length and panics; a damaged file must be an error.
        if nonce.len() != NONCE_SIZE {
            return None;
        }
        let plaintext = cipher.decrypt(nonce.into(), ciphertext).ok()?;
        String::from_utf8(plaintext).ok().map(Zeroizing::new)
    }

    /// Decrypt the private key with a password.
    ///
    /// SECURITY: recorded Argon2 parameters are the *only* ones tried; falling
    /// back to others would let unlock timing reveal which scheme protects the
    /// key.
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
        // A nonce of the wrong length is damage, not a password problem: say
        // so, rather than letting the unwrap below fail as if the password
        // were wrong (TAURI-20).
        if nonce.len() != NONCE_SIZE {
            bail!(
                "Stored nonce is {} bytes, expected {NONCE_SIZE}",
                nonce.len()
            );
        }
        let pem = if let Some(ap) = &self.argon_params {
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
            Self::unwrap_private_key(&argon, password, salt, nonce, ciphertext)
        } else {
            // No recorded parameters: a file from before `argon_params`, written
            // by one of two earlier schemes. Trying both leaks nothing — the
            // missing field is visible in the JSON.
            [strict_argon()?, Argon2::default()]
                .iter()
                .find_map(|argon| {
                    Self::unwrap_private_key(argon, password, salt, nonce, ciphertext)
                })
        };
        let pem = pem.ok_or_else(|| anyhow::Error::new(WrongPassword))?;
        self.private_key_pem_plaintext = Some(pem);
        Ok(())
    }

    /// Unsupported: identities must stay encrypted on disk.
    pub fn remove_password(&mut self, _password: &str) -> Result<()> {
        Err(anyhow!(
            "Removing password protection is not supported; identities must remain encrypted on disk"
        ))
    }

    pub fn private_key(&self) -> Result<RsaPrivateKey> {
        let pem = self.private_key_pem_plaintext.as_ref().ok_or_else(|| {
            anyhow::anyhow!("Private key not available. Was the identity decrypted?")
        })?;
        Ok(RsaPrivateKey::from_pkcs8_pem(pem)?)
    }

    pub fn public_key(&self) -> Result<RsaPublicKey> {
        Ok(RsaPublicKey::from_public_key_pem(&self.public_key_pem)?)
    }

    /// Encrypt a legacy identity's in-memory plaintext key with `password`.
    /// No-op if it is already encrypted.
    pub fn migrate_legacy_plaintext(&mut self, password: &str) -> Result<()> {
        if self.encrypted_private_key.is_some() {
            return Ok(());
        }
        if self.private_key_pem_plaintext.is_none() {
            return Err(anyhow!("No plaintext private key available to migrate"));
        }
        self.encrypt(password)
    }

    /// Generate a signed (RSA-PSS) invite link, verified on import.
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

    /// Generate a signed invite with direct-connect candidates in priority
    /// order plus an optional relay route.
    ///
    /// `address` carries the first candidate for old clients; `addresses` is
    /// only emitted (and signed) when it holds more than one, so single-address
    /// invites stay byte-identical to the older format and keep verifying.
    pub fn generate_signed_invite_link_with_addresses(
        &self,
        addresses: Vec<String>,
        relay_server: Option<String>,
        relay_token: Option<String>,
    ) -> Result<String> {
        use std::time::{SystemTime, UNIX_EPOCH};

        let mut invite_nonce = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut invite_nonce);

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let primary = addresses.first().cloned();
        let multi = if addresses.len() > 1 {
            addresses
        } else {
            Vec::new()
        };

        let payload = invite::SignedInvitePayload {
            version: if !multi.is_empty() {
                4
            } else if relay_server.is_some() || relay_token.is_some() {
                3
            } else {
                2
            },
            timestamp,
            nonce: hex::encode(invite_nonce),
            name: self.name.clone(),
            address: primary,
            relay_server,
            relay_token,
            fingerprint: self.fingerprint.clone(),
            public_key: self.public_key_pem.clone(),
            addresses: multi,
        };
        invite::encode_signed(payload, &self.private_key()?)
    }

    /// Generate an unsigned v1 invite link. Deprecated: prefer
    /// [`Identity::generate_signed_invite_link`].
    pub fn generate_invite_link(&self, address: Option<String>) -> Result<String> {
        invite::encode_unsigned(&invite::InvitePayload {
            name: self.name.clone(),
            address,
            fingerprint: self.fingerprint.clone(),
            public_key: self.public_key_pem.clone(),
        })
    }

    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let identity: Identity = serde_json::from_str(&content)?;
        tracing::info!("Loaded identity: {} ({})", identity.name, identity.id);

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

    /// Validate and set the display name; persist with [`Identity::save`].
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

    /// Persist the identity atomically and 0600 ([`crate::util::write_file_atomic`]).
    /// Refuses an unencrypted identity.
    pub fn save(&self, path: &Path) -> Result<()> {
        if self.encrypted_private_key.is_none() {
            return Err(anyhow!(
                "SECURITY ERROR: Cannot save identity without encryption!"
            ));
        }

        let content = serde_json::to_string_pretty(&self)?;
        crate::util::write_file_atomic(path, content.as_bytes())?;

        tracing::info!("Saved identity: {} to {}", self.name, path.display());
        Ok(())
    }

    /// Load the identity from `data_dir`, or create one if there is none.
    /// Returns `(identity, is_new)`.
    ///
    /// **A present-but-unreadable file is a hard error**, never a reason to
    /// mint a new identity: that would make the history (keyed by
    /// [`Identity::history_key`]) unreadable and break TOFU with every contact,
    /// while looking like a fresh install.
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

    /// Encrypted and not currently decrypted.
    pub fn is_locked(&self) -> bool {
        self.encrypted_private_key.is_some() && self.private_key_pem_plaintext.is_none()
    }

    /// The private key has a password (persisted form).
    pub fn is_encrypted(&self) -> bool {
        self.encrypted_private_key.is_some()
    }

    /// The 32-byte history encryption key: SHA-256 over the private key's
    /// PKCS#8 DER. Requires the identity to be unlocked.
    ///
    /// **Frozen**: it depends on the `rsa` crate's encoding, so any change would
    /// silently make every stored history undecryptable. Pinned by
    /// `history_key_derivation_is_frozen`; if that ever fails, the fix is a
    /// history migration, never a change here.
    pub fn history_key(&self) -> Result<[u8; 32]> {
        let privkey = self.private_key()?;
        let der = privkey.to_pkcs8_der()?;
        Ok(Sha256::digest(der.as_bytes()).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use tempfile::tempdir;

    fn test_password() -> String {
        format!("pw-{}", Uuid::new_v4())
    }

    fn fresh(name: &str) -> Identity {
        Identity::new_with_plaintext(name.to_string()).unwrap()
    }

    /// A fresh identity encrypted and then unlocked under the returned password.
    fn unlocked(name: &str) -> (Identity, String) {
        let mut identity = fresh(name);
        let password = test_password();
        identity.encrypt(&password).unwrap();
        identity.decrypt(&password).unwrap();
        (identity, password)
    }

    /// Whether a locked copy of `identity` opens with `password`.
    fn opens_with(identity: &Identity, password: &str) -> bool {
        let mut copy = identity.clone();
        copy.private_key_pem_plaintext = None;
        copy.decrypt(password).is_ok()
    }

    /// The decoded JSON of a v2 signed invite link (`payload` + `signature`).
    fn signed_invite_json(link: &str) -> serde_json::Value {
        let encoded = link.strip_prefix("chat-p2p://invite/v2/").unwrap();
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .unwrap();
        serde_json::from_slice(&decoded).unwrap()
    }

    /// The KAT key as raw components (a published test vector, so no PEM
    /// shaped like a real private key lives in the repo). 1024-bit: the test
    /// needs a stable encoding, not security.
    const EXPECTED_HISTORY_KEY: &str =
        "7ed689da4592916beda567f25fa36d746b2e2cccfa313ba8e08d0c2dbbdaa9f3";

    const KAT_N: &str = "bad7050643ee4c36bfa78df11a6c606b3bdf97f0889cd8e547a4a4c6b1c4d4a6a88b250ca40e836aaa0fdf03c4057af6192ccad1bb07d0b88ec1d0a74a457d0700160c36ff210ad27b2f6646e8eca14b5ec45c949d8cb78ee7bd69a3b36c56d738f0635247facf33cc2841666381a2a442607a3201c1b70ac8fbf6f436720ce7";
    const KAT_E: &str = "10001";
    const KAT_D: &str = "6dc05aaa3883256fcf9afc0d11c971c5ebf0c6cebb60ef2397b70637d53adaf35ef4057a6c703e100cffafb0059876875378755747b72a8b0f0898a97c3e5f5717bec35db9609ade4e98238c39ddac14895384c9635850e48d6284e185059a542e6c915aa6cd24bcddf87f7b7269520ab2dc5645a8c79b29eb873da9714e7761";
    const KAT_P: &str = "f529b9b1287b23fab001ce11821ae25e5c9e996c5362802a777541a2e6c4675c50ca7255352f604e9e7a8cdcc1a7f6cf4685418d867e3768acbff9c697bc1a11";
    const KAT_Q: &str = "c3194ed799ae0bd2cd9d9aa0711b7adb3b07b5f8a7e3df5b08a9c2e28cc1b025efbb10eede467df86cd2c15a2fa5a0574315350d35be821a7eb2a7954c5bff77";

    fn kat_identity() -> Identity {
        use rsa::traits::PublicKeyParts;
        use rsa::BoxedUint;

        let big = |h: &str| {
            let bytes = hex::decode(if h.len() % 2 == 1 {
                format!("0{h}")
            } else {
                h.to_string()
            })
            .unwrap();
            BoxedUint::from_be_slice_vartime(&bytes)
        };
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
        let fingerprint = Identity::calculate_fingerprint(&public_key_pem).unwrap();
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

    /// Every RSA artefact peers and files depend on, pinned to what `rsa` 0.9
    /// produced for the same key. `history_key_derivation_is_frozen` covers the
    /// PKCS#8 encoding; this covers the rest of what an `rsa` upgrade can move:
    ///
    /// - the public PEM and the fingerprint derived from it (every pin,
    ///   verified contact and invite is keyed by it);
    /// - the PKCS#1 private encoding `pem_encode_private` writes;
    /// - the Ed25519 subkey, which is derived from the PKCS#8 DER — a shift
    ///   there silently changes the key every v4 peer has bound to us;
    /// - RSA-PSS signatures made by 0.9 (identity proofs, invites, Ed25519
    ///   bindings) still verifying, and new ones verifying with the same
    ///   salt length 0.9 required.
    ///
    /// As with the history key, a failure here is not an expectation to update:
    /// it means the upgrade breaks existing users or peers.
    #[test]
    fn rsa_artefacts_match_what_rsa_0_9_produced() {
        use crate::core::crypto::{
            cross_sign_ed25519, derive_ed25519_subkey, pem_decode_private, pem_encode_private,
            rsa_sign_pss, rsa_verify_pss, verify_ed25519_binding,
        };
        use rsa::pkcs1::EncodeRsaPrivateKey;
        use rsa::signature::Verifier;

        const PUBLIC_PEM_SHA256: &str =
            "3dd4b1c4ca3f12e462b6bb3e611658fe79356a47c57d09e12fc44ad2f2f56c09";
        const FINGERPRINT: &str =
            "451daceba2d7f52a6d668e87363395bfa11e4e50f6ceeaaa051865181240bcac";
        const PKCS1_DER_SHA256: &str =
            "3ec5cfe7475929a9741ebaf3225a9a832f2ac59ed23d07fd60acb2fe8e55ffb0";
        const PKCS1_PEM_SHA256: &str =
            "ae65d6a05a8ee56b168deeed94a7a0ea7cc8eb0b440a10794e2231960c004e92";
        const ED25519_SUBKEY_PUBLIC: &str =
            "077df54cb0989b9ad239cec5a9425b3ab7d91768cb1c390665731d8a008b227d";
        const MESSAGE: &[u8] = b"p2pem rsa 0.9 -> 0.10 compatibility vector";
        const PSS_SIGNATURE_BY_0_9: &str = "3dbdec5cc8d1125a6ad7258bd4fe31f85cc79eb482d2b5403ac818bd96524d7591de3c4e6ac501e753597ed2f6607585d85e574b07584df5cefa71410fc8c8ab3bc842cf3a53ca64c6d426aa1f1333f94e37d9cfb7e961da4758a058ffce8ae908e4f10f259b60c650c39a39383e6c889e4e1e675c7585dc47760ba6fc4c6683";
        const BINDING_SIGNATURE_BY_0_9: &str = "9d1838484c7d52a8242c097b4203f6bff24d7e995cb8da44c75100a5c334374d98c9ec48b980bd24ae22af1093e93207ada90d2861a09f1904df2b75a9a5a047f3782f3e375255817ed2b1b3203f1f5dea77195260e6a045f694bc9aa11f9691d1768d96432f2da9e747f76e8f0711c03c1c5a7a1086ca4366e881ac5971aa61";

        let identity = kat_identity();
        let key = identity.private_key().unwrap();
        let public = identity.public_key().unwrap();

        assert_eq!(
            hex::encode(Sha256::digest(identity.public_key_pem.as_bytes())),
            PUBLIC_PEM_SHA256,
            "the public PEM encoding changed"
        );
        assert_eq!(identity.fingerprint, FINGERPRINT, "the fingerprint changed");
        assert_eq!(
            hex::encode(Sha256::digest(key.to_pkcs1_der().unwrap().as_bytes())),
            PKCS1_DER_SHA256,
            "the PKCS#1 private encoding changed"
        );
        let pkcs1_pem = pem_encode_private(&key).unwrap();
        // `pem_encode_private` uses the platform's line ending (CRLF on
        // Windows), so the text is compared with LF line endings.
        assert_eq!(
            hex::encode(Sha256::digest(pkcs1_pem.replace("\r\n", "\n").as_bytes())),
            PKCS1_PEM_SHA256,
            "pem_encode_private's output changed"
        );
        assert_eq!(
            pem_decode_private(&pkcs1_pem)
                .unwrap()
                .to_pkcs8_der()
                .unwrap()
                .as_bytes(),
            key.to_pkcs8_der().unwrap().as_bytes(),
            "a PKCS#1 round trip must give back the same key"
        );

        let subkey = derive_ed25519_subkey(&key).unwrap();
        assert_eq!(
            hex::encode(subkey.verifying_key().to_bytes()),
            ED25519_SUBKEY_PUBLIC,
            "the derived Ed25519 subkey changed"
        );

        rsa_verify_pss(
            &public,
            MESSAGE,
            &hex::decode(PSS_SIGNATURE_BY_0_9).unwrap(),
        )
        .expect("a signature made by rsa 0.9 must still verify");
        verify_ed25519_binding(
            &public,
            &subkey.verifying_key(),
            &hex::decode(BINDING_SIGNATURE_BY_0_9).unwrap(),
        )
        .expect("an Ed25519 binding made by rsa 0.9 must still verify");

        // And the other direction: what we sign now carries the 32-byte salt
        // rsa 0.9's verifier requires (a fixed-salt verifier rejects any other
        // length), so old peers accept it.
        let fresh = rsa_sign_pss(&key, MESSAGE).unwrap();
        rsa::pss::VerifyingKey::<rsa::sha2::Sha256>::new_with_salt_len(public.clone(), 32)
            .verify(
                MESSAGE,
                &rsa::pss::Signature::try_from(fresh.as_slice()).unwrap(),
            )
            .expect("new signatures must use the 32-byte salt");
        let binding = cross_sign_ed25519(&key, &subkey.verifying_key()).unwrap();
        verify_ed25519_binding(&public, &subkey.verifying_key(), &binding).unwrap();

        let mut tampered = hex::decode(PSS_SIGNATURE_BY_0_9).unwrap();
        tampered[10] ^= 1;
        assert!(rsa_verify_pss(&public, MESSAGE, &tampered).is_err());
    }

    /// A failure here is not a test to update: shipping the change would make
    /// every existing history undecryptable, so it needs a migration.
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

    #[test]
    fn history_key_is_stable_across_calls() {
        let identity = kat_identity();
        assert_eq!(
            identity.history_key().unwrap(),
            identity.history_key().unwrap()
        );
    }

    #[test]
    fn history_key_differs_between_identities() {
        let (a, b) = (fresh("A"), fresh("B"));
        assert_ne!(a.history_key().unwrap(), b.history_key().unwrap());
    }

    #[test]
    fn test_identity_creation() {
        let identity = fresh("Test User");

        assert_eq!(identity.name, "Test User");
        assert_eq!(identity.fingerprint.len(), 64); // SHA-256 in hex
        assert!(identity.private_key_pem_plaintext.is_some());
        assert!(identity
            .public_key_pem
            .starts_with("-----BEGIN PUBLIC KEY-----"));
    }

    #[test]
    fn unreadable_identity_is_an_error_never_a_new_identity() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("identity.json");

        // A truncated file, as an interrupted write used to leave.
        std::fs::write(&path, b"").unwrap();
        // `Identity` has no `Debug` (key material), so no `expect_err`.
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

        // The unreadable file is left exactly as it was.
        assert!(path.exists());
        assert_eq!(std::fs::read(&path).unwrap(), b"");

        std::fs::write(&path, b"{\"partial\": tru").unwrap();
        assert!(Identity::get_or_create(dir.path(), "User").is_err());
    }

    #[test]
    fn absent_identity_creates_a_new_one() {
        let dir = tempdir().unwrap();
        let (identity, is_new) = Identity::get_or_create(dir.path(), "Fresh").unwrap();
        assert!(is_new);
        assert_eq!(identity.name, "Fresh");
    }

    #[test]
    fn existing_identity_is_loaded_not_recreated() {
        let dir = tempdir().unwrap();
        let mut identity = fresh("Original");
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

    #[test]
    fn save_replaces_atomically_and_leaves_no_temp_files() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("identity.json");

        for name in ["First", "Second"] {
            let mut identity = fresh(name);
            identity.encrypt(&test_password()).unwrap();
            identity.save(&path).unwrap();
        }
        assert_eq!(Identity::load(&path).unwrap().name, "Second");

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

    #[test]
    fn encrypt_rejects_passwords_below_the_floor() {
        let mut identity = fresh("Test User");
        let short = "a".repeat(crate::MIN_PASSWORD_LEN - 1);
        let err = identity
            .encrypt(&short)
            .expect_err("a short password must be rejected");
        assert!(
            err.to_string().contains("at least"),
            "error should say what the floor is, got: {err}"
        );
        // Nothing was consumed: an acceptable password still works.
        assert!(identity.private_key_pem_plaintext.is_some());
        identity
            .encrypt(&"a".repeat(crate::MIN_PASSWORD_LEN))
            .unwrap();
        assert!(identity.encrypted_private_key.is_some());
    }

    /// Raising the floor must never lock out an older identity.
    #[test]
    fn decrypt_applies_no_length_policy() {
        let mut identity = fresh("Legacy User");
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
        let mut identity = fresh("Test User");
        let original_pem = identity.private_key_pem_plaintext.clone().unwrap();
        let password = test_password();

        identity.encrypt(&password).unwrap();
        assert!(identity.private_key_pem_plaintext.is_none());
        assert!(identity.encrypted_private_key.is_some());
        assert!(identity.salt.is_some());
        assert!(identity.nonce.is_some());

        identity.decrypt(&password).unwrap();
        assert_eq!(identity.private_key_pem_plaintext.unwrap(), original_pem);
    }

    #[test]
    fn change_password_rewraps_the_same_key_and_keeps_the_history_key() {
        let (mut identity, old) = unlocked("Test User");
        let new = test_password();
        let history_key_before = identity.history_key().unwrap();
        let pem_before = identity.private_key_pem_plaintext.clone().unwrap();

        identity.change_password(&old, &new).unwrap();

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
        assert!(opens_with(&identity, &new));
        assert!(!opens_with(&identity, &old));
    }

    #[test]
    fn change_password_rejects_a_wrong_current_password() {
        let (mut identity, old) = unlocked("Test User");
        let err = identity
            .change_password("not-the-password", &test_password())
            .expect_err("a wrong current password must be refused");
        assert!(err.to_string().contains("Current password is incorrect"));
        assert!(opens_with(&identity, &old));
    }

    #[test]
    fn change_password_enforces_the_length_floor_without_destroying_the_old() {
        let (mut identity, old) = unlocked("Test User");
        let err = identity
            .change_password(&old, "short")
            .expect_err("a too-short new password must be refused");
        assert!(err.to_string().contains("at least"));
        assert!(
            opens_with(&identity, &old),
            "a refused change must leave the identity openable with the old password"
        );
    }

    /// The display name both UIs set: trimmed, never empty, at most 48
    /// *characters* (a name in a non-Latin script must not hit the limit at a
    /// third of its length), and a refused name leaves the old one in place.
    #[test]
    fn set_name_trims_and_enforces_its_limits() {
        let mut identity = fresh("Old");
        identity.set_name("  Maya  ").unwrap();
        assert_eq!(identity.name, "Maya");

        for bad in ["", "   ", &"x".repeat(49)] {
            assert!(identity.set_name(bad).is_err(), "{bad:?} accepted");
            assert_eq!(identity.name, "Maya", "a refused name replaced the old one");
        }
        let longest = "é".repeat(48); // 96 bytes, 48 characters
        identity.set_name(&longest).unwrap();
        assert_eq!(identity.name, longest);
    }

    #[test]
    fn test_decryption_with_wrong_password_fails() {
        let mut identity = fresh("Test User");
        identity.encrypt(&test_password()).unwrap();
        // `test_password()` is random, so this is a different password. It must
        // fail as *wrong password* — the UIs ask again for that one and report
        // anything else as a damaged file (TAURI-20) — and say so if printed.
        let err = identity.decrypt(&test_password()).unwrap_err();
        assert!(err.is::<WrongPassword>(), "{err:#}");
        assert_eq!(err.to_string(), "Decryption failed (likely wrong password)");
    }

    /// CRYPTO-08: the nonce and salt come from `identity.json`, which can be
    /// truncated or edited. A wrong-sized nonce used to reach
    /// `GenericArray::from_slice`, which *asserts* the length — so a damaged
    /// file panicked the unlock instead of reaching the "error" auth state,
    /// and inside a Tauri command could poison the bridge mutex.
    #[test]
    fn a_malformed_nonce_or_salt_is_an_error_not_a_panic() {
        let password = test_password();
        let mut good = Identity::new_with_plaintext("Test User".to_string()).unwrap();
        good.encrypt(&password).unwrap();
        let nonce = good.nonce.clone().unwrap();
        let salt = good.salt.clone().unwrap();

        let damaged = [
            ("empty nonce", Vec::new(), salt.clone()),
            ("short nonce", nonce[..11].to_vec(), salt.clone()),
            (
                "long nonce",
                [nonce.clone(), vec![0]].concat(),
                salt.clone(),
            ),
            ("empty salt", nonce.clone(), Vec::new()),
            ("short salt", nonce.clone(), salt[..4].to_vec()),
        ];
        for (what, nonce, salt) in damaged {
            let mut identity = good.clone();
            identity.private_key_pem_plaintext = None;
            identity.nonce = Some(nonce);
            identity.salt = Some(salt);
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                identity.decrypt(&password)
            }));
            match outcome {
                Ok(result) => assert!(result.is_err(), "{what}: unlocked anyway"),
                Err(_) => panic!("{what}: decrypt panicked instead of returning an error"),
            }
        }
    }

    #[test]
    fn test_save_load_encrypted() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("identity.json");

        let mut identity = fresh("Test User");
        let original_pem = identity.private_key().unwrap();
        let password = test_password();
        identity.encrypt(&password).unwrap();
        identity.save(&path).unwrap();

        let mut loaded = Identity::load(&path).unwrap();
        assert!(loaded.private_key_pem_plaintext.is_none()); // never persisted
        loaded.decrypt(&password).unwrap();
        assert_eq!(loaded.private_key().unwrap(), original_pem);
    }

    /// The derived serde keeps the legacy file shapes: a plaintext key is read
    /// but never written, and absent optional fields load as `None`.
    #[test]
    fn legacy_identity_json_loads_and_never_writes_the_plaintext_key() {
        let identity = fresh("Legacy");
        let pem = identity.private_key_pem_plaintext.clone().unwrap();
        let mut json = serde_json::to_value(&identity).unwrap();
        assert!(json.get("private_key_pem_plaintext").is_none());
        assert!(json.get("encrypted_private_key").is_none());
        json["private_key_pem_plaintext"] = serde_json::Value::String(pem.to_string());

        let loaded: Identity = serde_json::from_value(json).unwrap();
        assert_eq!(loaded.private_key_pem_plaintext.as_deref(), Some(&*pem));
        assert!(loaded.encrypted_private_key.is_none() && loaded.argon_params.is_none());
        let rewritten = serde_json::to_value(&loaded).unwrap();
        assert!(rewritten.get("private_key_pem_plaintext").is_none());
    }

    #[test]
    fn test_save_unencrypted_fails() {
        let dir = tempdir().unwrap();
        let identity = fresh("Unsafe User");
        assert!(identity
            .save(&dir.path().join("identity_unsafe.json"))
            .is_err());
    }

    #[test]
    fn test_invite_link_generation() {
        let link = fresh("Test User").generate_invite_link(None).unwrap();
        assert!(link.starts_with("chat-p2p://invite/"));
        assert!(link.len() > 50);
    }

    #[test]
    fn test_invite_link_includes_address_when_provided() {
        let addr = Some("10.0.0.5:5000".to_string());
        let link = fresh("Tester").generate_invite_link(addr.clone()).unwrap();
        assert!(link.contains("chat-p2p://invite/"));

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

    #[test]
    fn test_signed_invite_link_generation() {
        let link = fresh("Signed Test User")
            .generate_signed_invite_link(None)
            .unwrap();
        assert!(link.starts_with("chat-p2p://invite/v2/"));
        assert!(link.len() > 50);

        // URL-safe base64: no `+` or `/`.
        let encoded = link.strip_prefix("chat-p2p://invite/v2/").unwrap();
        assert!(!encoded.contains('+'));
        assert!(!encoded.contains('/'));
    }

    #[test]
    fn test_signed_invite_link_includes_timestamp() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let link = fresh("Timestamp Test")
            .generate_signed_invite_link(None)
            .unwrap();
        let invite = signed_invite_json(&link);

        let timestamp = invite["payload"]["timestamp"].as_u64().unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(now - timestamp < 10);
    }

    #[test]
    fn test_signed_invite_link_includes_nonce() {
        let identity = fresh("Nonce Test");
        let nonce = || {
            let link = identity.generate_signed_invite_link(None).unwrap();
            signed_invite_json(&link)["payload"]["nonce"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let (nonce1, nonce2) = (nonce(), nonce());

        assert_ne!(nonce1, nonce2);
        assert!(!nonce1.is_empty());
        assert!(!nonce2.is_empty());
        hex::decode(&nonce1).unwrap();
        hex::decode(&nonce2).unwrap();
    }

    #[test]
    fn test_signed_invite_link_with_address() {
        let addr = Some("192.168.1.100:9000".to_string());
        let link = fresh("Address Test")
            .generate_signed_invite_link(addr.clone())
            .unwrap();
        let invite = signed_invite_json(&link);
        assert_eq!(invite["payload"]["address"].as_str(), addr.as_deref());
    }

    #[test]
    fn test_signed_invite_payload_contains_identity_info() {
        let identity = fresh("Info Test");
        let link = identity.generate_signed_invite_link(None).unwrap();
        let invite = signed_invite_json(&link);
        let payload = &invite["payload"];

        assert_eq!(payload["version"].as_u64(), Some(2));
        assert!(payload["timestamp"].as_u64().is_some());
        assert!(payload["nonce"].as_str().is_some());
        assert_eq!(payload["name"].as_str(), Some("Info Test"));
        assert_eq!(
            payload["fingerprint"].as_str(),
            Some(identity.fingerprint.as_str())
        );
        assert_eq!(
            payload["public_key"].as_str(),
            Some(identity.public_key_pem.as_str())
        );
        assert!(!invite["signature"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_signed_relay_invite_link_includes_relay_fields() {
        let link = fresh("Relay User")
            .generate_signed_invite_link_with_route(
                None,
                Some("relay.example.com:23456".to_string()),
                Some("0123456789abcdef0123456789abcdef".to_string()),
            )
            .unwrap();

        assert!(link.starts_with("chat-p2p://invite/v2/"));
        let json = signed_invite_json(&link).to_string();
        assert!(json.contains("relay.example.com:23456"));
        assert!(json.contains("0123456789abcdef0123456789abcdef"));
        assert!(json.contains("\"version\":3"));
    }

    #[test]
    fn test_multi_address_invite_carries_ordered_list_and_primary() {
        let link = fresh("Multi Addr")
            .generate_signed_invite_link_with_addresses(
                vec![
                    "203.0.113.7:12345".to_string(),
                    "192.168.1.20:12345".to_string(),
                ],
                None,
                None,
            )
            .unwrap();
        let invite = signed_invite_json(&link);
        let payload = &invite["payload"];

        // Primary `address` is the first candidate; the full list bumps to v4.
        assert_eq!(payload["address"].as_str(), Some("203.0.113.7:12345"));
        let addrs: Vec<&str> = payload["addresses"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(addrs, vec!["203.0.113.7:12345", "192.168.1.20:12345"]);
        assert_eq!(payload["version"].as_u64(), Some(4));
    }

    /// Byte-format guard: a single-address invite emits no `addresses` key, so
    /// it signs identically to invites minted before the field existed.
    #[test]
    fn test_single_address_invite_omits_addresses_key() {
        let identity = fresh("Single Addr");
        for link in [
            identity
                .generate_signed_invite_link(Some("192.168.1.20:12345".to_string()))
                .unwrap(),
            identity.generate_signed_invite_link(None).unwrap(),
        ] {
            let invite = signed_invite_json(&link);
            assert!(
                invite["payload"].get("addresses").is_none(),
                "single-address invite must omit the addresses key"
            );
            assert_ne!(invite["payload"]["version"].as_u64(), Some(4));
        }
    }

    /// Argon2 runs on this thread, counted.
    pub(super) mod derivations {
        use std::cell::Cell;
        thread_local!(static COUNT: Cell<u32> = const { Cell::new(0) });
        pub(in crate::identity) fn bump() {
            COUNT.with(|c| c.set(c.get() + 1));
        }
        pub(in crate::identity) fn reset() {
            COUNT.with(|c| c.set(0));
        }
        pub(in crate::identity) fn count() -> u32 {
            COUNT.with(Cell::get)
        }
    }

    // Argon2 compatibility. Three on-disk shapes exist: `Argon2::default()`,
    // strict parameters unrecorded, and (current) strict parameters recorded in
    // `argon_params`, which are then the only ones tried.

    /// Reproduce a file from before `argon_params`: the key wrapped with
    /// `argon`, parameters unrecorded.
    fn wrap_private_key_with(identity: &mut Identity, password: &str, argon: &Argon2<'_>) {
        let pem = identity
            .private_key_pem_plaintext
            .take()
            .expect("fixture needs a plaintext key");
        let (ciphertext, salt, nonce) = Identity::wrap_private_key(argon, password, &pem).unwrap();
        identity.encrypted_private_key = Some(ciphertext);
        identity.salt = Some(salt);
        identity.nonce = Some(nonce);
        identity.argon_params = None;
    }

    /// The wrapping key for the strict parameters, pinned against the
    /// reference C implementation (argon2-cffi 25.1, `hash_secret_raw`, type
    /// ID, v19). What it guards: every identity file opens only if this
    /// derivation never changes — a crate upgrade, or turning on the
    /// `parallel` feature (PERF-12), must reproduce these bytes exactly.
    #[test]
    fn the_wrapping_key_matches_the_reference_implementation() {
        let mut key = [0u8; KEY_SIZE];
        strict_argon()
            .unwrap()
            .hash_password_into(
                b"correct horse battery staple",
                b"p2pem-kat-salt!!",
                &mut key,
            )
            .unwrap();
        assert_eq!(
            hex::encode(key),
            "e6e1221008715771c9ac2c8b0317d37f4301e772a202accc6224e51946701a8e"
        );
    }

    /// PERF-12: first-time setup ran Argon2 twice — `encrypt`, then `decrypt`
    /// only to get back the key `encrypt` had just thrown away. `protect`
    /// wraps once and keeps the key, and what reaches the disk is the same
    /// encrypted file `encrypt` writes.
    #[test]
    fn protecting_an_identity_derives_once_and_stays_unlocked() {
        let mut identity = Identity::new_with_plaintext("Setup".to_string()).unwrap();
        let history_key = identity.history_key().unwrap();
        derivations::reset();
        identity.protect("a-long-enough-password").unwrap();
        assert_eq!(derivations::count(), 1, "Argon2 ran more than once");
        assert!(identity.is_encrypted());
        assert!(!identity.is_locked(), "setup left the identity locked");
        assert_eq!(identity.history_key().unwrap(), history_key);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.json");
        identity.save(&path).unwrap();
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(
            !on_disk.contains("PRIVATE KEY"),
            "the plaintext key was written"
        );
        let mut reopened = Identity::load(&path).unwrap();
        assert!(reopened.is_locked());
        reopened.decrypt("a-long-enough-password").unwrap();
        assert_eq!(reopened.history_key().unwrap(), history_key);

        // Same floor as `encrypt`, and a refusal changes nothing.
        let mut fresh = Identity::new_with_plaintext("Short".to_string()).unwrap();
        assert!(fresh.protect("short").is_err());
        assert!(!fresh.is_encrypted() && !fresh.is_locked());
    }

    #[test]
    fn stored_argon_params_are_the_only_ones_tried() {
        let mut identity = fresh("Mismatched User");
        let password = test_password();

        // Oldest scheme, in a file that claims the current parameters.
        wrap_private_key_with(&mut identity, &password, &Argon2::default());
        identity.argon_params = Some(ArgonParams {
            m_cost_kib: 65536,
            t_cost: 3,
            p_cost: 4,
            output_len: KEY_SIZE as u32,
        });

        assert!(
            identity.decrypt(&password).is_err(),
            "decrypt fell back to parameters other than the recorded ones"
        );
    }

    #[test]
    fn absurd_stored_argon_params_are_refused() {
        let mut locked = fresh("Hostile File");
        let password = test_password();
        locked.encrypt(&password).unwrap();

        // Each cost on its own, one past its cap: an edited `identity.json`
        // needs only one of them to turn unlock into a huge allocation (memory)
        // or a hang (time, lanes). 2 GiB is legal per the RFC, for instance.
        type Edit = fn(&mut ArgonParams);
        let edits: [(&str, Edit); 3] = [
            ("memory", |p| p.m_cost_kib = MAX_ARGON_M_COST_KIB + 1),
            ("time", |p| p.t_cost = MAX_ARGON_T_COST + 1),
            ("lanes", |p| p.p_cost = MAX_ARGON_P_COST + 1),
        ];
        for (which, edit) in edits {
            let mut identity = locked.clone();
            edit(identity.argon_params.as_mut().unwrap());
            let err = identity
                .decrypt(&password)
                .expect_err("out-of-range parameters must be refused");
            assert!(
                err.to_string().contains("out of range"),
                "{which}: expected a parameter-range error, got: {err}"
            );
        }

        // At the cap is allowed: it gets past the range check, and then fails
        // only because these are not the costs the key was wrapped with. (The
        // lane count is the one boundary cheap enough to run; the memory and
        // time caps at their limit would cost a 1 GiB / 16-pass derivation.)
        let mut identity = locked.clone();
        identity.argon_params.as_mut().unwrap().p_cost = MAX_ARGON_P_COST;
        let err = identity.decrypt(&password).unwrap_err();
        assert!(
            err.is::<WrongPassword>(),
            "the cap itself was refused: {err}"
        );
    }

    fn assert_legacy_identity_opens(argon: &Argon2<'_>) {
        let mut identity = fresh("Legacy User");
        let original_pem = identity.private_key_pem_plaintext.clone().unwrap();
        let password = test_password();
        wrap_private_key_with(&mut identity, &password, argon);
        assert!(identity.argon_params.is_none());

        identity
            .decrypt(&password)
            .expect("legacy identity must open");
        assert_eq!(identity.private_key_pem_plaintext.unwrap(), original_pem);
    }

    #[test]
    fn identity_predating_stored_params_still_opens_default_scheme() {
        assert_legacy_identity_opens(&Argon2::default());
    }

    #[test]
    fn identity_predating_stored_params_still_opens_strict_scheme() {
        assert_legacy_identity_opens(&strict_argon().unwrap());
    }

    #[test]
    fn wrong_password_on_a_legacy_identity_still_fails() {
        let mut identity = fresh("Oldest User");
        wrap_private_key_with(&mut identity, &test_password(), &Argon2::default());
        assert!(identity.decrypt(&test_password()).is_err());
    }

    #[test]
    fn test_remove_password_is_rejected() {
        let mut identity = fresh("Protected User");
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
