//! Identity lifecycle (unlock, set-password, status) and user settings.
use crate::*;

#[derive(Serialize)]
pub(crate) struct AuthStatus {
    state: &'static str,
    name: String,
    fingerprint: String,
    /// The password floor the core enforces, surfaced so the set-password
    /// screen validates against the real rule instead of a hardcoded number
    /// that can silently drift from it.
    min_password_len: usize,
    /// Populated only when `state == "error"`: what went wrong at startup, in
    /// words meant for the user.
    error: Option<String>,
}

#[tauri::command]
pub(crate) fn auth_status(state: tauri::State<'_, Bridge>) -> AuthStatus {
    // A startup failure outranks every other state. In particular it must NOT
    // present as "set_password", which would invite the user to create a new
    // identity over the one that failed to load.
    if let Some(err) = &state.init_error {
        return AuthStatus {
            state: "error",
            name: String::new(),
            fingerprint: String::new(),
            min_password_len: messenger_core::MIN_PASSWORD_LEN,
            error: Some(err.clone()),
        };
    }
    let id = crate::lock_identity(&state.identity);
    let st = if id.is_locked() {
        "unlock"
    } else if state.needs_password_setup() {
        "set_password"
    } else {
        "ready"
    };
    AuthStatus {
        state: st,
        name: id.name.clone(),
        fingerprint: id.fingerprint.clone(),
        min_password_len: messenger_core::MIN_PASSWORD_LEN,
        error: None,
    }
}

/// Refuse the auth commands when startup could not read the identity file.
///
/// These two are deliberately *not* behind `ensure_ready` (they are what makes
/// the app ready), so they need their own guard. `set_password` in particular
/// must never run here: it would encrypt and save the throwaway identity created
/// only to display the error, overwriting the unreadable-but-recoverable file
/// with a brand new one — the exact destruction this whole path exists to
/// prevent.
fn ensure_identity_loaded(state: &Bridge) -> Result<(), String> {
    match &state.init_error {
        Some(err) => Err(err.clone()),
        None => Ok(()),
    }
}

/// Unlock an existing, password-protected identity and load history.
#[tauri::command]
pub(crate) async fn unlock(
    password: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_identity_loaded(&state)?;
    // Already unlocked: nothing to do. Running the body again re-ran Argon2
    // and reloaded the history file over the live state — `apply_history`
    // replaces, so everything not yet saved (saves are coalesced) was dropped
    // while its sessions stayed open. A double-submitted form was enough.
    if !crate::lock_identity(&state.identity).is_locked() && !state.needs_password_setup() {
        return Ok(());
    }
    let key = {
        let mut id = crate::lock_identity(&state.identity);
        id.decrypt(&password)
            .map_err(|_| "Wrong password".to_string())?;
        let key = id.history_key().map_err(|e| e.to_string())?;
        state.password_setup_done();
        key
    };
    let mut mgr = state.manager.lock().await;
    mgr.set_history_key(key);
    if let Err(e) = mgr.load_history_auto(&state.history_path, &key) {
        tracing::warn!("Failed to load history after unlock: {}", e);
    }
    auto_host_if_configured(&state, &mut mgr).await;
    Ok(())
}

/// Honor `auto_host_on_startup` and `auto_connect` once the identity is
/// unlocked and history (with its persisted config) is loaded — "open the app
/// and be reachable / reconnected", matching the terminal client. Failures are
/// logged, never fatal to the unlock.
async fn auto_host_if_configured(state: &tauri::State<'_, Bridge>, mgr: &mut ChatManager) {
    let pk_result = crate::lock_identity(&state.identity).private_key();
    let pk = match pk_result {
        Ok(pk) => pk,
        Err(e) => {
            tracing::warn!("auto-host/auto-connect skipped: no private key: {e}");
            return;
        }
    };
    if mgr.config.auto_host_on_startup {
        let port = mgr.config.listen_port;
        if let Err(e) = mgr.start_host(port, pk.clone()).await {
            tracing::warn!("auto-host on startup failed: {e}");
        } else {
            tracing::info!(port, "auto-hosting on startup");
        }
    }
    // Self-gated on config.auto_connect; best-effort per contact.
    mgr.auto_reconnect_contacts(&pk).await;
}

