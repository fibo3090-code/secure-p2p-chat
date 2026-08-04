use anyhow::{anyhow, Result};
use chacha20poly1305::{
    aead::{Aead, AeadCore},
    ChaCha20Poly1305, KeyInit,
};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::types::{Chat, Config, ToastLevel};
use uuid::Uuid;

const CURRENT_HISTORY_VERSION: &str = "1.1";

fn history_version_supported(version: &str) -> bool {
    matches!(version, "1.0" | "1.1")
}

/// System locations a history file must never be able to redirect writes into.
/// Compared **per path component**, case-insensitively, against the leading
/// components of the candidate path.
#[cfg(unix)]
const PROTECTED_ROOTS: &[&[&str]] = &[
    &["etc"],
    &["usr"],
    &["bin"],
    &["sbin"],
    &["lib"],
    &["boot"],
    &["dev"],
    &["proc"],
    &["sys"],
    &["var"],
    &["root"],
];
#[cfg(windows)]
const PROTECTED_ROOTS: &[&[&str]] = &[
    &["windows"],
    &["program files"],
    &["program files (x86)"],
    &["programdata"],
];
/// Traversal is still rejected on any other target; there is no meaningful
/// system-directory list to apply.
#[cfg(not(any(unix, windows)))]
const PROTECTED_ROOTS: &[&[&str]] = &[];

/// Returns true if the path is considered dangerous (traversal or system dir).
/// Used to prevent malicious history files from redirecting writes to system paths.
///
/// Matching is done on **normalized path components**, not substrings. Substring
/// matching was both too loose and too strict: `starts_with("/usr")` also caught
/// `/usrdata`, while `contains("..")` rejected a perfectly ordinary directory
/// named `my..files`. Only a `..` component is real traversal, and only a
/// leading `usr` component is really `/usr`.
fn is_dangerous_path(p: &Path) -> bool {
    use std::path::Component;

    let mut normal: Vec<String> = Vec::new();
    for component in p.components() {
        match component {
            // Genuine traversal — a literal `..` component, not the two
            // characters appearing somewhere in a name.
            Component::ParentDir => return true,
            Component::Normal(part) => {
                normal.push(part.to_string_lossy().to_lowercase());
            }
            // Root/prefix/current-dir carry no name to compare.
            Component::RootDir | Component::CurDir | Component::Prefix(_) => {}
        }
    }

    PROTECTED_ROOTS.iter().any(|protected| {
        normal.len() >= protected.len()
            && protected
                .iter()
                .zip(normal.iter())
                .all(|(want, got)| want == got)
    })
}

