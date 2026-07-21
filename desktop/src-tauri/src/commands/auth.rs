//! Identity lifecycle (unlock, set-password, status) and user settings.
use crate::*;

#[derive(Serialize)]
pub(crate) struct AuthStatus {
    state: &'static str,
    name: String,
    fingerprint: String,
}

#[tauri::command]
pub(crate) fn auth_status(state: tauri::State<'_, Bridge>) -> AuthStatus {
    let id = state.identity.lock().unwrap();
    let st = if id.is_locked() {
        "unlock"
    } else if *state.is_new.lock().unwrap() || *state.force_setup.lock().unwrap() {
        "set_password"
    } else {
        "ready"
    };
    AuthStatus {
        state: st,
        name: id.name.clone(),
        fingerprint: id.fingerprint.clone(),
    }
}

/// Unlock an existing, password-protected identity and load history.
#[tauri::command]
pub(crate) async fn unlock(
    password: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    let key = {
        let mut id = state.identity.lock().unwrap();
        id.decrypt(&password)
            .map_err(|_| "Wrong password".to_string())?;
        let key = id.history_key().map_err(|e| e.to_string())?;
        *state.is_new.lock().unwrap() = false;
        *state.force_setup.lock().unwrap() = false;
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
/// and be reachable / reconnected", matching the egui/TUI apps. Failures are
/// logged, never fatal to the unlock.
async fn auto_host_if_configured(state: &tauri::State<'_, Bridge>, mgr: &mut ChatManager) {
    let pk_result = state.identity.lock().unwrap().private_key();
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
    let key = {
        let mut id = state.identity.lock().unwrap();
        id.encrypt(&password).map_err(|e| e.to_string())?;
        id.save(&state.identity_save_path())
            .map_err(|e| e.to_string())?;
        // Decrypt in memory so we remain unlocked for this session.
        id.decrypt(&password).map_err(|e| e.to_string())?;
        let key = id.history_key().map_err(|e| e.to_string())?;
        *state.is_new.lock().unwrap() = false;
        *state.force_setup.lock().unwrap() = false;
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
        let mut id = state.identity.lock().unwrap();
        id.set_name(&name).map_err(|e| e.to_string())?;
        id.save(&state.identity_save_path())
            .map_err(|e| e.to_string())?;
    }
    Ok(auth_status(state))
}

/// Export a backup copy of the encrypted identity file (identity.json) to a
/// user-chosen location. The file is already encrypted at rest (Argon2 +
/// ChaCha20-Poly1305), so the copy is as safe as the original — but it IS the
/// identity: losing it (and the password) means losing the account.
/// Returns the destination path, or None if the dialog was cancelled.
#[tauri::command]
pub(crate) async fn export_identity(
    state: tauri::State<'_, Bridge>,
) -> Result<Option<String>, String> {
    ensure_ready(&state)?;
    let source = state.identity_save_path();
    if !source.exists() {
        return Err("No identity file to back up yet".to_string());
    }
    let picked = tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_file_name("p2pem-identity-backup.json")
            .save_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(dest) = picked else {
        return Ok(None); // cancelled
    };
    tokio::fs::copy(&source, &dest)
        .await
        .map_err(|e| format!("Backup failed: {e}"))?;
    Ok(Some(dest.display().to_string()))
}

/// Export a support bundle (state metadata + config, never key material) to
/// `<data dir>/diagnostics/bundle-<stamp>/`, mirroring the egui app's export.
/// Returns the bundle directory path.
#[tauri::command]
pub(crate) async fn export_diagnostics(state: tauri::State<'_, Bridge>) -> Result<String, String> {
    ensure_ready(&state)?;
    let (identity_locked, identity_name, fp_prefix) = {
        let id = state.identity.lock().unwrap();
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
            discovered_peers: state.discovered.lock().unwrap().len(),
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
    }
    persist_history(&state.manager, &state.history_path).await;
    Ok(())
}

/// Pick a new download directory with the native folder dialog (on a blocking
/// thread) and persist it. Returns the new path, or `None` if cancelled.
#[tauri::command]
pub(crate) async fn pick_download_dir(
    state: tauri::State<'_, Bridge>,
) -> Result<Option<String>, String> {
    ensure_ready(&state)?;
    let picked = tokio::task::spawn_blocking(|| rfd::FileDialog::new().pick_folder())
        .await
        .map_err(|e| e.to_string())?;
    let Some(dir) = picked else {
        return Ok(None);
    };
    {
        let mut mgr = state.manager.lock().await;
        mgr.config.download_dir = dir.clone();
    }
    persist_history(&state.manager, &state.history_path).await;
    Ok(Some(dir.display().to_string()))
}