/// Set a password on a fresh / plaintext identity, persist it, and stay unlocked.
#[tauri::command]
pub(crate) async fn set_password(
    password: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_identity_loaded(&state)?;
    // First setup only. Once a password exists the plaintext key is in memory
    // while unlocked, so `encrypt` would happily re-wrap `identity.json` under
    // whatever the caller sent — no current password asked. Anything able to
    // invoke a command could lock the owner out of their identity and history.
    if !state.needs_password_setup() {
        return Err("A password is already set — use Change password instead".to_string());
    }
    let key = {
        let mut id = crate::lock_identity(&state.identity);
        // `encrypt` enforces the length floor; the message it returns is
        // already user-facing, so pass it straight through to the screen.
        id.encrypt(&password).map_err(|e| e.to_string())?;
        id.save(&state.identity_path).map_err(|e| e.to_string())?;
        // Decrypt in memory so we remain unlocked for this session.
        id.decrypt(&password).map_err(|e| e.to_string())?;
        let key = id.history_key().map_err(|e| e.to_string())?;
        state.password_setup_done();
        key
    };
    // Load any existing history the same way `unlock` does. Without this, the
    // first post-setup session starts from an empty manager and the next save
    // could overwrite an existing `history.json.enc` with empty state.
    let mut mgr = state.manager.lock().await;
    mgr.set_history_key(key);
    if let Err(e) = mgr.load_history_auto(&state.history_path, &key) {
        tracing::warn!("Failed to load history after password setup: {}", e);
    }
    auto_host_if_configured(&state, &mut mgr).await;
    Ok(())
}

/// Change the password protecting the identity file.
///
/// The private key — and therefore the history key derived from it — is
/// unchanged, so every stored message stays readable and the fingerprint every
/// contact verified stays the same. Only the Argon2/ChaCha20 wrapper around the
/// key is replaced.
///
/// The re-wrap is done on a **copy** and only swapped into the shared identity
/// once the new file is safely on disk. Mutating in place first would, on a
/// failed write, leave the running app wanting the new password while the file
/// on disk still wanted the old one — and the next attempt would then report
/// "current password is incorrect" for the password the user just set.
#[tauri::command]
pub(crate) async fn change_password(
    current: String,
    new: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let path = state.identity_path.clone();
    // Argon2 at 64 MiB × 3 passes runs twice here (verify + re-wrap), which is
    // ~1s of CPU — off the async runtime so the UI keeps painting.
    let mut candidate = crate::lock_identity(&state.identity).clone();
    let candidate = tokio::task::spawn_blocking(move || -> Result<Identity, String> {
        candidate
            .change_password(&current, &new)
            .map_err(|e| e.to_string())?;
        candidate.save(&path).map_err(|e| e.to_string())?;
        Ok(candidate)
    })
    .await
    .map_err(|e| e.to_string())??;
    *crate::lock_identity(&state.identity) = candidate;
    Ok(())
}

#[tauri::command]
pub(crate) fn my_identity(state: tauri::State<'_, Bridge>) -> AuthStatus {
    auth_status(state)
}

/// Change the identity's display name (used in invite links and the UI) and
/// persist it. The key material and fingerprint are untouched.
#[tauri::command]
pub(crate) fn set_display_name(
    name: String,
    state: tauri::State<'_, Bridge>,
) -> Result<AuthStatus, String> {
    ensure_ready(&state)?;
    {
        let mut id = crate::lock_identity(&state.identity);
        id.set_name(&name).map_err(|e| e.to_string())?;
        id.save(&state.identity_path).map_err(|e| e.to_string())?;
    }
    Ok(auth_status(state))
}

/// Whether the history on disk failed to load this session.
#[derive(serde::Serialize)]
pub(crate) struct HistoryStatus {
    /// `Some(reason)` when the file exists but could not be read. Saving is
    /// blocked while this is set (see `ChatManager::history_load_error`), so
    /// the UI must say so and offer [`set_aside_history`].
    error: Option<String>,
    /// Where the history lives, so the message can name the file.
    path: String,
}

