//! Session establishment: hosting, connecting to peers (direct or relay),
//! and TOFU fingerprint confirmation.
use crate::*;

/// Start hosting. `password` (optional) becomes the session-only connection
/// password peers must supply; it is never persisted.
#[tauri::command]
pub(crate) async fn start_host(
    port: u16,
    password: Option<String>,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let (pk, name, fingerprint) = {
        let id = state.identity.lock().unwrap();
        (
            id.private_key().map_err(|e| e.to_string())?,
            id.name.clone(),
            id.fingerprint.clone(),
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
    // Advertise on the LAN so nearby peers can find this host without typing
    // an address. Best-effort: hosting works fine without mDNS.
    if enable_mdns {
        let mut slot = state.discovery.lock().unwrap();
        if slot.is_none() {
            *slot = messenger_core::network::Discovery::new().ok();
        }
        if let Some(d) = slot.as_mut() {
            if let Err(e) = d.register(&name, port, &fingerprint) {
                tracing::warn!("mDNS registration failed: {e}");
            }
        }
    }
    Ok(())
}

/// Dial a peer directly. `password` (optional) is the host's connection
/// password; session-only, never persisted.
#[tauri::command]
pub(crate) async fn connect_peer(
    host: String,
    port: u16,
    password: Option<String>,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let pk = {
        let id = state.identity.lock().unwrap();
        id.private_key().map_err(|e| e.to_string())?
    };
    let mut mgr = state.manager.lock().await;
    mgr.set_connection_password(password.filter(|p| !p.is_empty()));
    mgr.connect_to_host(&host, port, None, pk)
        .await
        .map(|_chat_id| ())
        .map_err(|e| e.to_string())
}

/// Peers discovered on the local network via mDNS. Returns `enabled: false`
/// (and no peers) while the `enable_mdns` setting is off — browsing only runs
/// when the user opted in, since it also reveals presence on the LAN.
#[tauri::command]
pub(crate) async fn list_discovered_peers(
    state: tauri::State<'_, Bridge>,
) -> Result<serde_json::Value, String> {
    ensure_ready(&state)?;
    let enabled = state.manager.lock().await.config.enable_mdns;
    if !enabled {
        return Ok(serde_json::json!({ "enabled": false, "peers": [] }));
    }
    {
        let mut slot = state.discovery.lock().unwrap();
        if slot.is_none() {
            *slot = messenger_core::network::Discovery::new().ok();
        }
        if let Some(d) = slot.as_ref() {
            d.poll(&state.discovered);
        }
    }
    let peers: Vec<serde_json::Value> = state
        .discovered
        .lock()
        .unwrap()
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

/// The addresses a peer can use to reach this host: the LAN address (primary
/// local IPv4 + listening port) and, when UPnP resolved one, the external
/// address. Shown after "Start hosting" so the user knows what to share.
#[tauri::command]
pub(crate) async fn my_addresses(
    state: tauri::State<'_, Bridge>,
) -> Result<serde_json::Value, String> {
    ensure_ready(&state)?;
    let mgr = state.manager.lock().await;
    // The live listener's port, not the settings one — the user may have
    // typed a different port in the Host pane.
    let port = mgr.hosting_port.unwrap_or(mgr.config.listen_port);
    let local = messenger_core::util::primary_local_ipv4()
        .map(|ip| messenger_core::util::format_host_port(&ip, port));
    Ok(serde_json::json!({
        "hosting": mgr.is_hosting,
        "local": local,
        "external": mgr.external_address,
    }))
}

/// Start hosting through a blind relay broker. Returns the connection token the
/// peer needs (alongside the relay address) to dial in — relay is a *transport*,
/// the conversation is still a verified DM.
#[tauri::command]
pub(crate) async fn host_via_relay(
    relay: String,
    state: tauri::State<'_, Bridge>,
) -> Result<String, String> {
    ensure_ready(&state)?;
    let pk = {
        let id = state.identity.lock().unwrap();
        id.private_key().map_err(|e| e.to_string())?
    };
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
    let pk = {
        let id = state.identity.lock().unwrap();
        id.private_key().map_err(|e| e.to_string())?
    };
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
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    let result = state
        .manager
        .lock()
        .await
        .confirm_fingerprint(uuid, accept)
        .map_err(|e| e.to_string());
    // Only clear the pending prompt on success. If confirmation failed, keep it so
    // `pending_fingerprint()` can still surface it and the user can retry.
    if result.is_ok() {
        *state.pending_fp.lock().unwrap() = None;
        // An accepted fingerprint is persisted onto the chat; save so trust survives a restart.
        persist_history(&state.manager, &state.history_path).await;
    }
    result
}

/// The pending TOFU fingerprint prompt, if any. The frontend polls this so a
/// dropped `fingerprint-request` event never leaves a session stuck unverified.
#[tauri::command]
pub(crate) fn pending_fingerprint(state: tauri::State<'_, Bridge>) -> Option<serde_json::Value> {
    if ensure_ready(&state).is_err() {
        return None;
    }
    state
        .pending_fp
        .lock()
        .unwrap()
        .clone()
        .map(|(fingerprint, peer_name, chat_id)| {
            serde_json::json!({
                "fingerprint": fingerprint,
                "peer_name": peer_name,
                "chat_id": chat_id,
            })
        })
}
