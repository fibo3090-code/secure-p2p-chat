//! Identity lifecycle (unlock, set-password, status) and user settings.
use crate::*;

#[derive(Serialize)]
pub(crate) struct AuthStatus {
    state: &'static str,
    name: String,
    fingerprint: String,
    /// The core's password floor, so the set-password screen validates
    /// against the real rule.
    min_password_len: usize,
    /// Only when `state == "error"`: the startup failure, for the user.
    error: Option<String>,
}

#[tauri::command]
pub(crate) fn auth_status(state: tauri::State<'_, Bridge>) -> AuthStatus {
    // A startup failure must never present as "set_password", which would
    // invite creating a new identity over the unreadable one.
    if let Some(err) = &state.init_error {
        return AuthStatus {
            state: "error",
            name: String::new(),
            fingerprint: String::new(),
            min_password_len: messenger_core::MIN_PASSWORD_LEN,
            error: Some(err.clone()),
        };
    }
    let id = lock_or_recover(&state.identity);
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

/// Guard for the auth commands, which are not behind `ensure_ready`.
/// `set_password` here would overwrite the unreadable identity file.
fn ensure_identity_loaded(state: &Bridge) -> Result<(), String> {
    match &state.init_error {
        Some(err) => Err(err.clone()),
        None => Ok(()),
    }
}

/// Serialises the commands that run Argon2 over the identity (`unlock`,
/// `set_password`, `change_password`). They derive on a *copy*, off the
/// identity mutex (TAURI-07: holding it for the ~1 s of Argon2 froze every
/// command that reads the identity, and the poll loop behind them), so without
/// this two of them could each finish against the same starting state — two
/// setups writing different passwords, or two unlocks both reloading history.
static AUTH_OP: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// What to tell the user when `decrypt` fails: only a wrong password asks for
/// the password again; anything else is a damaged file (TAURI-20).
fn unlock_error(e: &anyhow::Error) -> String {
    if e.is::<messenger_core::identity::WrongPassword>() {
        "Wrong password".to_string()
    } else {
        format!(
            "Your identity file is damaged and cannot be opened ({e}). Restore it from a \
             backup (Settings → Identity backup) — retyping the password will not help."
        )
    }
}

/// Unlock an existing, password-protected identity and load history.
#[tauri::command]
pub(crate) async fn unlock(
    password: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_identity_loaded(&state)?;
    let _auth = AUTH_OP.lock().await;
    // Already unlocked: nothing to do. Running the body again re-ran Argon2
    // and reloaded the history file over the live state — `apply_history`
    // replaces, so everything not yet saved (saves are coalesced) was dropped
    // while its sessions stayed open. A double-submitted form was enough.
    if !crate::lock_identity(&state.identity).is_locked() && !state.needs_password_setup() {
        return Ok(());
    }
    let candidate = crate::lock_identity(&state.identity).clone();
    let (candidate, key) = tokio::task::spawn_blocking(move || {
        let mut candidate = candidate;
        candidate.decrypt(&password).map_err(|e| unlock_error(&e))?;
        let key = candidate.history_key().map_err(|e| e.to_string())?;
        Ok::<_, String>((candidate, key))
    })
    .await
    .map_err(|e| e.to_string())??;
    *crate::lock_identity(&state.identity) = candidate;
    state.password_setup_done();
    let mut mgr = state.manager.lock().await;
    mgr.set_history_key(key);
    if let Err(e) = mgr.load_history_auto(&state.history_path, &key) {
        tracing::warn!("Failed to load history after unlock: {}", e);
    }
    auto_host_if_configured(&state, &mut mgr).await;
    Ok(())
}

/// Honor `auto_host_on_startup` and `auto_connect` after unlock; failures are
/// logged, never fatal.
async fn auto_host_if_configured(state: &tauri::State<'_, Bridge>, mgr: &mut ChatManager) {
    let pk_result = lock_or_recover(&state.identity).private_key();
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
    let _auth = AUTH_OP.lock().await;
    if !state.needs_password_setup() {
        return Err("A password is already set — use Change password instead".to_string());
    }
    let path = state.identity_path.clone();
    let candidate = crate::lock_identity(&state.identity).clone();
    let (candidate, key) = tokio::task::spawn_blocking(move || {
        let mut id = candidate;
        // `protect` is `encrypt` (which enforces the length floor, with a
        // user-facing message to pass straight to the screen) keeping the key
        // in memory, so this session stays unlocked on one Argon2 run, not two.
        id.protect(&password).map_err(|e| e.to_string())?;
        id.save(&path).map_err(|e| e.to_string())?;
        let key = id.history_key().map_err(|e| e.to_string())?;
        Ok::<_, String>((id, key))
    })
    .await
    .map_err(|e| e.to_string())??;
    *crate::lock_identity(&state.identity) = candidate;
    state.password_setup_done();
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

/// Change the password protecting the identity file. The private key (and so
/// the history key and fingerprint) is unchanged. The re-wrap runs on a copy
/// swapped in only once the new file is on disk, so a failed write cannot
/// leave memory and disk wanting different passwords.
#[tauri::command]
pub(crate) async fn change_password(
    current: String,
    new: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let _auth = AUTH_OP.lock().await;
    let path = state.identity_path.clone();
    // Argon2 runs twice (~1s of CPU): keep it off the async runtime.
    let mut candidate = lock_or_recover(&state.identity).clone();
    let candidate = tokio::task::spawn_blocking(move || -> Result<Identity, String> {
        candidate
            .change_password(&current, &new)
            .map_err(|e| e.to_string())?;
        candidate.save(&path).map_err(|e| e.to_string())?;
        Ok(candidate)
    })
    .await
    .map_err(|e| e.to_string())??;
    *lock_or_recover(&state.identity) = candidate;
    Ok(())
}

#[tauri::command]
pub(crate) fn my_identity(state: tauri::State<'_, Bridge>) -> AuthStatus {
    auth_status(state)
}

/// Change the identity's display name (used in invite links and the UI) and
/// persist it. The key material and fingerprint are untouched.
///
/// Takes `AUTH_OP` like the password commands: `change_password` re-wraps a
/// copy and swaps it in at the end, so a rename landing in between was put
/// back, in memory and on disk (TAURI-17).
#[tauri::command]
pub(crate) async fn set_display_name(
    name: String,
    state: tauri::State<'_, Bridge>,
) -> Result<AuthStatus, String> {
    ensure_ready(&state)?;
    let _auth = AUTH_OP.lock().await;
    {
        let mut id = lock_or_recover(&state.identity);
        id.set_name(&name).map_err(|e| e.to_string())?;
        id.save(&state.identity_path).map_err(|e| e.to_string())?;
    }
    Ok(auth_status(state))
}

#[derive(serde::Serialize)]
pub(crate) struct HistoryStatus {
    /// `Some(reason)` when the file exists but could not be read; saving is
    /// blocked until [`set_aside_history`].
    error: Option<String>,
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

/// Delete everything this app stores on this device, then quit (audit
/// JOURNEY-03 / DOC-02). Guarded by the password (checked against the file on
/// disk) and the typed word `DELETE`. `ChatManager::delete_all_data` refuses
/// every later save, so the close flush cannot write history back.
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
    *lock_or_recover(&state.discovery) = None;
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

/// Copy the (already encrypted) identity file to a user-chosen location.
/// Returns the destination, or None if cancelled.
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

/// Export a support bundle (metadata + config, never key material) to
/// `<data dir>/diagnostics/bundle-<stamp>/`. Returns the bundle directory.
#[tauri::command]
pub(crate) async fn export_diagnostics(state: tauri::State<'_, Bridge>) -> Result<String, String> {
    ensure_ready(&state)?;
    let (identity_locked, identity_name, fp_prefix) = {
        let id = lock_or_recover(&state.identity);
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
            discovered_peers: lock_or_recover(&state.discovered).len(),
            config: p2pem_classic::support::DiagnosticsConfig::from(&mgr.config),
        }
    };
    let base_dir = state
        .history_path
        .parent()
        .map(|d| d.join("diagnostics"))
        .ok_or_else(|| "no data directory".to_string())?;
    // The recent log, from the in-process buffer: a release build has no
    // console to read it from (QUAL-07). `export_diagnostics_bundle` scrubs it.
    let logs = p2pem_classic::support::format_event_logs(crate::app_logs());
    let bundle = tokio::task::spawn_blocking(move || {
        p2pem_classic::support::export_diagnostics_bundle(&base_dir, &report, &logs)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    Ok(bundle.display().to_string())
}

/// Open the app's data directory in the system file manager.
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

/// The settings the desktop app exposes; only fields the core actually honors.
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
    /// RFC 3339 time of the last identity backup; read-only (set by `export_identity`).
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

/// Apply the toggle/number settings and persist them (config lives in the
/// encrypted history). The download directory has its own picker.
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

/// Pick and persist a new download directory. `None` if cancelled.
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
