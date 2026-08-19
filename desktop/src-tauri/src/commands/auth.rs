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
    } else if *state.is_new.lock().unwrap() || *state.force_setup.lock().unwrap() {
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
    let key = {
        let mut id = crate::lock_identity(&state.identity);
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
    let key = {
        let mut id = crate::lock_identity(&state.identity);
        // `encrypt` enforces the length floor; the message it returns is
        // already user-facing, so pass it straight through to the screen.
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
    let path = state.identity_save_path();
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
pub(crate) async fn export_identity<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    state: tauri::State<'_, Bridge>,
) -> Result<Option<String>, String> {
    ensure_ready(&state)?;
    let source = state.identity_save_path();
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
    // Record that a backup exists so the app can stop nagging — and, more
    // importantly, so it *can* nag when one never happened.
    {
        let mut mgr = state.manager.lock().await;
        mgr.config.identity_backed_up_at = Some(chrono::Utc::now().to_rfc3339());
    }
    persist_history(&state.manager, &state.history_path, &state.saved_sig).await;
    Ok(Some(dest.display().to_string()))
}

/// Export a support bundle (state metadata + config, never key material) to
/// `<data dir>/diagnostics/bundle-<stamp>/`, mirroring the egui app's export.
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
