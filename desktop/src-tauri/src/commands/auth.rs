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

/// Unlock an existing, password-protected identity and load history.
#[tauri::command]
pub(crate) async fn unlock(
    password: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_identity_loaded(&state)?;
    let key = {
        let mut id = lock_or_recover(&state.identity);
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
    let key = {
        let mut id = lock_or_recover(&state.identity);
        // `encrypt` enforces the length floor with a user-facing message.
        id.encrypt(&password).map_err(|e| e.to_string())?;
        id.save(&state.identity_path).map_err(|e| e.to_string())?;
        // Stay unlocked for this session.
        id.decrypt(&password).map_err(|e| e.to_string())?;
        let key = id.history_key().map_err(|e| e.to_string())?;
        state.password_setup_done();
        key
    };
    // Load existing history like `unlock`, or the next save could overwrite it
    // with empty state.
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
#[tauri::command]
pub(crate) fn set_display_name(
    name: String,
    state: tauri::State<'_, Bridge>,
) -> Result<AuthStatus, String> {
    ensure_ready(&state)?;
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
    let picked = crate::native_file_dialog(window, |d| {
        d.set_title("Save identity backup")
            .set_file_name("p2pem-identity-backup.json")
            .save_file()
    })
    .await?;
    let Some(dest) = picked else {
        return Ok(None); // cancelled
    };
    tokio::fs::copy(&source, &dest)
        .await
        .map_err(|e| format!("Backup failed: {e}"))?;
    // Record the backup so the app can stop (or start) nagging.
    {
        let mut mgr = state.manager.lock().await;
        mgr.config.identity_backed_up_at = Some(chrono::Utc::now().to_rfc3339());
    }
    persist_history(&state.manager, &state.history_path, &state.saved_sig).await;
    Ok(Some(dest.display().to_string()))
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
    let picked = crate::native_file_dialog(window, |d| {
        d.set_title("Choose download folder").pick_folder()
    })
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