#[tauri::command]
pub(crate) async fn history_status(
    state: tauri::State<'_, Bridge>,
) -> Result<HistoryStatus, String> {
    ensure_ready(&state)?;
    let mgr = state.manager.lock().await;
    Ok(HistoryStatus {
        error: mgr.history_load_error().map(str::to_string),
        path: state.history_path.display().to_string(),
    })
}

/// Move an unreadable history file aside (renamed, never deleted) and start a
/// fresh history. Returns where the old file went.
#[tauri::command]
pub(crate) async fn set_aside_history(
    state: tauri::State<'_, Bridge>,
) -> Result<Option<String>, String> {
    ensure_ready(&state)?;
    let moved = state
        .manager
        .lock()
        .await
        .set_aside_unreadable_history(&state.history_path)
        .map_err(|e| format!("Could not move the unreadable history aside: {e}"))?;
    persist_history(&state.manager, &state.history_path, &state.saved_sig).await;
    Ok(moved.map(|p| p.display().to_string()))
}

/// What the user types to confirm deleting everything.
pub(crate) const DELETE_CONFIRMATION: &str = "DELETE";

/// Delete everything this app stores on this device — identity, encrypted
/// history, saved communities, diagnostics — then quit (audit JOURNEY-03 /
/// DOC-02: the guide promised this and nothing implemented it).
///
/// Guarded twice, because it cannot be undone and is the one action that
/// destroys the identity itself: the password, checked against the file on
/// disk, and the typed word `DELETE`. Communities and discovery are dropped
/// first, then `ChatManager::delete_all_data` stops hosting and sessions,
/// removes the files and refuses every later save, so the window-close flush
/// cannot write a history file back.
#[tauri::command]
pub(crate) async fn delete_all_data<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    password: String,
    confirm: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    if confirm.trim() != DELETE_CONFIRMATION {
        return Err(format!("Type {DELETE_CONFIRMATION} to confirm"));
    }
    let path = state.identity_path.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut on_disk = Identity::load(&path).map_err(|e| e.to_string())?;
        on_disk
            .decrypt(&password)
            .map_err(|_| "Wrong password — nothing was deleted".to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    {
        let mut party = state.party.lock().await;
        for id in party.server_ids() {
            party.remove_server(id);
        }
    }
    *crate::lock_or_recover(&state.discovery) = None;
    let data_dir = state
        .history_path
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
    state
        .manager
        .lock()
        .await
        .delete_all_data(&data_dir, &state.history_path, &state.identity_path)
        .map_err(|e| format!("Deleting data failed part-way: {e}"))?;
    tracing::warn!("all local data deleted at the user's request; exiting");
    // Not under the mock runtime: the test process is the app there.
    #[cfg(not(test))]
    app.exit(0);
    let _ = app;
    Ok(())
}

/// Export a backup copy of the encrypted identity file (identity.json) to a
/// user-chosen location. The file is already encrypted at rest (Argon2 +
/// ChaCha20-Poly1305), so the copy is as safe as the original — but it IS the
/// identity: losing it (and the password) means losing the account.
/// Returns the destination path, or None if the dialog was cancelled.
#[tauri::command]
pub(crate) async fn export_identity<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: tauri::State<'_, Bridge>,
) -> Result<Option<String>, String> {
    ensure_ready(&state)?;
    let source = state.identity_path.clone();
    if !source.exists() {
        return Err("No identity file to back up yet".to_string());
    }
    let picked = crate::native_file_dialog(
        window,
        crate::Dialog::Save {
            title: "Save identity backup",
            file_name: "p2pem-identity-backup.json".to_string(),
        },
    )
    .await?;
    let Some(dest) = picked else {
        return Ok(None); // cancelled
    };
    backup_identity_to(&state, &dest).await?;
    Ok(Some(dest.display().to_string()))
}

