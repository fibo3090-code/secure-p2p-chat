use anyhow::{anyhow, Result};
use chacha20poly1305::{
    aead::{Aead, AeadCore},
    ChaCha20Poly1305, KeyInit,
};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::types::{Chat, Config};
use uuid::Uuid;

/// Returns true if the path is considered dangerous (traversal or system dir).
/// Used to prevent malicious history files from redirecting writes to system paths.
fn is_dangerous_path(p: &Path) -> bool {
    let s = p.to_string_lossy();
    if s.contains("..") {
        return true;
    }
    #[cfg(windows)]
    {
        let lower: std::string::String = s.chars().flat_map(|c| c.to_lowercase()).collect();
        if lower.contains("\\windows\\")
            || lower.contains("\\program files")
            || lower.contains("\\system32")
            || lower.contains("\\program files (x86)")
        {
            return true;
        }
    }
    #[cfg(unix)]
    {
        if s.starts_with("/etc")
            || s.starts_with("/usr")
            || s.starts_with("/bin")
            || s.starts_with("/sbin")
        {
            return true;
        }
    }
    false
}

/// Sanitize loaded config: replace dangerous paths with defaults.
fn sanitize_loaded_config(mut config: Config) -> Config {
    if is_dangerous_path(&config.download_dir) {
        config.download_dir = PathBuf::from("Downloads");
    }
    if is_dangerous_path(&config.temp_dir) {
        config.temp_dir = PathBuf::from("temp");
    }
    config
}

/// History file format for JSON serialization
#[derive(Serialize, Deserialize)]
pub struct HistoryFile {
    pub version: String,
    pub chats: Vec<Chat>,
    pub contacts: Vec<crate::types::Contact>,
    #[serde(default)]
    pub config: Config,
    /// Contact -> chat association map so we can reconnect automatically
    #[serde(default)]
    pub contact_chat_map: Vec<(Uuid, Uuid)>,
}

impl HistoryFile {
    pub fn new(chats: Vec<Chat>) -> Self {
        Self {
            version: "1.0".to_string(),
            chats,
            contacts: Vec::new(),
            config: Config::default(),
            contact_chat_map: Vec::new(),
        }
    }

    /// Load history from JSON file (plaintext - legacy support)
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let history: HistoryFile = serde_json::from_str(&content)?;

        if history.version != "1.0" {
            anyhow::bail!("Unsupported history version: {}", history.version);
        }

        tracing::info!("Loaded {} chats from history", history.chats.len());
        Ok(history)
    }

    /// Load encrypted history from file
    pub fn load_encrypted(path: &Path, key: &[u8; 32]) -> Result<Self> {
        let encrypted_data = std::fs::read(path)?;

        if encrypted_data.len() < 12 {
            return Err(anyhow!("Invalid encrypted history file: too short"));
        }

        // Extract nonce (first 12 bytes) and ciphertext
        let (nonce_bytes, ciphertext) = encrypted_data.split_at(12);
        let nonce = chacha20poly1305::Nonce::from(*<&[u8; 12]>::try_from(nonce_bytes)?);

        let cipher = ChaCha20Poly1305::new(key.into());
        let plaintext = cipher
            .decrypt(&nonce, ciphertext)
            .map_err(|e| anyhow!("Decryption failed (wrong password?): {}", e))?;

        let history: HistoryFile = serde_json::from_slice(&plaintext)?;

        if history.version != "1.0" {
            anyhow::bail!("Unsupported history version: {}", history.version);
        }

        tracing::info!(
            "Loaded {} chats from encrypted history",
            history.chats.len()
        );
        Ok(history)
    }

    /// Save history to JSON file (plaintext - NOT RECOMMENDED)
    pub fn save(&self, path: &Path) -> Result<()> {
        // Create parent directory if it doesn't exist
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = serde_json::to_string_pretty(&self)?;
        std::fs::write(path, content)?;

        tracing::warn!(
            "Saved {} chats to UNENCRYPTED history - use save_encrypted instead!",
            self.chats.len()
        );
        Ok(())
    }

    /// Save encrypted history to file (RECOMMENDED)
    pub fn save_encrypted(&self, path: &Path, key: &[u8; 32]) -> Result<()> {
        // Create parent directory if it doesn't exist
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let json = serde_json::to_string_pretty(&self)?;

        let cipher = ChaCha20Poly1305::new(key.into());
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ciphertext = cipher
            .encrypt(&nonce, json.as_bytes())
            .map_err(|e| anyhow!("Encryption failed: {}", e))?;

        // Format: nonce (12 bytes) || ciphertext
        let mut output = nonce.to_vec();
        output.extend_from_slice(&ciphertext);

        std::fs::write(path, output)?;

        // Set restrictive file permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }

        tracing::info!("Saved {} chats to encrypted history", self.chats.len());
        Ok(())
    }
}