/// Sanitize loaded config: replace dangerous paths with defaults, and upgrade
/// the legacy **relative** defaults (`Downloads`, `temp`) that older builds
/// persisted — those resolved against the process working directory, so
/// received files landed next to wherever the app was launched from.
fn sanitize_loaded_config(mut config: Config) -> Config {
    if is_dangerous_path(&config.download_dir) || config.download_dir.is_relative() {
        config.download_dir = crate::types::default_download_dir();
    }
    if is_dangerous_path(&config.temp_dir) || config.temp_dir.is_relative() {
        config.temp_dir = crate::types::default_temp_dir();
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
            version: CURRENT_HISTORY_VERSION.to_string(),
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

        if !history_version_supported(&history.version) {
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

        if !history_version_supported(&history.version) {
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
        // Compact, not pretty: this JSON is encrypted immediately and never read
        // by a human, so the indentation was pure cost — it inflated the
        // plaintext, the ciphertext, and every rewrite of the file. The whole
        // history is re-serialized on each change, so the saving compounds.
        let json = serde_json::to_string(&self)?;

        let cipher = ChaCha20Poly1305::new(key.into());
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ciphertext = cipher
            .encrypt(&nonce, json.as_bytes())
            .map_err(|e| anyhow!("Encryption failed: {}", e))?;

        // Format: nonce (12 bytes) || ciphertext
        let mut output = nonce.to_vec();
        output.extend_from_slice(&ciphertext);

        // Atomic + fsynced, with 0600 from creation on Unix. The old path wrote
        // a temp file and renamed but never flushed it, so a power loss could
        // land the rename with the data still in the page cache.
        crate::util::write_file_atomic(path, &output)?;

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

            self.remove_migrated_plaintext_history(&plaintext_path);

            tracing::info!("Successfully migrated to encrypted history");
            return Ok(true);
        }

        // No history found
        tracing::info!("No history file found (new or first load)");
        Ok(false)
    }

    /// Dispose of the legacy plaintext history once it has been migrated into
    /// the encrypted file.
    ///
    /// The product's core promise is that messages are encrypted at rest, so
    /// "couldn't delete it, carry on" is the wrong outcome — it leaves every
    /// message readable in the clear and says so only in a log line nobody
    /// reads. Escalating:
    ///
    /// 1. delete it (the normal path);
    /// 2. if the file cannot be unlinked (locked by antivirus, indexer, or a
    ///    permission quirk), **truncate it to zero bytes** so the plaintext is
    ///    gone even though the file remains, then try the delete again;
    /// 3. if even that fails, tell the user where the readable copy is, with an
    ///    error toast — not a silent warning.
    fn remove_migrated_plaintext_history(&mut self, plaintext_path: &Path) {
        match std::fs::remove_file(plaintext_path) {
            Ok(()) => return,
            Err(e) => tracing::warn!(
                path = %plaintext_path.display(),
                error = %e,
                "could not delete legacy plaintext history; emptying it instead"
            ),
        }

        // Emptying the file removes the readable content even when the
        // directory entry itself is pinned.
        let emptied = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(plaintext_path)
            .is_ok();
        if emptied {
            // Now that it holds nothing, a second delete attempt is worth it.
            let _ = std::fs::remove_file(plaintext_path);
            tracing::warn!(
                path = %plaintext_path.display(),
                "legacy plaintext history could not be deleted, but was emptied"
            );
            return;
        }

        tracing::error!(
            path = %plaintext_path.display(),
            "legacy plaintext history could NOT be removed or emptied; messages remain readable on disk"
        );
        self.add_toast(
            ToastLevel::Error,
            format!(
                "Your old unencrypted history at {} could not be removed. \
                 Your messages are still readable in that file — delete it manually.",
                plaintext_path.display()
            ),
        );
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

    /// Build a serializable snapshot for background persistence.
    pub fn history_snapshot(&self) -> Result<(HistoryFile, [u8; 32])> {
        let mut history = HistoryFile::new(self.chats.values().cloned().collect());
        history.contacts = self.contacts.values().cloned().collect();
        history.config = self.config.clone();
        history.contact_chat_map = self.contact_to_chat.iter().map(|(k, v)| (*k, *v)).collect();
        let key = self
            .history_key
            .ok_or_else(|| anyhow!("History encryption key not available"))?;
        Ok((history, key))
    }

    /// Replace the in-memory conversation state with what was loaded.
    ///
    /// This *replaces* rather than merges. Merging looked harmless while load
    /// only ever ran once at startup, but it meant a deleted chat would come
    /// back the moment a second load happened — the loaded file would be layered
    /// on top of current state instead of becoming it. Any future reload,
    /// import, or account-switch would have inherited that bug silently.
    ///
    /// Live sessions are unaffected: those live in `sessions`/`chat_id_mapping`,
    /// not here. Placeholder host chats are preserved for the same reason —
    /// they represent a live listener, not persisted history.
    fn apply_history(&mut self, history: HistoryFile) {
        let placeholders: Vec<Chat> = self
            .chats
            .values()
            .filter(|c| c.is_host_placeholder)
            .cloned()
            .collect();

        self.chats.clear();
        self.contacts.clear();
        self.contact_to_chat.clear();

        for chat in history.chats {
            self.chats.insert(chat.id, chat);
        }
        for placeholder in placeholders {
            self.chats.entry(placeholder.id).or_insert(placeholder);
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

    /// Loading history must *replace* in-memory state, not merge into it.
    /// Merging meant a deleted chat came back the moment a second load ran.
    #[test]
    fn loading_history_replaces_rather_than_merges() {
        let temp_file = NamedTempFile::new().unwrap();
        let mut manager = ChatManager::new(Config::default());
        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        manager.set_history_key(key);

        let kept = Uuid::new_v4();
        manager.create_local_chat_for_test(kept, "Kept".into());
        manager.save_history(temp_file.path()).unwrap();

        // A chat created (or resurrected) after that save must not survive a
        // reload — the file is the truth.
        let stale = Uuid::new_v4();
        manager.create_local_chat_for_test(stale, "Deleted elsewhere".into());
        manager
            .load_history_encrypted(temp_file.path(), &key)
            .unwrap();

        assert!(manager.get_chat(kept).is_some());
        assert!(
            manager.get_chat(stale).is_none(),
            "a chat absent from the loaded history must not linger"
        );
    }

    /// A live listener is not persisted history, so a reload must not drop it
    /// out from under the running session.
    #[test]
    fn loading_history_preserves_a_live_host_placeholder() {
        let temp_file = NamedTempFile::new().unwrap();
        let mut manager = ChatManager::new(Config::default());
        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        manager.set_history_key(key);
        manager.save_history(temp_file.path()).unwrap();

        let placeholder_id = Uuid::new_v4();
        manager.create_local_chat_for_test(placeholder_id, "Listening".into());
        manager
            .get_chat_mut(placeholder_id)
            .unwrap()
            .is_host_placeholder = true;

        manager
            .load_history_encrypted(temp_file.path(), &key)
            .unwrap();
        assert!(
            manager.get_chat(placeholder_id).is_some(),
            "the live listener must survive a history load"
        );
    }

    #[test]
    fn dangerous_paths_match_components_not_substrings() {
        // Real traversal is rejected.
        assert!(is_dangerous_path(Path::new("../../etc")));
        assert!(is_dangerous_path(Path::new("/home/me/../../etc")));
        // A name that merely contains ".." is a perfectly ordinary directory.
        assert!(!is_dangerous_path(Path::new("/home/me/my..files")));
        assert!(!is_dangerous_path(Path::new("/home/me/archive..2024")));
    }

    #[cfg(unix)]
    #[test]
    fn unix_system_roots_are_rejected_without_false_positives() {
        for bad in [
            "/etc/x", "/usr/lib", "/bin", "/sbin/x", "/var/lib", "/root", "/proc/1",
        ] {
            assert!(
                is_dangerous_path(Path::new(bad)),
                "{bad} should be rejected"
            );
        }
        // A prefix match on the *string* would wrongly catch these.
        for ok in [
            "/usrdata/files",
            "/etcetera",
            "/home/user/binaries",
            "/rootfs-backup",
        ] {
            assert!(!is_dangerous_path(Path::new(ok)), "{ok} should be allowed");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_system_roots_are_rejected_without_false_positives() {
        for bad in [
            r"C:\Windows\System32",
            r"C:\windows",
            r"C:\Program Files\App",
            r"C:\Program Files (x86)\App",
            r"C:\ProgramData\x",
        ] {
            assert!(
                is_dangerous_path(Path::new(bad)),
                "{bad} should be rejected"
            );
        }
        for ok in [
            r"C:\Users\me\Downloads",
            r"C:\WindowsApps-backup",
            r"D:\Program Files Backup",
        ] {
            assert!(!is_dangerous_path(Path::new(ok)), "{ok} should be allowed");
        }
    }

    #[test]
    fn test_history_roundtrip() {
        let temp_file = NamedTempFile::new().unwrap();

        let chat = Chat {
            id: Uuid::new_v4(),
            title: "Test Chat".to_string(),
            kind: crate::types::ChatKind::Dm,
            transport: crate::types::Transport::Direct,
            peer_fingerprint: Some("abc123".to_string()),
            participants: Vec::new(),
            messages: Vec::new(),
            created_at: chrono::Utc::now(),
            peer_typing: false,
            typing_since: None,
            send_seq: 0,
            recv_seq: 0,
            is_host_placeholder: false,
            read_count: 0,
            title_is_custom: false,
        };

        let history = HistoryFile::new(vec![chat.clone()]);

        // Save
        history.save(temp_file.path()).unwrap();

        // Load
        let loaded = HistoryFile::load(temp_file.path()).unwrap();

        assert_eq!(loaded.version, CURRENT_HISTORY_VERSION);
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
                kind: crate::types::ChatKind::Dm,
                transport: crate::types::Transport::Direct,
                peer_fingerprint: None,
                participants: Vec::new(),
                messages: Vec::new(),
                created_at: chrono::Utc::now(),
                peer_typing: false,
                typing_since: None,
                send_seq: 0,
                recv_seq: 0,
                is_host_placeholder: false,
                read_count: 0,
                title_is_custom: false,
            },
        );

        manager.contacts.insert(
            contact_id,
            crate::types::Contact {
                id: contact_id,
                name: "Alice".into(),
                address: Some("127.0.0.1:5000".into()),
                addresses: Vec::new(),
                relay_server: None,
                relay_token: None,
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

    /// PHASE 0 REGRESSION TEST: background persistence snapshots must remain encrypted.
    #[test]
    fn test_history_snapshot_always_encrypted() {
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
                kind: crate::types::ChatKind::Dm,
                transport: crate::types::Transport::Direct,
                peer_fingerprint: None,
                participants: Vec::new(),
                messages: vec![Message {
                    id: Uuid::new_v4(),
                    from_me: true,
                    content: MessageContent::Text {
                        text: "Secret message".to_string(),
                    },
                    timestamp: chrono::Utc::now(),
                    delivered: false,
                }],
                created_at: chrono::Utc::now(),
                peer_typing: false,
                typing_since: None,
                send_seq: 0,
                recv_seq: 0,
                is_host_placeholder: false,
                read_count: 0,
                title_is_custom: false,
            },
        );

        let history_path = temp_dir.path().join("history.json.enc");
        let (history, snapshot_key) = manager
            .history_snapshot()
            .expect("history_snapshot should succeed with key");
        history
            .save_encrypted(&history_path, &snapshot_key)
            .expect("save_encrypted should succeed");

        // Verify the saved file is encrypted (not plaintext JSON)
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

    /// PHASE 0 REGRESSION TEST: history snapshots without a key must fail gracefully.
    #[test]
    fn test_history_snapshot_without_key_fails() {
        let manager = ChatManager::new(Config {
            ..Config::default()
        });

        // Do NOT set history_key
        let result = manager.history_snapshot();

        // Should fail, not silently save as plaintext
        assert!(
            result.is_err(),
            "history_snapshot should fail if history_key is not set"
        );
    }
}
