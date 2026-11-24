use anyhow::{anyhow, Result};
use chacha20poly1305::{
    aead::{Aead, AeadCore, AeadInPlace},
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

/// System locations a history file must never redirect writes into, compared
/// per path component, case-insensitively, against the path's leading components.
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
/// Traversal is still rejected elsewhere; there is no system-directory list.
#[cfg(not(any(unix, windows)))]
const PROTECTED_ROOTS: &[&[&str]] = &[];

/// True for traversal (a `..` component) or a protected system root. Matched on
/// normalized components, not substrings: `/usrdata` and `my..files` are fine.
fn is_dangerous_path(p: &Path) -> bool {
    use std::path::Component;

    let mut normal: Vec<String> = Vec::new();
    for component in p.components() {
        match component {
            Component::ParentDir => return true,
            Component::Normal(part) => {
                normal.push(part.to_string_lossy().to_lowercase());
            }
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

/// Replace dangerous paths with defaults, and upgrade the legacy *relative*
/// defaults older builds persisted (they resolved against the working directory).
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
    /// The owning manager's write gate, attached by `history_snapshot`.
    #[serde(skip)]
    pub(crate) gate: Option<PersistGate>,
}

/// Serialises history writes against `ChatManager::delete_all_data`. Shells
/// snapshot under the lock and write later, so a save holds this gate for its
/// whole write and refuses once it reads `true`; the wipe sets it while holding
/// it (Codex review on PR #191).
pub type PersistGate = std::sync::Arc<std::sync::Mutex<bool>>;

impl HistoryFile {
    pub fn new(chats: Vec<Chat>) -> Self {
        Self {
            version: CURRENT_HISTORY_VERSION.to_string(),
            chats,
            contacts: Vec::new(),
            config: Config::default(),
            contact_chat_map: Vec::new(),
            gate: None,
        }
    }

    fn checked(history: Self) -> Result<Self> {
        if !history_version_supported(&history.version) {
            anyhow::bail!("Unsupported history version: {}", history.version);
        }
        Ok(history)
    }

    /// Load history from JSON file (plaintext - legacy support)
    pub fn load(path: &Path) -> Result<Self> {
        let history = Self::checked(serde_json::from_str(&std::fs::read_to_string(path)?)?)?;
        tracing::info!("Loaded {} chats from history", history.chats.len());
        Ok(history)
    }

    /// Load encrypted history (`nonce || ciphertext || tag`) from file.
    pub fn load_encrypted(path: &Path, key: &[u8; 32]) -> Result<Self> {
        let encrypted_data = std::fs::read(path)?;
        if encrypted_data.len() < 12 {
            return Err(anyhow!("Invalid encrypted history file: too short"));
        }
        let (nonce_bytes, ciphertext) = encrypted_data.split_at(12);
        let nonce = chacha20poly1305::Nonce::from(*<&[u8; 12]>::try_from(nonce_bytes)?);
        let plaintext = ChaCha20Poly1305::new(key.into())
            .decrypt(&nonce, ciphertext)
            .map_err(|e| anyhow!("Decryption failed (wrong password?): {}", e))?;
        let history = Self::checked(serde_json::from_slice(&plaintext)?)?;
        tracing::info!(
            "Loaded {} chats from encrypted history",
            history.chats.len()
        );
        Ok(history)
    }

    /// Save encrypted history to file. The only writer: plaintext history is
    /// read solely to migrate it.
    pub fn save_encrypted(&self, path: &Path, key: &[u8; 32]) -> Result<()> {
        // Held to the end of the write: see `PersistGate`.
        let _gate = match &self.gate {
            Some(gate) => {
                let held = gate.lock().unwrap_or_else(|e| e.into_inner());
                if *held {
                    anyhow::bail!("refusing to save: all data was deleted");
                }
                Some(held)
            }
            None => None,
        };
        // Compact JSON encrypted in place in one buffer (PERF-03); on disk it is
        // `nonce || ciphertext || tag`.
        const NONCE: usize = 12;
        let mut output = vec![0u8; NONCE];
        serde_json::to_writer(&mut output, &self)?;
        let cipher = ChaCha20Poly1305::new(key.into());
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        output[..NONCE].copy_from_slice(&nonce);
        let tag = cipher
            .encrypt_in_place_detached(&nonce, b"", &mut output[NONCE..])
            .map_err(|e| anyhow!("Encryption failed: {}", e))?;
        output.extend_from_slice(&tag);

        // Atomic + fsynced, 0600 from creation on Unix.
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

    /// Load the encrypted history, or migrate a legacy plaintext one. Returns
    /// `Ok(false)` when there is none.
    ///
    /// **A failed load blocks every later save** (see `history_load_error` and
    /// [`Self::set_aside_unreadable_history`]): otherwise the next save writes
    /// the empty state over a file that was merely unreadable.
    pub fn load_history_auto(&mut self, encrypted_path: &Path, key: &[u8; 32]) -> Result<bool> {
        let result = self.load_history_auto_inner(encrypted_path, key);
        match &result {
            Ok(_) => self.history_load_error = None,
            Err(e) => {
                let reason = format!("{e:#}");
                tracing::error!(
                    path = %encrypted_path.display(),
                    error = %reason,
                    "history could not be loaded; saving is blocked so the file is not overwritten"
                );
                self.history_load_error = Some(reason);
                self.add_toast(
                    ToastLevel::Error,
                    format!(
                        "Your message history at {} could not be read. It has not been changed, \
                         and nothing will be saved over it until you choose what to do.",
                        encrypted_path.display()
                    ),
                );
            }
        }
        result
    }

    fn load_history_auto_inner(&mut self, encrypted_path: &Path, key: &[u8; 32]) -> Result<bool> {
        if encrypted_path.exists() {
            tracing::info!(
                "Loading encrypted history from {}",
                encrypted_path.display()
            );
            return self
                .load_history_encrypted(encrypted_path, key)
                .map(|_| true);
        }

        let plaintext_path = encrypted_path.with_file_name("history.json");
        if plaintext_path.exists() {
            tracing::warn!(
                "Found legacy plaintext history at {}. Migrating to encrypted format...",
                plaintext_path.display()
            );
            self.load_history(&plaintext_path)?;
            self.save_history(encrypted_path)?;
            self.remove_migrated_plaintext_history(&plaintext_path);
            tracing::info!("Successfully migrated to encrypted history");
            return Ok(true);
        }

        tracing::info!("No history file found (new or first load)");
        Ok(false)
    }

    /// Dispose of the migrated plaintext history — messages must not stay
    /// readable at rest. Delete it; failing that, truncate it to zero and retry
    /// the delete; failing that, tell the user with an error toast.
    fn remove_migrated_plaintext_history(&mut self, plaintext_path: &Path) {
        match std::fs::remove_file(plaintext_path) {
            Ok(()) => return,
            Err(e) => tracing::warn!(
                path = %plaintext_path.display(),
                error = %e,
                "could not delete legacy plaintext history; emptying it instead"
            ),
        }

        let emptied = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(plaintext_path)
            .is_ok();
        if emptied {
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

    /// Why the history could not be loaded. While `Some`, saving is refused; a
    /// front-end should offer [`Self::set_aside_unreadable_history`].
    pub fn history_load_error(&self) -> Option<&str> {
        self.history_load_error.as_deref()
    }

    /// Rename an unreadable history file to `<name>.unreadable-<timestamp>`
    /// (never delete it) and allow saving again.
    pub fn set_aside_unreadable_history(
        &mut self,
        path: &Path,
    ) -> Result<Option<std::path::PathBuf>> {
        // With no encrypted file, the legacy `history.json` is what failed to
        // load, so that is the one to set aside (Codex review on PR #191).
        let legacy = path.with_file_name("history.json");
        let path = if !path.exists() && legacy.exists() {
            legacy.as_path()
        } else {
            path
        };
        let moved = if path.exists() {
            let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
            let file_name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "history.json.enc".to_string());
            let aside = path.with_file_name(format!("{file_name}.unreadable-{stamp}"));
            std::fs::rename(path, &aside)?;
            tracing::warn!(from = %path.display(), to = %aside.display(), "unreadable history set aside");
            Some(aside)
        } else {
            None
        };
        self.history_load_error = None;
        Ok(moved)
    }

    /// Load encrypted chat history from file
    pub fn load_history_encrypted(&mut self, path: &Path, key: &[u8; 32]) -> Result<()> {
        let history = HistoryFile::load_encrypted(path, key)?;
        self.apply_history(history);
        Ok(())
    }

    /// A cheap fingerprint of everything persisted (chats, contacts and their
    /// chat links, config). Shells skip a save when it is unchanged, since every
    /// save re-encrypts and fsyncs the whole history (PERF-03).
    pub fn persisted_signature(&self) -> u64 {
        use std::hash::{Hash, Hasher};

        let ids = self.chat_ids();
        let mut sig = ids.len() as u64;
        for id in &ids {
            if let Some(c) = self.get_chat(*id) {
                let delivered = c.messages.iter().filter(|m| m.delivered).count() as u64;
                let mut h = std::collections::hash_map::DefaultHasher::new();
                // The title itself, not its length, so a rename is noticed.
                c.title.hash(&mut h);
                c.peer_fingerprint.hash(&mut h);
                sig = sig
                    .wrapping_mul(1_000_003)
                    .wrapping_add(c.messages.len() as u64)
                    .wrapping_add(delivered.wrapping_mul(65_537))
                    .wrapping_add((c.read_count as u64).wrapping_mul(31))
                    .wrapping_add(h.finish());
            }
        }
        // Contacts share the file: imports, blocks and trust changes count.
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.contacts.len().hash(&mut h);
        for c in self.contacts.values() {
            c.id.hash(&mut h);
            c.name.hash(&mut h);
            c.fingerprint.hash(&mut h);
            c.address.hash(&mut h);
            c.addresses.hash(&mut h);
            c.relay_server.hash(&mut h);
            c.relay_token.hash(&mut h);
            (c.trust_state as u8).hash(&mut h);
        }
        let mut pairs: Vec<(Uuid, Uuid)> =
            self.contact_to_chat.iter().map(|(k, v)| (*k, *v)).collect();
        pairs.sort();
        pairs.hash(&mut h);
        // Hashing the serialization keeps a new config field from being missed.
        serde_json::to_vec(&self.config)
            .unwrap_or_default()
            .hash(&mut h);
        sig.wrapping_mul(31).wrapping_add(h.finish())
    }

    /// Save chat history to file
    pub fn save_history(&self, path: &Path) -> Result<()> {
        let (history, key) = self.history_snapshot()?;
        history.save_encrypted(path, &key)
    }

    /// Build a serializable snapshot for background persistence. Refused after
    /// a wipe or a failed load. Host placeholders (live listeners) are left out.
    pub fn history_snapshot(&self) -> Result<(HistoryFile, [u8; 32])> {
        if self.data_wiped {
            anyhow::bail!("refusing to save: all data was deleted");
        }
        if let Some(reason) = &self.history_load_error {
            anyhow::bail!(
                "refusing to save: the history on disk could not be loaded ({reason}), \
                 and saving would replace it"
            );
        }
        let key = self
            .history_key
            .ok_or_else(|| anyhow!("History encryption key not available"))?;
        let chats = self
            .chats
            .values()
            .filter(|c| !c.is_host_placeholder)
            .cloned()
            .collect();
        let mut history = HistoryFile::new(chats);
        history.gate = Some(self.persist_gate.clone());
        history.contacts = self.contacts.values().cloned().collect();
        history.config = self.config.clone();
        history.contact_chat_map = self.contact_to_chat.iter().map(|(k, v)| (*k, *v)).collect();
        Ok((history, key))
    }

    /// *Replace* (never merge) the in-memory state with what was loaded, so a
    /// deleted chat cannot reappear on a reload. Live host placeholders are
    /// kept: they represent a listener, not history.
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

        // A placeholder in the file was saved by an older build; its listener died.
        for chat in history.chats.into_iter().filter(|c| !c.is_host_placeholder) {
            self.chats.insert(chat.id, chat);
        }
        for placeholder in placeholders {
            self.chats.entry(placeholder.id).or_insert(placeholder);
        }

        for mut contact in history.contacts {
            // Trust comparisons are exact, so canonicalise legacy spellings.
            contact.fingerprint = contact
                .fingerprint
                .as_deref()
                .map(crate::util::canonical_fingerprint);
            self.contacts.insert(contact.id, contact);
        }
        self.contact_to_chat.extend(history.contact_chat_map);
        // A malicious history must not redirect writes.
        self.config = sanitize_loaded_config(history.config);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatKind, Message, MessageContent, Transport};
    use tempfile::NamedTempFile;

    /// A manager with a fresh random history key.
    fn keyed_manager() -> (ChatManager, [u8; 32]) {
        let key: [u8; 32] = rand::random();
        let mut manager = ChatManager::default();
        manager.set_history_key(key);
        (manager, key)
    }

    /// A host placeholder is never saved, and one an older build saved is
    /// dropped on load.
    #[test]
    fn host_placeholders_are_not_history() {
        let (mut manager, key) = keyed_manager();
        let real = Uuid::new_v4();
        manager.create_local_chat_for_test(real, "Alice".into());
        let listener = Uuid::new_v4();
        manager.create_local_chat_for_test(listener, "Host on :12345".into());
        manager.get_chat_mut(listener).unwrap().is_host_placeholder = true;

        let (mut history, _) = manager.history_snapshot().unwrap();
        let saved: Vec<Uuid> = history.chats.iter().map(|c| c.id).collect();
        assert_eq!(
            saved,
            vec![real],
            "the listener was saved as a conversation"
        );

        // A file an older build wrote, with the placeholder in it.
        let temp_file = NamedTempFile::new().unwrap();
        history
            .chats
            .push(manager.get_chat(listener).unwrap().clone());
        history.save_encrypted(temp_file.path(), &key).unwrap();
        let (mut reloaded, _) = keyed_manager();
        reloaded.load_history_auto(temp_file.path(), &key).unwrap();
        assert_eq!(reloaded.chat_ids(), vec![real]);
    }

    /// Loading replaces in-memory state (no resurrected chats) but keeps a live
    /// listener.
    #[test]
    fn loading_history_replaces_state_but_keeps_a_live_placeholder() {
        let temp_file = NamedTempFile::new().unwrap();
        let (mut manager, key) = keyed_manager();
        let kept = Uuid::new_v4();
        manager.create_local_chat_for_test(kept, "Kept".into());
        manager.save_history(temp_file.path()).unwrap();

        let stale = Uuid::new_v4();
        manager.create_local_chat_for_test(stale, "Deleted elsewhere".into());
        let placeholder_id = Uuid::new_v4();
        manager.create_local_chat_for_test(placeholder_id, "Listening".into());
        manager
            .get_chat_mut(placeholder_id)
            .unwrap()
            .is_host_placeholder = true;
        manager
            .load_history_encrypted(temp_file.path(), &key)
            .unwrap();

        assert!(manager.get_chat(kept).is_some());
        assert!(
            manager.get_chat(stale).is_none(),
            "a chat absent from the loaded history must not linger"
        );
        assert!(
            manager.get_chat(placeholder_id).is_some(),
            "the live listener must survive a history load"
        );
    }

    #[test]
    fn dangerous_paths_match_components_not_substrings() {
        assert!(is_dangerous_path(Path::new("../../etc")));
        assert!(is_dangerous_path(Path::new("/home/me/../../etc")));
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
            peer_fingerprint: Some("abc123".to_string()),
            ..Chat::new(Uuid::new_v4(), "Test Chat", ChatKind::Dm, Transport::Direct)
        };
        // A legacy plaintext file, as old versions wrote it.
        let history = HistoryFile::new(vec![chat.clone()]);
        std::fs::write(
            temp_file.path(),
            serde_json::to_string_pretty(&history).unwrap(),
        )
        .unwrap();

        let loaded = HistoryFile::load(temp_file.path()).unwrap();
        assert_eq!(loaded.version, CURRENT_HISTORY_VERSION);
        assert_eq!(loaded.chats.len(), 1);
        assert_eq!(loaded.chats[0].id, chat.id);
        assert_eq!(loaded.chats[0].title, chat.title);
    }

    #[test]
    fn history_persists_contact_mapping() {
        let temp_file = NamedTempFile::new().unwrap();
        let (mut manager, key) = keyed_manager();
        let chat_id = Uuid::new_v4();
        manager.create_local_chat_for_test(chat_id, "Chat".into());
        let contact_id =
            manager.add_contact("Alice".into(), Some("127.0.0.1:5000".into()), None, None);
        manager.associate_contact_with_chat(contact_id, chat_id);
        manager.save_history(temp_file.path()).unwrap();

        let mut reloaded = ChatManager::default();
        reloaded
            .load_history_encrypted(temp_file.path(), &key)
            .unwrap();
        assert_eq!(reloaded.contact_to_chat.get(&contact_id), Some(&chat_id));
    }

    /// Without a key, saving fails rather than falling back to plaintext.
    #[test]
    fn saving_requires_a_key() {
        let temp_file = NamedTempFile::new().unwrap();
        let manager = ChatManager::default();
        let err = manager
            .save_history(temp_file.path())
            .expect_err("should require key");
        assert!(err.to_string().contains("History encryption key"));
        assert!(
            manager.history_snapshot().is_err(),
            "history_snapshot should fail if history_key is not set"
        );
    }

    /// A history file that exists but cannot be read must never be replaced.
    #[test]
    fn an_unreadable_history_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.json.enc");
        // Written with a different key: decrypts as garbage.
        keyed_manager().0.save_history(&path).unwrap();
        let original = std::fs::read(&path).unwrap();

        let (mut mgr, key) = keyed_manager();
        assert!(mgr.load_history_auto(&path, &key).is_err());
        assert!(mgr.history_load_error().is_some());
        assert!(mgr.save_history(&path).is_err(), "saving must be refused");
        assert!(
            mgr.history_snapshot().is_err(),
            "background saves must be refused too"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            original,
            "the file is untouched"
        );

        // Even "clear history" sets the unreadable file aside, not over it.
        mgr.clear_history(&path);
        assert_eq!(
            std::fs::read_dir(dir.path())
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().contains(".unreadable-"))
                .map(|e| std::fs::read(e.path()).unwrap())
                .collect::<Vec<_>>(),
            vec![original],
            "the unreadable file must survive, set aside, byte for byte"
        );
    }

    #[test]
    fn setting_an_unreadable_history_aside_keeps_it_and_allows_saving() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.json.enc");
        std::fs::write(&path, b"not a history file at all").unwrap();

        let (mut mgr, key) = keyed_manager();
        assert!(mgr.load_history_auto(&path, &key).is_err());

        let aside = mgr.set_aside_unreadable_history(&path).unwrap().unwrap();
        assert_eq!(std::fs::read(&aside).unwrap(), b"not a history file at all");
        assert!(mgr.history_load_error().is_none());
        mgr.save_history(&path).unwrap();
        assert!(mgr.load_history_auto(&path, &key).unwrap());
    }

    /// With no encrypted file the legacy plaintext one is what failed, so it is
    /// the one set aside (Codex review on PR #191).
    #[test]
    fn an_unreadable_legacy_history_is_the_file_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let encrypted = dir.path().join("history.json.enc");
        let legacy = dir.path().join("history.json");
        std::fs::write(&legacy, b"{ not json").unwrap();

        let (mut mgr, key) = keyed_manager();
        assert!(mgr.load_history_auto(&encrypted, &key).is_err());
        assert!(mgr.history_load_error().is_some());

        let moved = mgr
            .set_aside_unreadable_history(&encrypted)
            .unwrap()
            .expect("the legacy file is moved");
        assert!(!legacy.exists());
        assert_eq!(std::fs::read(&moved).unwrap(), b"{ not json");
        assert!(mgr.history_load_error().is_none());
    }

    /// Both save paths (direct and background snapshot) write ciphertext.
    #[test]
    fn saved_history_is_always_encrypted() {
        let temp_dir = tempfile::tempdir().unwrap();
        let (mut manager, key) = keyed_manager();

        let direct = temp_dir.path().join("direct.enc");
        manager.save_history(&direct).unwrap();
        assert!(HistoryFile::load(&direct).is_err());

        let chat_id = Uuid::new_v4();
        manager.chats.insert(
            chat_id,
            Chat {
                messages: vec![Message::sent(
                    Uuid::new_v4(),
                    MessageContent::Text {
                        text: "Secret message".to_string(),
                    },
                )],
                ..Chat::new(chat_id, "Test Chat", ChatKind::Dm, Transport::Direct)
            },
        );
        let history_path = temp_dir.path().join("history.json.enc");
        let (history, snapshot_key) = manager
            .history_snapshot()
            .expect("history_snapshot should succeed with key");
        history
            .save_encrypted(&history_path, &snapshot_key)
            .expect("save_encrypted should succeed");

        assert!(
            HistoryFile::load(&history_path).is_err(),
            "history.json.enc should not be valid plaintext JSON"
        );
        let decrypted = HistoryFile::load_encrypted(&history_path, &key)
            .expect("Should decrypt with correct key");
        assert_eq!(decrypted.chats.len(), 1);
        assert_eq!(decrypted.chats[0].id, chat_id);
    }
}
