//! Session establishment: hosting, connecting to peers (direct or relay),
//! and TOFU fingerprint confirmation.
use crate::*;

#[tauri::command]
pub(crate) async fn start_host(port: u16, state: tauri::State<'_, Bridge>) -> Result<(), String> {
    ensure_ready(&state)?;
    let pk = {
        let id = state.identity.lock().unwrap();
        id.private_key().map_err(|e| e.to_string())?
    };
    state
        .manager
        .lock()
        .await
        .start_host(port, pk)
        .await
        .map(|_chat_id| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn connect_peer(
    host: String,
    port: u16,
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
        .connect_to_host(&host, port, None, pk)
        .await
        .map(|_chat_id| ())
        .map_err(|e| e.to_string())
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
