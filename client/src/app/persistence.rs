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

    /// Save encrypted history to file. The only writer: plaintext history is
    /// read solely to migrate it.
    pub fn save_encrypted(&self, path: &Path, key: &[u8; 32]) -> Result<()> {
        // Compact, not pretty: this JSON is encrypted immediately and never read
        // by a human, so the indentation was pure cost — it inflated the
        // plaintext, the ciphertext, and every rewrite of the file. The whole
        // history is re-serialized on each change, so the saving compounds.
        //
        // One buffer end to end — nonce, then the JSON encrypted in place,
        // then the tag — instead of JSON, ciphertext and a concatenated copy:
        // three full copies of the history less at peak (PERF-03). The bytes
        // on disk are identical (`nonce || ciphertext || tag`).
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
    ///
    /// **A failed load blocks every later save** until the user decides what to
    /// do (see [`Self::history_load_error`] and
    /// [`Self::set_aside_unreadable_history`]). The callers used to log the
    /// error and carry on with an empty, unlocked manager, and the next save —
    /// on the desktop, the first poll tick — wrote that emptiness over the file.
    /// Downgrading to an older build, a history file from a newer one, a cloud
    /// placeholder that was not downloaded, or bit-rot was enough to lose every
    /// conversation and contact.
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

    /// Why the history could not be loaded, if it could not. While this is
    /// `Some`, saving is refused; a front-end should tell the user and offer
    /// [`Self::set_aside_unreadable_history`].
    pub fn history_load_error(&self) -> Option<&str> {
        self.history_load_error.as_deref()
    }

    /// Move an unreadable history file out of the way (renamed to
    /// `<name>.unreadable-<timestamp>`, never deleted) and allow saving again,
    /// so the user can start a fresh history without losing the old file —
    /// it can still be opened by the build that wrote it.
    pub fn set_aside_unreadable_history(
        &mut self,
        path: &Path,
    ) -> Result<Option<std::path::PathBuf>> {
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

    /// A cheap fingerprint of everything that is persisted: chats (count,
    /// titles, trust, messages, delivery, read marks), contacts and their
    /// association with chats, and the config. Both shells compare it with the
    /// value at the last save and skip the write when nothing changed — every
    /// save re-encrypts and fsyncs the whole history (PERF-03).
    pub fn persisted_signature(&self) -> u64 {
        use std::hash::{Hash, Hasher};

        let ids = self.chat_ids();
        let mut sig = ids.len() as u64;
        for id in &ids {
            if let Some(c) = self.get_chat(*id) {
                let delivered = c.messages.iter().filter(|m| m.delivered).count() as u64;
                let mut h = std::collections::hash_map::DefaultHasher::new();
                // The title itself, not its length: renaming "Mum" to "Dad"
                // changes nothing a length-only signature can see.
                c.title.hash(&mut h);
                c.peer_fingerprint.hash(&mut h);
                sig = sig
                    .wrapping_mul(1_000_003)
                    .wrapping_add(c.messages.len() as u64)
                    .wrapping_add(delivered.wrapping_mul(65_537))
                    // The read mark is persisted and drives the unread badge.
                    .wrapping_add((c.read_count as u64).wrapping_mul(31))
                    .wrapping_add(h.finish());
            }
        }
        // Contacts live in the same encrypted file, so a signature that
        // ignored them could not notice an imported invite, a block, or a
        // fingerprint promoted to Verified.
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
        // Config is small; hashing its serialization keeps a new field from
        // being silently left out.
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

    /// Build a serializable snapshot for background persistence.
    ///
    /// Host placeholders are left out: one stands for a listener that is
    /// running *now*. Saved, it came back after a restart as a "Host on :port"
    /// conversation shown as waiting for a peer, with nothing listening.
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
        history.contacts = self.contacts.values().cloned().collect();
        history.config = self.config.clone();
        history.contact_chat_map = self.contact_to_chat.iter().map(|(k, v)| (*k, *v)).collect();
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

        // A placeholder in the file was saved by an older build; it stands for
        // a listener that died with that process.
        for chat in history.chats.into_iter().filter(|c| !c.is_host_placeholder) {
            self.chats.insert(chat.id, chat);
        }
        for placeholder in placeholders {
            self.chats.entry(placeholder.id).or_insert(placeholder);
        }

        for mut contact in history.contacts {
            // Older builds stored fingerprints as typed or as the invite spelled
            // them; every trust comparison is exact, so canonicalise on load.
            contact.fingerprint = contact
                .fingerprint
                .as_deref()
                .map(crate::util::canonical_fingerprint);
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

    /// A host placeholder is a listener, not history: it is never saved, and
    /// one saved by an older build is dropped on load rather than coming back
    /// as a "waiting for peer" conversation with nothing listening.
    #[test]
    fn host_placeholders_are_not_history() {
        let mut manager = ChatManager::new(Config::default());
        manager.set_history_key([3u8; 32]);
        let real = Uuid::new_v4();
        manager.create_local_chat_for_test(real, "Alice".into());
        let listener = Uuid::new_v4();
        manager.create_local_chat_for_test(listener, "Host on :12345".into());
        manager.get_chat_mut(listener).unwrap().is_host_placeholder = true;

        let (history, _) = manager.history_snapshot().unwrap();
        let saved: Vec<Uuid> = history.chats.iter().map(|c| c.id).collect();
        assert_eq!(
            saved,
            vec![real],
            "the listener was saved as a conversation"
        );

        // A file an older build wrote, with the placeholder in it.
        let temp_file = NamedTempFile::new().unwrap();
        let mut legacy = history;
        legacy
            .chats
            .push(manager.get_chat(listener).unwrap().clone());
        legacy.save_encrypted(temp_file.path(), &[3u8; 32]).unwrap();
        let mut reloaded = ChatManager::new(Config::default());
        reloaded.set_history_key([3u8; 32]);
        reloaded
            .load_history_auto(temp_file.path(), &[3u8; 32])
            .unwrap();
        assert_eq!(reloaded.chat_ids(), vec![real]);
    }

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
            peer_fingerprint: Some("abc123".to_string()),
            ..Chat::new(
                Uuid::new_v4(),
                "Test Chat",
                crate::types::ChatKind::Dm,
                crate::types::Transport::Direct,
            )
        };

        let history = HistoryFile::new(vec![chat.clone()]);

        // Write a legacy plaintext file the way old versions did. There is no
        // plaintext writer any more — only the migration path reads this format.
        std::fs::write(
            temp_file.path(),
            serde_json::to_string_pretty(&history).unwrap(),
        )
        .unwrap();

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
            Chat::new(
                chat_id,
                "Chat",
                crate::types::ChatKind::Dm,
                crate::types::Transport::Direct,
            ),
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

    /// A history file that exists but cannot be read must never be replaced.
    /// Every caller used to warn and carry on, and the next save wrote the
    /// empty state the failed load left behind over every conversation.
    #[test]
    fn an_unreadable_history_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.json.enc");
        // Written with a different key: decrypts as garbage, like a file from
        // another identity, a newer build, or bit-rot.
        let mut other = ChatManager::new(Config::default());
        other.set_history_key([7u8; 32]);
        other.save_history(&path).unwrap();
        let original = std::fs::read(&path).unwrap();

        let mut mgr = ChatManager::new(Config::default());
        mgr.set_history_key([1u8; 32]);
        assert!(mgr.load_history_auto(&path, &[1u8; 32]).is_err());
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

        // Even an explicit "clear history" sets the unreadable file aside
        // rather than writing over it.
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

        let mut mgr = ChatManager::new(Config::default());
        mgr.set_history_key([1u8; 32]);
        assert!(mgr.load_history_auto(&path, &[1u8; 32]).is_err());

        let aside = mgr.set_aside_unreadable_history(&path).unwrap().unwrap();
        assert_eq!(std::fs::read(&aside).unwrap(), b"not a history file at all");
        assert!(mgr.history_load_error().is_none());
        mgr.save_history(&path).unwrap();
        assert!(mgr.load_history_auto(&path, &[1u8; 32]).unwrap());
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
                messages: vec![Message {
                    id: Uuid::new_v4(),
                    from_me: true,
                    content: MessageContent::Text {
                        text: "Secret message".to_string(),
                    },
                    timestamp: chrono::Utc::now(),
                    delivered: false,
                }],
                ..Chat::new(
                    chat_id,
                    "Test Chat",
                    crate::types::ChatKind::Dm,
                    crate::types::Transport::Direct,
                )
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