/// Copy the (encrypted) identity file to `dest` and record that a backup
/// exists, persisting that. The half of [`export_identity`] behind its save
/// dialog, split out so it is testable without one.
pub(crate) async fn backup_identity_to(
    state: &Bridge,
    dest: &std::path::Path,
) -> Result<(), String> {
    // Never onto the source. The dialog opens wherever the user points it, and
    // copying a file onto itself truncates it on Linux — destroying the only
    // key to the history and then recording "backed up" (TAURI-31). Compared
    // canonically, so `./identity.json` or a symlink cannot slip past; the
    // destination need not exist yet, so its directory is what is resolved.
    let canonical = |p: &std::path::Path| -> Option<std::path::PathBuf> {
        let dir = p.parent().filter(|d| !d.as_os_str().is_empty())?;
        Some(std::fs::canonicalize(dir).ok()?.join(p.file_name()?))
    };
    let source = std::fs::canonicalize(&state.identity_path).ok();
    let target = std::fs::canonicalize(dest).ok().or_else(|| canonical(dest));
    if source.is_some() && source == target {
        return Err("Choose somewhere other than the identity file itself for the backup".into());
    }
    tokio::fs::copy(&state.identity_path, dest)
        .await
        .map_err(|e| format!("Backup failed: {e}"))?;
    // A backup that cannot be read back is not one: only record it if it
    // parses as an identity (a short write on a full disk returns Ok too).
    if let Err(e) = messenger_core::identity::Identity::load(dest) {
        return Err(format!(
            "The backup could not be read back ({e}); it was not recorded"
        ));
    }
    // Record that a backup exists so the app can stop nagging — and, more
    // importantly, so it *can* nag when one never happened.
    {
        let mut mgr = state.manager.lock().await;
        mgr.config.identity_backed_up_at = Some(chrono::Utc::now().to_rfc3339());
    }
    persist_history(&state.manager, &state.history_path, &state.saved_sig).await;
    Ok(())
}

