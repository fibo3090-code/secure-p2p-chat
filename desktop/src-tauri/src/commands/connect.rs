//! Session establishment: hosting, connecting to peers (direct or relay),
//! and TOFU fingerprint confirmation.
use crate::*;

/// Start hosting. `password` (optional) is the session-only connection
/// password peers must supply; never persisted.
#[tauri::command]
pub(crate) async fn start_host(
    port: u16,
    password: Option<String>,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    // mDNS advertises reachability only, never identity (no fingerprint).
    let (pk, name) = {
        let id = lock_or_recover(&state.identity);
        (
            id.private_key().map_err(|e| e.to_string())?,
            id.name.clone(),
        )
    };
    let enable_mdns = {
        let mut mgr = state.manager.lock().await;
        mgr.set_connection_password(password.filter(|p| !p.is_empty()));
        mgr.start_host(port, pk)
            .await
            .map(|_chat_id| ())
            .map_err(|e| e.to_string())?;
        mgr.config.enable_mdns
    };
    // Best-effort LAN advertisement.
    if enable_mdns {
        if let Some(d) = discovery(&state).as_mut() {
            if let Err(e) = d.register(&name, port) {
                tracing::warn!("mDNS registration failed: {e}");
            }
        }
    }
    Ok(())
}

/// The mDNS handle, created on first use.
fn discovery(state: &Bridge) -> MutexGuard<'_, Option<Discovery>> {
    let mut slot = lock_or_recover(&state.discovery);
    if slot.is_none() {
        *slot = Discovery::new().ok();
    }
    slot
}

/// Dial a peer directly. `password` (optional) is the host's session-only
/// connection password.
#[tauri::command]
pub(crate) async fn connect_peer(
    host: String,
    port: u16,
    password: Option<String>,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let pk = lock_or_recover(&state.identity)
        .private_key()
        .map_err(|e| e.to_string())?;
    let mut mgr = state.manager.lock().await;
    mgr.set_connection_password(password.filter(|p| !p.is_empty()));
    mgr.connect_to_host(&host, port, None, pk)
        .await
        .map(|_chat_id| ())
        .map_err(|e| e.to_string())
}

/// Peers discovered on the LAN via mDNS. Browsing runs only while the user has
/// opted in (`enable_mdns`), since it reveals presence; otherwise `enabled: false`.
#[tauri::command]
pub(crate) async fn list_discovered_peers(
    state: tauri::State<'_, Bridge>,
) -> Result<serde_json::Value, String> {
    ensure_ready(&state)?;
    let enabled = state.manager.lock().await.config.enable_mdns;
    if !enabled {
        return Ok(serde_json::json!({ "enabled": false, "peers": [] }));
    }
    if let Some(d) = discovery(&state).as_ref() {
        d.poll(&state.discovered);
    }
    let peers: Vec<serde_json::Value> = lock_or_recover(&state.discovered)
        .iter()
        .map(|p| {
            serde_json::json!({
                "name": p.name,
                "address": p.address,
                "port": p.port,
                "fingerprint": p.fingerprint,
            })
        })
        .collect();
    Ok(serde_json::json!({ "enabled": true, "peers": peers }))
}

/// Whether the conversation is locked (no new peer may connect; auto-rehost
/// paused), like the terminal client's `:lock`.
#[tauri::command]
pub(crate) async fn lock_state(state: tauri::State<'_, Bridge>) -> Result<bool, String> {
    ensure_ready(&state)?;
    Ok(state.manager.lock().await.is_conversation_locked())
}

/// Lock or unlock the conversation. Locking also stops the listener (existing
/// sessions keep running).
#[tauri::command]
pub(crate) async fn set_locked(
    locked: bool,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    {
        let mut mgr = state.manager.lock().await;
        mgr.set_conversation_locked(locked);
        if locked {
            mgr.stop_hosting();
        }
    }
    // A locked user must also stop advertising on the LAN.
    if locked {
        if let Some(d) = lock_or_recover(&state.discovery).as_mut() {
            if let Err(e) = d.unregister() {
                tracing::warn!("mDNS unregister on lock failed: {e}");
            }
        }
    }
    Ok(())
}

/// The addresses a peer can use to reach this host: LAN (primary IPv4 + port)
/// and, when UPnP resolved one, external.
#[tauri::command]
pub(crate) async fn my_addresses(
    state: tauri::State<'_, Bridge>,
) -> Result<serde_json::Value, String> {
    ensure_ready(&state)?;
    let mgr = state.manager.lock().await;
    // The live listener's port, not the settings one.
    let port = mgr.hosting_port.unwrap_or(mgr.config.listen_port);
    let local = messenger_core::util::primary_local_ipv4()
        .map(|ip| messenger_core::util::format_host_port(&ip, port));
    Ok(serde_json::json!({
        "hosting": mgr.is_hosting,
        "local": local,
        "external": mgr.external_address,
    }))
}

/// Host through a blind relay. Returns the token the peer needs to dial in.
#[tauri::command]
pub(crate) async fn host_via_relay(
    relay: String,
    state: tauri::State<'_, Bridge>,
) -> Result<String, String> {
    ensure_ready(&state)?;
    let pk = lock_or_recover(&state.identity)
        .private_key()
        .map_err(|e| e.to_string())?;
    state
        .manager
        .lock()
        .await
        .start_host_via_relay(&relay, None, pk)
        .await
        .map(|(_chat_id, token)| token)
        .map_err(|e| e.to_string())
}

/// Dial a peer through a relay using the relay address + the token they shared.
#[tauri::command]
pub(crate) async fn connect_via_relay(
    relay: String,
    token: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let pk = lock_or_recover(&state.identity)
        .private_key()
        .map_err(|e| e.to_string())?;
    state
        .manager
        .lock()
        .await
        .connect_via_relay(&relay, &token, None, pk)
        .await
        .map(|_chat_id| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn confirm_fingerprint(
    id: String,
    accept: bool,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let uuid = parse_id(&id)?;
    let result = {
        let mut mgr = state.manager.lock().await;
        if accept {
            mgr.confirm_fingerprint(uuid, true)
        } else {
            // Ends the session; discards the chat only if this connection created it.
            mgr.reject_fingerprint(uuid)
        }
    }
    .map_err(|e| e.to_string());
    // Keep the prompt on failure so the user can retry.
    if result.is_ok() {
        *lock_or_recover(&state.pending_fp) = None;
        // The accepted fingerprint is on the chat now; save so trust survives a restart.
        persist_history(&state.manager, &state.history_path, &state.saved_sig).await;
    }
    result
}

/// The pending TOFU prompt, polled so a dropped `fingerprint-request` event
/// never leaves a session stuck unverified.
#[tauri::command]
pub(crate) fn pending_fingerprint(state: tauri::State<'_, Bridge>) -> Option<serde_json::Value> {
    if ensure_ready(&state).is_err() {
        return None;
    }
    lock_or_recover(&state.pending_fp)
        .as_ref()
        .map(fingerprint_prompt_json)
}