use crate::app::ChatManager;

impl ChatManager {
    /// Load chat history from file
    pub fn load_history(&mut self, path: &Path) -> Result<()> {
        let history = HistoryFile::load(path)?;
        self.apply_history(history);
        Ok(())
    }

    /// Load history from encrypted or plaintext file with automatic migration.
    /// If encrypted history exists, loads it. Otherwise tries plaintext (legacy) and migrates it.
    /// Returns Ok(true) if history was loaded, Ok(false) if no history exists.
    pub fn load_history_auto(&mut self, encrypted_path: &Path, key: &[u8; 32]) -> Result<bool> {
        // Try encrypted history first
        if encrypted_path.exists() {
            tracing::info!(
                "Loading encrypted history from {}",
                encrypted_path.display()
            );
            return self
                .load_history_encrypted(encrypted_path, key)
                .map(|_| true);
        }

        // Try legacy plaintext history (for migration)
        let plaintext_path = encrypted_path.with_file_name("history.json");
        if plaintext_path.exists() {
            tracing::warn!(
                "Found legacy plaintext history at {}. Migrating to encrypted format...",
                plaintext_path.display()
            );

            // Load plaintext history
            self.load_history(&plaintext_path)?;

            // Immediately save as encrypted
            self.save_history(encrypted_path)?;

            // Remove the plaintext file to prevent future confusion
            if let Err(e) = std::fs::remove_file(&plaintext_path) {
                tracing::warn!("Could not delete plaintext history after migration: {}", e);
            }

            tracing::info!("Successfully migrated to encrypted history");
            return Ok(true);
        }

        // No history found
        tracing::info!("No history file found (new or first load)");
        Ok(false)
    }

    /// Load encrypted chat history from file
    pub fn load_history_encrypted(&mut self, path: &Path, key: &[u8; 32]) -> Result<()> {
        let history = HistoryFile::load_encrypted(path, key)?;
        self.apply_history(history);
        Ok(())
    }

    /// Save chat history to file
    pub fn save_history(&self, path: &Path) -> Result<()> {
        let mut history = HistoryFile::new(self.chats.values().cloned().collect());
        history.contacts = self.contacts.values().cloned().collect();
        history.config = self.config.clone();
        history.contact_chat_map = self.contact_to_chat.iter().map(|(k, v)| (*k, *v)).collect();
        let key = self
            .history_key
            .ok_or_else(|| anyhow!("History encryption key not available"))?;
        history.save_encrypted(path, &key)
    }

    /// Auto-save to default location
    pub fn auto_save(&self) -> Result<()> {
        let path = self.config.download_dir.join("history.json.enc");
        self.save_history(&path)
    }