/// Export a support bundle (state metadata + config, never key material) to
/// `<data dir>/diagnostics/bundle-<stamp>/`, the same bundle the terminal client exports.
/// Returns the bundle directory path.
#[tauri::command]
pub(crate) async fn export_diagnostics(state: tauri::State<'_, Bridge>) -> Result<String, String> {
    ensure_ready(&state)?;
    let (identity_locked, identity_name, fp_prefix) = {
        let id = crate::lock_identity(&state.identity);
        (
            id.is_locked(),
            id.name.clone(),
            id.fingerprint.chars().take(16).collect::<String>(),
        )
    };
    let report = {
        let mgr = state.manager.lock().await;
        p2pem_classic::support::DiagnosticsReport {
            generated_at_utc: chrono::Utc::now().to_rfc3339(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            history_path: state.history_path.display().to_string(),
            identity_path: state.identity_path.display().to_string(),
            history_exists: state.history_path.exists(),
            identity_exists: state.identity_path.exists(),
            identity_locked,
            identity_name,
            identity_fingerprint_prefix: fp_prefix,
            chats: mgr.chats.len(),
            contacts: mgr.contacts.len(),
            sessions: mgr.sessions_len(),
            active_toasts: mgr.toasts.len(),
            discovered_peers: crate::lock_or_recover(&state.discovered).len(),
            config: p2pem_classic::support::DiagnosticsConfig::from(&mgr.config),
        }
    };
    let base_dir = state
        .history_path
        .parent()
        .map(|d| d.join("diagnostics"))
        .ok_or_else(|| "no data directory".to_string())?;
    // The desktop bridge logs to stdout (tracing subscriber); there is no
    // in-app log buffer to bundle, so say so instead of shipping an empty file.
    let logs = "Desktop bridge logs go to stdout (run with `tauri dev` or check the OS console); \
                no in-app log buffer is captured in this bundle.\n";
    let bundle = tokio::task::spawn_blocking(move || {
        p2pem_classic::support::export_diagnostics_bundle(&base_dir, &report, logs)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    Ok(bundle.display().to_string())
}

/// Open the app's data directory (identity, encrypted history, diagnostics)
/// in the system file manager.
#[tauri::command]
pub(crate) fn open_data_dir(state: tauri::State<'_, Bridge>) -> Result<String, String> {
    ensure_ready(&state)?;
    let dir = state
        .history_path
        .parent()
        .ok_or_else(|| "no data directory".to_string())?
        .to_path_buf();
    open::that(&dir).map_err(|e| e.to_string())?;
    Ok(dir.display().to_string())
}

/// The user-facing settings the desktop app exposes. Only fields the core
/// actually honors are surfaced (a toggle the runtime ignores is a lying UI):
/// `download_dir` and typing/notification switches are read by `ChatManager`,
/// and auto-host is implemented by this bridge on unlock.
#[derive(Serialize)]
pub(crate) struct SettingsDto {
    download_dir: String,
    enable_notifications: bool,
    enable_typing_indicators: bool,
    auto_host_on_startup: bool,
    listen_port: u16,
    enable_upnp: bool,
    auto_accept_files: bool,
    auto_connect: bool,
    enable_mdns: bool,
    notification_content: messenger_core::types::NotificationContent,
    /// RFC 3339 timestamp of the last identity backup, or `null` if there has
    /// never been one. Read-only — it is set by `export_identity`, not by the
    /// settings form.
    identity_backed_up_at: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct SettingsUpdate {
    enable_notifications: bool,
    enable_typing_indicators: bool,
    auto_host_on_startup: bool,
    listen_port: u16,
    enable_upnp: bool,
    auto_accept_files: bool,
    auto_connect: bool,
    enable_mdns: bool,
    /// Optional so a client that predates the setting can still save.
    #[serde(default)]
    notification_content: Option<messenger_core::types::NotificationContent>,
}

#[tauri::command]
pub(crate) async fn get_settings(state: tauri::State<'_, Bridge>) -> Result<SettingsDto, String> {
    ensure_ready(&state)?;
    let mgr = state.manager.lock().await;
    Ok(SettingsDto {
        download_dir: mgr.config.download_dir.display().to_string(),
        enable_notifications: mgr.config.enable_notifications,
        enable_typing_indicators: mgr.config.enable_typing_indicators,
        auto_host_on_startup: mgr.config.auto_host_on_startup,
        listen_port: mgr.config.listen_port,
        enable_upnp: mgr.config.enable_upnp,
        auto_accept_files: mgr.config.auto_accept_files,
        auto_connect: mgr.config.auto_connect,
        enable_mdns: mgr.config.enable_mdns,
        notification_content: mgr.config.notification_content,
        identity_backed_up_at: mgr.config.identity_backed_up_at.clone(),
    })
}

/// Apply the toggle/number settings and persist them (config is part of the
/// encrypted history file). The download directory has its own picker command.
#[tauri::command]
pub(crate) async fn update_settings(
    settings: SettingsUpdate,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    if settings.listen_port == 0 {
        return Err("listen port must be between 1 and 65535".to_string());
    }
    {
        let mut mgr = state.manager.lock().await;
        mgr.config.enable_notifications = settings.enable_notifications;
        mgr.config.enable_typing_indicators = settings.enable_typing_indicators;
        mgr.config.auto_host_on_startup = settings.auto_host_on_startup;
        mgr.config.listen_port = settings.listen_port;
        mgr.config.enable_upnp = settings.enable_upnp;
        mgr.config.auto_accept_files = settings.auto_accept_files;
        mgr.config.auto_connect = settings.auto_connect;
        mgr.config.enable_mdns = settings.enable_mdns;
        if let Some(content) = settings.notification_content {
            mgr.config.notification_content = content;
        }
    }
    // Off means off now, not at the next launch: dropping the daemon
    // unregisters any advertisement and stops browsing (TAURI-08), and what it
    // found is forgotten with it.
    if !settings.enable_mdns {
        *crate::lock_or_recover(&state.discovery) = None;
        crate::lock_or_recover(&state.discovered).clear();
    }
    persist_history(&state.manager, &state.history_path, &state.saved_sig).await;
    Ok(())
}

/// Pick a new download directory with the native folder dialog (on a blocking
/// thread) and persist it. Returns the new path, or `None` if cancelled.
#[tauri::command]
pub(crate) async fn pick_download_dir<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: tauri::State<'_, Bridge>,
) -> Result<Option<String>, String> {
    ensure_ready(&state)?;
    let picked = crate::native_file_dialog(
        window,
        crate::Dialog::Folder {
            title: "Choose download folder",
        },
    )
    .await?;
    let Some(dir) = picked else {
        return Ok(None);
    };
    {
        let mut mgr = state.manager.lock().await;
        mgr.config.download_dir = dir.clone();
    }
    persist_history(&state.manager, &state.history_path, &state.saved_sig).await;
    Ok(Some(dir.display().to_string()))
}
