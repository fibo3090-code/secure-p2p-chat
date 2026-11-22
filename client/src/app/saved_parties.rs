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
#[derive(Clone, Serialize, Deserialize)]
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
