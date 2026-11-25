//! Saved communities and their pinned server fingerprints (`parties.json`).
//!
//! Shared by both shells: the desktop app and the terminal client pin a
//! community's fingerprint on its first successful join and require it on every
//! join after, so a server that swapped its key is refused before any
//! credential is sent. (The terminal client kept none, so every join was a
//! first contact — audit TUI-17.)

use serde::{Deserialize, Serialize};
use std::path::Path;

/// A community this client has joined, persisted (plaintext, no secrets — the
/// password is never stored) so it survives restarts: the UI offers one-click
/// rejoin, and the pinned server fingerprint turns the first join's TOFU into a
/// real trust anchor — a different identity at the same address is refused.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedParty {
    pub address: String,
    pub username: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub fingerprint: String,
}

/// Load the saved-communities list.
///
/// A missing file is an empty list — that is a first run. An *unreadable* one is
/// an error, deliberately. This file holds the pinned fingerprint of every
/// community the user has joined, and that pin is the only thing standing
/// between them and a server that swapped its key. Swallowing a parse failure
/// silently discarded every pin and turned the next join back into an
/// unverified first contact, which is exactly the situation the pin exists to
/// prevent — a corrupt file must stop the join, not quietly downgrade it.
pub fn load_saved_parties(path: &Path) -> Result<Vec<SavedParty>, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(format!(
                "Could not read your saved communities ({}): {e}. \
                 Not joining, because the fingerprints that protect those \
                 connections are stored there.",
                path.display()
            ))
        }
    };
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&raw).map_err(|e| {
        format!(
            "Your saved communities file ({}) is damaged: {e}. \
             It holds the fingerprints that protect those connections, so \
             joining is refused until it is repaired or removed.",
            path.display()
        )
    })
}

/// Save the saved-communities list.
///
/// Written through `write_file_atomic` — temp file, fsync, rename, 0600 from
/// creation — for the same reason `identity.json` is: a half-written file after
/// a crash is indistinguishable from a file with no pins in it.
pub fn save_saved_parties(path: &Path, list: &[SavedParty]) {
    match serde_json::to_string_pretty(list) {
        Ok(json) => {
            if let Err(e) = messenger_core::util::write_file_atomic(path, json.as_bytes()) {
                tracing::warn!("saving communities list failed: {e}");
            }
        }
        Err(e) => tracing::warn!("serializing communities list failed: {e}"),
    }
}

/// Insert or update the entry for `address` (matched case-insensitively).
///
/// A load failure aborts the write: rewriting the file from an empty list would
/// destroy every other community's pin.
pub fn upsert_saved_party(path: &Path, entry: SavedParty) {
    let mut list = match load_saved_parties(path) {
        Ok(list) => list,
        Err(e) => {
            tracing::error!("{e}");
            return;
        }
    };
    match list
        .iter_mut()
        .find(|p| p.address.eq_ignore_ascii_case(&entry.address))
    {
        Some(existing) => {
            existing.username = entry.username;
            if !entry.name.is_empty() {
                existing.name = entry.name;
            }
            if !entry.fingerprint.is_empty() {
                existing.fingerprint = entry.fingerprint;
            }
        }
        None => list.push(entry),
    }
    save_saved_parties(path, &list);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn party(address: &str, username: &str, fingerprint: &str) -> SavedParty {
        SavedParty {
            address: address.to_string(),
            username: username.to_string(),
            name: String::new(),
            fingerprint: fingerprint.to_string(),
        }
    }

    /// A first run has no file, and an empty one is the same: no communities.
    #[test]
    fn a_missing_or_empty_file_is_an_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("parties.json");
        assert!(load_saved_parties(&path).unwrap().is_empty());
        std::fs::write(&path, " \n").unwrap();
        assert!(load_saved_parties(&path).unwrap().is_empty());
    }

    /// The file holds every community's pin. A damaged or unreadable one
    /// stops the join — and, above all, is never rewritten from an empty list,
    /// which would destroy the pins of every other community.
    #[test]
    fn a_damaged_file_is_an_error_and_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("parties.json");
        let damaged = br#"[{"address":"a.example:9000","username":"me","fingerprint":"AB"#;
        std::fs::write(&path, damaged).unwrap();
        let err = load_saved_parties(&path).unwrap_err();
        assert!(err.contains("damaged"), "{err}");

        upsert_saved_party(&path, party("b.example:9000", "me", "CD"));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            damaged,
            "a failed load was followed by a rewrite that dropped the pins"
        );

        // Unreadable (a directory where the file should be) is refused too.
        let blocked = dir.path().join("blocked");
        std::fs::create_dir(&blocked).unwrap();
        let err = load_saved_parties(&blocked).unwrap_err();
        assert!(err.contains("Could not read"), "{err}");
    }

    /// Rejoining updates the entry for that address — matched without regard
    /// to case — and an update that carries no fingerprint or name keeps the
    /// ones on file: a pin is never cleared by a later, emptier write. Other
    /// communities are untouched.
    #[test]
    fn upserting_updates_one_entry_and_never_clears_a_pin() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("parties.json");
        upsert_saved_party(
            &path,
            SavedParty {
                name: "Club".into(),
                ..party("Chat.Example:9000", "alice", "PIN-ONE")
            },
        );
        upsert_saved_party(&path, party("other.example:9000", "alice", "PIN-TWO"));
        upsert_saved_party(&path, party("chat.example:9000", "alice2", ""));

        let list = load_saved_parties(&path).unwrap();
        assert_eq!(
            list.len(),
            2,
            "the case-insensitive match added a duplicate"
        );
        let club = list
            .iter()
            .find(|p| p.fingerprint == "PIN-ONE")
            .expect("pin cleared");
        assert_eq!(club.username, "alice2");
        assert_eq!(club.name, "Club");
        assert!(list.iter().any(|p| p.fingerprint == "PIN-TWO"));

        // Owner-only, like identity.json.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }
}