    fn apply_history(&mut self, history: HistoryFile) {
        for chat in history.chats {
            self.chats.insert(chat.id, chat);
        }

        for contact in history.contacts {
            self.contacts.insert(contact.id, contact);
        }

        // Restore persisted contact/chat associations for reconnect
        self.contact_to_chat.extend(history.contact_chat_map);

        // Load persisted config (if present). Sanitize paths to prevent malicious
        // history files from redirecting writes to system or sensitive directories.
        self.config = sanitize_loaded_config(history.config);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ChatManager;
    use crate::types::{Chat, Message, MessageContent};
    use rand::RngCore;
    use tempfile::NamedTempFile;
    use uuid::Uuid;

    #[test]
    fn test_history_roundtrip() {
        let temp_file = NamedTempFile::new().unwrap();

        let chat = Chat {
            id: Uuid::new_v4(),
            title: "Test Chat".to_string(),
            peer_fingerprint: Some("abc123".to_string()),
            participants: Vec::new(),
            messages: Vec::new(),
            created_at: chrono::Utc::now(),
            peer_typing: false,
            typing_since: None,
            send_seq: 0,
            recv_seq: 0,
        };

        let history = HistoryFile::new(vec![chat.clone()]);

        // Save
        history.save(temp_file.path()).unwrap();

        // Load
        let loaded = HistoryFile::load(temp_file.path()).unwrap();

        assert_eq!(loaded.version, "1.0");
        assert_eq!(loaded.chats.len(), 1);
        assert_eq!(loaded.chats[0].id, chat.id);
        assert_eq!(loaded.chats[0].title, chat.title);
    }

    #[test]
    fn history_persists_contact_mapping() {
        let temp_file = NamedTempFile::new().unwrap();

        let mut manager = ChatManager::new(Config::default());
        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        manager.set_history_key(key);
        let chat_id = Uuid::new_v4();
        let contact_id = Uuid::new_v4();

        manager.chats.insert(
            chat_id,
            Chat {
                id: chat_id,
                title: "Chat".into(),
                peer_fingerprint: None,
                participants: Vec::new(),
                messages: Vec::new(),
                created_at: chrono::Utc::now(),
                peer_typing: false,
                typing_since: None,
                send_seq: 0,
                recv_seq: 0,
            },
        );

        manager.contacts.insert(
            contact_id,
            crate::types::Contact {
                id: contact_id,
                name: "Alice".into(),
                address: Some("127.0.0.1:5000".into()),
                fingerprint: None,
                public_key: None,
                created_at: chrono::Utc::now(),
                trust_state: crate::types::TrustState::Unverified,
                notes: String::new(),
                tags: Vec::new(),
                last_seen: None,
            },
        );

        manager.associate_contact_with_chat(contact_id, chat_id);
        manager.save_history(temp_file.path()).unwrap();

        let mut reloaded = ChatManager::new(Config::default());
        reloaded
            .load_history_encrypted(temp_file.path(), &key)
            .unwrap();

        assert_eq!(reloaded.contact_to_chat.get(&contact_id), Some(&chat_id));
    }

    #[test]
    fn save_history_requires_key() {
        let temp_file = NamedTempFile::new().unwrap();
        let manager = ChatManager::new(Config::default());
        let err = manager
            .save_history(temp_file.path())
            .expect_err("should require key");
        assert!(err.to_string().contains("History encryption key"));
    }

    #[test]
    fn save_history_writes_encrypted_payload() {
        let temp_file = NamedTempFile::new().unwrap();
        let mut manager = ChatManager::new(Config::default());
        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        manager.set_history_key(key);

        manager.save_history(temp_file.path()).unwrap();

        // Encrypted file should not be valid plaintext JSON
        let plaintext_load = HistoryFile::load(temp_file.path());
        assert!(plaintext_load.is_err());
    }

    /// PHASE 0 REGRESSION TEST: auto_save must always use encrypted storage.
    /// If this test fails, the audit's HIGH-priority plaintext autosave issue is present.
    #[test]
    fn test_autosave_always_encrypted() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut manager = ChatManager::new(Config {
            download_dir: temp_dir.path().to_path_buf(),
            ..Config::default()
        });

        // Set up a history key
        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        manager.set_history_key(key);

        // Add a chat with a message
        let chat_id = Uuid::new_v4();
        manager.chats.insert(
            chat_id,
            Chat {
                id: chat_id,
                title: "Test Chat".into(),
                peer_fingerprint: None,
                participants: Vec::new(),
                messages: vec![Message {
                    id: Uuid::new_v4(),
                    from_me: true,
                    content: MessageContent::Text {
                        text: "Secret message".to_string(),
                    },
                    timestamp: chrono::Utc::now(),
                }],
                created_at: chrono::Utc::now(),
                peer_typing: false,
                typing_since: None,
                send_seq: 0,
                recv_seq: 0,
            },
        );

        // Auto-save should succeed
        manager
            .auto_save()
            .expect("auto_save should succeed with key");

        // Verify the saved file is encrypted (not plaintext JSON)
        let history_path = temp_dir.path().join("history.json.enc");
        assert!(
            history_path.exists(),
            "history.json.enc should exist after auto_save"
        );

        // Try to load as plaintext JSON - should fail
        let result = HistoryFile::load(&history_path);
        assert!(
            result.is_err(),
            "history.json.enc should not be valid plaintext JSON"
        );

        // Verify it can be decrypted with the key
        let decrypted = HistoryFile::load_encrypted(&history_path, &key)
            .expect("Should decrypt with correct key");
        assert_eq!(decrypted.chats.len(), 1);
        assert_eq!(decrypted.chats[0].id, chat_id);
    }

    /// PHASE 0 REGRESSION TEST: auto_save without key should fail gracefully.
    /// This guards against fallback to unencrypted storage.
    #[test]
    fn test_autosave_without_key_fails() {
        let temp_dir = tempfile::tempdir().unwrap();
        let manager = ChatManager::new(Config {
            download_dir: temp_dir.path().to_path_buf(),
            ..Config::default()
        });

        // Do NOT set history_key
        let result = manager.auto_save();

        // Should fail, not silently save as plaintext
        assert!(
            result.is_err(),
            "auto_save should fail if history_key is not set"
        );
    }
}
