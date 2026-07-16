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

/// Honor `auto_host_on_startup` once the identity is unlocked and history (with
/// its persisted config) is loaded — "open the app and be reachable", matching
/// the egui/TUI apps. Failures are logged, never fatal to the unlock.
async fn auto_host_if_configured(state: &tauri::State<'_, Bridge>, mgr: &mut ChatManager) {
    if !mgr.config.auto_host_on_startup {
        return;
    }
    let port = mgr.config.listen_port;
    let pk = { state.identity.lock().unwrap().private_key() };
    match pk {
        Ok(pk) => {
            if let Err(e) = mgr.start_host(port, pk).await {
                tracing::warn!("auto-host on startup failed: {e}");
            } else {
                tracing::info!(port, "auto-hosting on startup");
            }
        }
        Err(e) => tracing::warn!("auto-host: no private key: {e}"),
    }
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
}

#[derive(Deserialize)]
pub(crate) struct SettingsUpdate {
    enable_notifications: bool,
    enable_typing_indicators: bool,
    auto_host_on_startup: bool,
    listen_port: u16,
    enable_upnp: bool,
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
