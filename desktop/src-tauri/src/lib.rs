//! P2PEM desktop bridge (Phase 1).
//!
//! A thin Tauri layer over the existing, UI-agnostic `ChatManager` from the
//! client crate. It mirrors what the egui `App` does — owns an `Identity` plus
//! an `Arc<Mutex<ChatManager>>`, shares a single tokio runtime with Tauri, and
//! runs a background poll loop that drains `SessionEvent`s and notifies the
//! webview. The frontend (static `dist/`) talks to it over the commands below.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use messenger_core::identity::Identity;
use messenger_core::types::{Config, MessageContent, ToastLevel, TransferStatus};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};
use tokio::sync::Mutex;
use uuid::Uuid;

use encodeur_rsa_rust::app::party_manager::{PartyManager, PartyStatus};
use encodeur_rsa_rust::app::ChatManager;

/// Shared application state managed by Tauri.
struct Bridge {
    manager: Arc<Mutex<ChatManager>>,
    /// Client-side Party/Community server connections (channels, members, DMs).
    /// State is ephemeral and re-fetched on join — the server holds durable history.
    party: Arc<Mutex<PartyManager>>,
    identity: StdMutex<Identity>,
    history_path: PathBuf,
    /// Identity file (sibling of the history file).
    identity_path: PathBuf,
    /// Saved communities (`parties.json`, sibling of the history file): address,
    /// username, and the pinned server fingerprint, so joined communities survive
    /// a restart (one-click rejoin) and a changed server identity is detected.
    parties_path: PathBuf,
    /// A brand-new identity with no password yet.
    is_new: StdMutex<bool>,
    /// Plaintext key present (no password set) — force a set-password step.
    force_setup: StdMutex<bool>,
    /// Pending TOFU fingerprint request `(fingerprint, peer_name, session_id)`,
    /// held as queryable state so a missed `fingerprint-request` event never
    /// strands a session waiting for verification.
    pending_fp: Arc<StdMutex<Option<(String, String, String)>>>,
}

impl Bridge {
    fn identity_save_path(&self) -> PathBuf {
        self.identity_path.clone()
    }
}

/// Best-effort encrypted history save, mirroring what the egui app does on a
/// timer. A no-op while the identity is still locked (no history key) or when
/// there is nothing to persist. Errors are logged, never propagated — a
/// transient disk failure must not break a live command.
///
/// Without this the desktop app never wrote history back: `unlock` loaded it,
/// but every message sent or received in a session was lost on restart.
async fn persist_history(manager: &Arc<Mutex<ChatManager>>, path: &Path) {
    // `history_snapshot()` fails only when the identity is still locked (no
    // history key); in that state there is nothing to persist. An unlocked but
    // empty history IS saved on purpose, so deleting the last chat sticks.
    let snapshot = manager.lock().await.history_snapshot();
    let Ok((history, key)) = snapshot else {
        return;
    };
    let path = path.to_path_buf();
    match tokio::task::spawn_blocking(move || history.save_encrypted(&path, &key)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!("desktop history save failed: {e}"),
        Err(e) => tracing::warn!("desktop history save task join failed: {e}"),
    }
}

/// A cheap signature of the persisted-state surface (chat count + per-chat
/// message count and title length). Used by the poll loop to save only when
/// something actually changed, so received messages are persisted without
/// rewriting the encrypted history to disk on every idle tick.
fn state_signature(mgr: &ChatManager) -> u64 {
    let ids = mgr.chat_ids();
    let mut sig = ids.len() as u64;
    for id in &ids {
        if let Some(c) = mgr.get_chat(*id) {
            sig = sig
                .wrapping_mul(1_000_003)
                .wrapping_add(c.messages.len() as u64)
                .wrapping_add(c.title.len() as u64);
        }
    }
    sig
}

// ── DTOs sent to the webview ────────────────────────────────────────────────

#[derive(Serialize)]
struct AuthStatus {
    state: &'static str,
    name: String,
    fingerprint: String,
}

#[derive(Serialize)]
struct ConvSummary {
    id: String,
    title: String,
    last: Option<String>,
    connected: bool,
    placeholder: bool,
    kind: &'static str,
    transport: &'static str,
    /// True once the peer's fingerprint has been confirmed (TOFU-verified). The
    /// UI must not claim "verified" for conversations that are still pending.
    verified: bool,
    /// Total message count; the frontend derives unread badges from it (count
    /// beyond what was on screen when the conversation was last open).
    messages: usize,
}

fn kind_str(k: messenger_core::types::ChatKind) -> &'static str {
    use messenger_core::types::ChatKind::*;
    match k {
        Dm => "dm",
        Group => "group",
        Channel => "channel",
    }
}

fn transport_str(t: messenger_core::types::Transport) -> &'static str {
    use messenger_core::types::Transport::*;
    match t {
        Direct => "direct",
        Relay => "relay",
        Server => "server",
    }
}

/// Reject any post-auth command while the identity is still locked or a password
/// setup is pending. The React shell already hides the UI in those states, but
/// the bridge must enforce the barrier itself so no command can mutate state or
/// start a session before unlock/set-password completes.
fn ensure_ready(state: &Bridge) -> Result<(), String> {
    if state.identity.lock().unwrap().is_locked() {
        return Err("Unlock required".to_string());
    }
    if *state.is_new.lock().unwrap() || *state.force_setup.lock().unwrap() {
        return Err("Password setup required".to_string());
    }
    Ok(())
}

// ── Commands ────────────────────────────────────────────────────────────────

#[tauri::command]
fn auth_status(state: tauri::State<'_, Bridge>) -> AuthStatus {
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
async fn unlock(password: String, state: tauri::State<'_, Bridge>) -> Result<(), String> {
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
async fn set_password(password: String, state: tauri::State<'_, Bridge>) -> Result<(), String> {
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
fn my_identity(state: tauri::State<'_, Bridge>) -> AuthStatus {
    auth_status(state)
}

/// The user-facing settings the desktop app exposes. Only fields the core
/// actually honors are surfaced (a toggle the runtime ignores is a lying UI):
/// `download_dir` and typing/notification switches are read by `ChatManager`,
/// and auto-host is implemented by this bridge on unlock.
#[derive(Serialize)]
struct SettingsDto {
    download_dir: String,
    enable_notifications: bool,
    enable_typing_indicators: bool,
    auto_host_on_startup: bool,
    listen_port: u16,
}

#[derive(Deserialize)]
struct SettingsUpdate {
    enable_notifications: bool,
    enable_typing_indicators: bool,
    auto_host_on_startup: bool,
    listen_port: u16,
}

#[tauri::command]
async fn get_settings(state: tauri::State<'_, Bridge>) -> Result<SettingsDto, String> {
    ensure_ready(&state)?;
    let mgr = state.manager.lock().await;
    Ok(SettingsDto {
        download_dir: mgr.config.download_dir.display().to_string(),
        enable_notifications: mgr.config.enable_notifications,
        enable_typing_indicators: mgr.config.enable_typing_indicators,
        auto_host_on_startup: mgr.config.auto_host_on_startup,
        listen_port: mgr.config.listen_port,
    })
}

/// Apply the toggle/number settings and persist them (config is part of the
/// encrypted history file). The download directory has its own picker command.
#[tauri::command]
async fn update_settings(
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
    }
    persist_history(&state.manager, &state.history_path).await;
    Ok(())
}

/// Pick a new download directory with the native folder dialog (on a blocking
/// thread) and persist it. Returns the new path, or `None` if cancelled.
#[tauri::command]
async fn pick_download_dir(state: tauri::State<'_, Bridge>) -> Result<Option<String>, String> {
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

#[tauri::command]
async fn list_conversations(state: tauri::State<'_, Bridge>) -> Result<Vec<ConvSummary>, String> {
    ensure_ready(&state)?;
    let mgr = state.manager.lock().await;
    let mut out = Vec::new();
    for id in mgr.chat_ids() {
        if let Some(chat) = mgr.get_chat(id) {
            let last = chat.messages.last().map(|m| match &m.content {
                MessageContent::Text { text } => text.clone(),
                MessageContent::File { filename, .. } => format!("📎 {}", filename),
                MessageContent::Edited { new_text } => new_text.clone(),
            });
            out.push(ConvSummary {
                id: id.to_string(),
                title: chat.title.clone(),
                last,
                connected: mgr.is_connected(&id),
                placeholder: chat.is_host_placeholder,
                kind: kind_str(chat.kind),
                transport: transport_str(chat.transport),
                verified: chat.peer_fingerprint.is_some(),
                messages: chat.messages.len(),
            });
        }
    }
    Ok(out)
}

/// A live file transfer, for progress display in the chat pane.
#[derive(Serialize)]
struct TransferDto {
    id: String,
    chat_id: String,
    filename: String,
    size: u64,
    received: u64,
    status: &'static str,
    /// Failure reason when `status == "failed"`.
    error: Option<String>,
}

fn transfer_status_parts(s: &TransferStatus) -> (&'static str, Option<String>) {
    match s {
        TransferStatus::Pending => ("pending", None),
        TransferStatus::InProgress => ("active", None),
        TransferStatus::Completed => ("done", None),
        TransferStatus::Failed(e) => ("failed", Some(e.clone())),
        TransferStatus::Cancelled => ("cancelled", None),
    }
}

/// The active file transfers (both directions), polled by the frontend on the
/// same `state-updated` cadence as the conversation list, so large sends and
/// receives show live progress instead of nothing.
#[tauri::command]
async fn list_transfers(state: tauri::State<'_, Bridge>) -> Result<Vec<TransferDto>, String> {
    ensure_ready(&state)?;
    let mgr = state.manager.lock().await;
    Ok(mgr
        .active_transfers_snapshot()
        .into_iter()
        .map(|t| {
            let (status, error) = transfer_status_parts(&t.status);
            TransferDto {
                id: t.id.to_string(),
                chat_id: t.chat_id.to_string(),
                filename: t.filename,
                size: t.size,
                received: t.received,
                status,
                error,
            }
        })
        .collect())
}

/// Return the full conversation (with messages) as JSON for the chat pane.
#[tauri::command]
async fn get_conversation(
    id: String,
    state: tauri::State<'_, Bridge>,
) -> Result<serde_json::Value, String> {
    ensure_ready(&state)?;
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    let mgr = state.manager.lock().await;
    match mgr.get_chat(uuid) {
        Some(chat) => serde_json::to_value(chat).map_err(|e| e.to_string()),
        None => Err("No such conversation".to_string()),
    }
}

#[tauri::command]
async fn send_message(
    id: String,
    text: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    state
        .manager
        .lock()
        .await
        .send_message(uuid, text)
        .map_err(|e| e.to_string())?;
    persist_history(&state.manager, &state.history_path).await;
    Ok(())
}

/// Pick a file with the native dialog and send it over the given conversation.
/// The picker runs on a blocking thread so it never stalls the async runtime;
/// a cancelled dialog is a successful no-op.
#[tauri::command]
async fn send_file(id: String, state: tauri::State<'_, Bridge>) -> Result<(), String> {
    ensure_ready(&state)?;
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    let picked = tokio::task::spawn_blocking(|| rfd::FileDialog::new().pick_file())
        .await
        .map_err(|e| e.to_string())?;
    let Some(path) = picked else {
        return Ok(()); // user cancelled
    };
    state
        .manager
        .lock()
        .await
        .send_file(uuid, path)
        .await
        .map_err(|e| e.to_string())?;
    persist_history(&state.manager, &state.history_path).await;
    Ok(())
}

#[tauri::command]
async fn start_host(port: u16, state: tauri::State<'_, Bridge>) -> Result<(), String> {
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
async fn connect_peer(
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
async fn host_via_relay(relay: String, state: tauri::State<'_, Bridge>) -> Result<String, String> {
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
async fn connect_via_relay(
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
async fn confirm_fingerprint(
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
fn pending_fingerprint(state: tauri::State<'_, Bridge>) -> Option<serde_json::Value> {
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

#[tauri::command]
async fn rename_chat(
    id: String,
    title: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    state
        .manager
        .lock()
        .await
        .rename_chat(uuid, title)
        .map_err(|e| e.to_string())?;
    persist_history(&state.manager, &state.history_path).await;
    Ok(())
}

#[tauri::command]
async fn delete_chat(id: String, state: tauri::State<'_, Bridge>) -> Result<(), String> {
    ensure_ready(&state)?;
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    state.manager.lock().await.delete_chat(uuid);
    persist_history(&state.manager, &state.history_path).await;
    Ok(())
}

// ── Contacts + invite links ─────────────────────────────────────────────────

#[derive(Serialize)]
struct ContactDto {
    id: String,
    name: String,
    fingerprint: Option<String>,
    address: Option<String>,
    trust: &'static str,
}

fn trust_str(t: &messenger_core::types::TrustState) -> &'static str {
    use messenger_core::types::TrustState::*;
    match t {
        Unverified => "unverified",
        Verified => "verified",
        Trusted => "trusted",
        Blocked => "blocked",
    }
}

fn contact_dto(c: &messenger_core::types::Contact) -> ContactDto {
    ContactDto {
        id: c.id.to_string(),
        name: c.name.clone(),
        fingerprint: c.fingerprint.clone(),
        address: c.address.clone(),
        trust: trust_str(&c.trust_state),
    }
}

#[tauri::command]
async fn list_contacts(state: tauri::State<'_, Bridge>) -> Result<Vec<ContactDto>, String> {
    ensure_ready(&state)?;
    let mgr = state.manager.lock().await;
    Ok(mgr.contacts.values().map(contact_dto).collect())
}

/// The current user's signed invite link (address derived from the local IP +
/// the configured listen port, when resolvable).
#[tauri::command]
async fn my_invite_link(state: tauri::State<'_, Bridge>) -> Result<String, String> {
    ensure_ready(&state)?;
    let address = {
        let mgr = state.manager.lock().await;
        let port = mgr.config.listen_port;
        messenger_core::util::primary_local_ipv4()
            .map(|ip| messenger_core::util::format_host_port(&ip, port))
    };
    let id = state.identity.lock().unwrap();
    id.generate_signed_invite_link(address)
        .map_err(|e| e.to_string())
}

/// Parse an invite link and store it as a contact.
#[tauri::command]
async fn import_invite(
    link: String,
    state: tauri::State<'_, Bridge>,
) -> Result<ContactDto, String> {
    ensure_ready(&state)?;
    let contact = {
        let mgr = state.manager.lock().await;
        mgr.parse_invite_link(&link).map_err(|e| e.to_string())?
    };
    let mut mgr = state.manager.lock().await;
    let id = mgr.import_contact(contact);
    Ok(contact_dto(mgr.get_contact(id).expect("just inserted")))
}

/// Dial a stored contact by its saved address.
#[tauri::command]
async fn connect_contact(id: String, state: tauri::State<'_, Bridge>) -> Result<(), String> {
    ensure_ready(&state)?;
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    let address = {
        let mgr = state.manager.lock().await;
        mgr.get_contact(uuid).and_then(|c| c.address.clone())
    };
    let address = address.ok_or_else(|| "Contact has no saved address".to_string())?;
    let (host, port) =
        messenger_core::util::parse_host_port(&address, Some(messenger_core::PORT_DEFAULT))
            .map_err(|e| e.to_string())?;
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

// ── Communities (Party servers) ─────────────────────────────────────────────
//
// A thin bridge over `PartyManager`, mirroring the egui Party tab. Command
// params are single words (`server`, `channel`, …) to avoid the Tauri 2
// arg-naming footgun where a snake_case param silently no-ops.

#[derive(Serialize)]
struct PartyMemberDto {
    id: String,
    username: String,
    online: bool,
    is_me: bool,
}

#[derive(Serialize)]
struct PartyChannelDto {
    id: String,
    name: String,
}

#[derive(Serialize)]
struct PartyServerDto {
    id: String,
    name: String,
    address: String,
    /// The username this client joined with (for rejoin flows).
    username: String,
    fingerprint: String,
    status: &'static str,
    status_detail: Option<String>,
    member_id: Option<String>,
    channels: Vec<PartyChannelDto>,
    members: Vec<PartyMemberDto>,
    last_error: Option<String>,
}

#[derive(Serialize)]
struct PartyMessageDto {
    sender_name: String,
    from_me: bool,
    kind: &'static str,
    text: String,
    size: Option<u64>,
    /// Content hash of a file message, used to request its download. `None` for text.
    hash: Option<String>,
    timestamp: u64,
}

fn party_status_parts(status: &PartyStatus) -> (&'static str, Option<String>) {
    match status {
        PartyStatus::Connecting => ("connecting", None),
        PartyStatus::Joined => ("joined", None),
        PartyStatus::Rejected(reason) => ("rejected", Some(reason.clone())),
        PartyStatus::Disconnected => ("disconnected", None),
    }
}

/// Serialize one connection's directory (channels + members), resolving "you".
fn server_dto(
    id: Uuid,
    conn: &encodeur_rsa_rust::app::party_manager::PartyServerConn,
) -> PartyServerDto {
    let (status, status_detail) = party_status_parts(&conn.status);
    let members = conn
        .members
        .iter()
        .map(|m| PartyMemberDto {
            id: m.id.to_string(),
            username: m.username.clone(),
            online: m.online,
            is_me: conn.member_id == Some(m.id),
        })
        .collect();
    let channels = conn
        .channels
        .iter()
        .map(|c| PartyChannelDto {
            id: c.id.to_string(),
            name: c.name.clone(),
        })
        .collect();
    PartyServerDto {
        id: id.to_string(),
        name: conn.server_name.clone(),
        address: conn.address.clone(),
        username: conn.username.clone(),
        fingerprint: conn.server_fingerprint.clone(),
        status,
        status_detail,
        member_id: conn.member_id.map(|m| m.to_string()),
        channels,
        members,
        last_error: conn.last_error.clone(),
    }
}

/// Turn a stored envelope into a display DTO, resolving the sender's username
/// from the member directory and flagging the local user's own messages.
fn message_dto(
    env: &messenger_core::party::Envelope,
    conn: &encodeur_rsa_rust::app::party_manager::PartyServerConn,
) -> PartyMessageDto {
    use messenger_core::party::MessagePayload;
    let sender_name = conn
        .members
        .iter()
        .find(|m| m.id == env.sender)
        .map(|m| m.username.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let (kind, text, size, hash) = match &env.payload {
        MessagePayload::Text(t) => ("text", t.clone(), None, None),
        MessagePayload::File(f) => ("file", f.name.clone(), Some(f.size), Some(f.hash.clone())),
    };
    PartyMessageDto {
        sender_name,
        from_me: conn.member_id == Some(env.sender),
        kind,
        text,
        size,
        hash,
        timestamp: env.timestamp,
    }
}

/// Connect to a community server, verify (TOFU) its fingerprint out of band, and
/// join with a username. Returns the local server id.
#[tauri::command]
async fn party_join(
    address: String,
    username: String,
    password: String,
    state: tauri::State<'_, Bridge>,
) -> Result<String, String> {
    ensure_ready(&state)?;
    let address = address.trim().to_string();
    let username = username.trim().to_string();
    let pk = {
        let id = state.identity.lock().unwrap();
        id.private_key().map_err(|e| e.to_string())?
    };
    let password = Some(password).filter(|p| !p.trim().is_empty());
    let mut party = state.party.lock().await;
    let sid = party
        .connect_and_join(&address, &username, password, &pk)
        .await
        .map_err(|e| e.to_string())?;

    // TOFU pinning: if we've joined this address before, its identity must not
    // have changed. A mismatch is either a redeployed server or an active MITM —
    // refuse and tell the user, never silently trust the new key.
    let fingerprint = party
        .server(sid)
        .map(|c| c.server_fingerprint.clone())
        .unwrap_or_default();
    let saved = load_saved_parties(&state.parties_path);
    if let Some(pinned) = saved
        .iter()
        .find(|p| p.address.eq_ignore_ascii_case(&address) && !p.fingerprint.is_empty())
    {
        if pinned.fingerprint != fingerprint {
            party.remove_server(sid);
            return Err(format!(
                "SECURITY: this server's identity changed since you last joined \
                 (expected {}…, got {}…). If the operator redeployed the server this \
                 may be expected — leave the saved community and rejoin to trust the \
                 new identity. Otherwise, do not proceed.",
                &pinned.fingerprint[..16.min(pinned.fingerprint.len())],
                &fingerprint[..16.min(fingerprint.len())]
            ));
        }
    }
    // Rejoin replaces: drop any older entry for the same address (e.g. a
    // disconnected or join-rejected zombie) so the list never shows duplicates.
    let stale: Vec<Uuid> = party
        .server_ids()
        .into_iter()
        .filter(|id| {
            *id != sid
                && party
                    .server(*id)
                    .is_some_and(|c| c.address.eq_ignore_ascii_case(&address))
        })
        .collect();
    for id in stale {
        party.remove_server(id);
    }
    drop(party);

    upsert_saved_party(
        &state.parties_path,
        SavedParty {
            address,
            username,
            name: String::new(),
            fingerprint,
        },
    );
    Ok(sid.to_string())
}

/// The joined community servers with their channels and member directories.
#[tauri::command]
async fn party_list(state: tauri::State<'_, Bridge>) -> Result<Vec<PartyServerDto>, String> {
    ensure_ready(&state)?;
    let party = state.party.lock().await;
    let mut out: Vec<PartyServerDto> = party
        .server_ids()
        .into_iter()
        .filter_map(|id| party.server(id).map(|conn| server_dto(id, conn)))
        .collect();
    // Stable order so the UI's server list doesn't reshuffle each poll.
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    Ok(out)
}

/// Messages in a channel (already seeded from durable history on join; live
/// posts arrive via the poll loop).
#[tauri::command]
async fn party_history(
    server: String,
    channel: String,
    state: tauri::State<'_, Bridge>,
) -> Result<Vec<PartyMessageDto>, String> {
    ensure_ready(&state)?;
    let sid = Uuid::parse_str(&server).map_err(|e| e.to_string())?;
    let cid = Uuid::parse_str(&channel).map_err(|e| e.to_string())?;
    let party = state.party.lock().await;
    let conn = party
        .server(sid)
        .ok_or_else(|| "unknown server".to_string())?;
    let msgs = conn
        .messages
        .get(&cid)
        .map(|v| v.iter().map(|e| message_dto(e, conn)).collect())
        .unwrap_or_default();
    Ok(msgs)
}

/// Post a text message to a channel.
#[tauri::command]
async fn party_post(
    server: String,
    channel: String,
    text: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = Uuid::parse_str(&server).map_err(|e| e.to_string())?;
    let cid = Uuid::parse_str(&channel).map_err(|e| e.to_string())?;
    state
        .party
        .lock()
        .await
        .post(sid, cid, text)
        .map_err(|e| e.to_string())
}

/// Create a new channel on a community server.
#[tauri::command]
async fn party_create_channel(
    server: String,
    name: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = Uuid::parse_str(&server).map_err(|e| e.to_string())?;
    state
        .party
        .lock()
        .await
        .create_channel(sid, name)
        .map_err(|e| e.to_string())
}

/// Send a direct message to another member of a community server.
#[tauri::command]
async fn party_send_dm(
    server: String,
    to: String,
    text: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = Uuid::parse_str(&server).map_err(|e| e.to_string())?;
    let to = Uuid::parse_str(&to).map_err(|e| e.to_string())?;
    state
        .party
        .lock()
        .await
        .send_dm(sid, to, text)
        .map_err(|e| e.to_string())
}

/// The DM thread with a member. Requests a fresh fetch (offline catch-up); the
/// authoritative history lands on a later poll.
#[tauri::command]
async fn party_dm_history(
    server: String,
    peer: String,
    state: tauri::State<'_, Bridge>,
) -> Result<Vec<PartyMessageDto>, String> {
    ensure_ready(&state)?;
    let sid = Uuid::parse_str(&server).map_err(|e| e.to_string())?;
    let peer = Uuid::parse_str(&peer).map_err(|e| e.to_string())?;
    let party = state.party.lock().await;
    let _ = party.fetch_dm_history(sid, peer);
    let conn = party
        .server(sid)
        .ok_or_else(|| "unknown server".to_string())?;
    Ok(party
        .dm_messages(sid, peer)
        .iter()
        .map(|e| message_dto(e, conn))
        .collect())
}

/// Clear a server's last surfaced error (e.g. a rejected post) after the UI has
/// shown it.
#[tauri::command]
async fn party_clear_error(server: String, state: tauri::State<'_, Bridge>) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = Uuid::parse_str(&server).map_err(|e| e.to_string())?;
    state.party.lock().await.clear_server_error(sid);
    Ok(())
}

/// A community this client has joined, persisted (plaintext, no secrets — the
/// password is never stored) so it survives restarts: the UI offers one-click
/// rejoin, and the pinned server fingerprint turns the first join's TOFU into a
/// real trust anchor — a different identity at the same address is refused.
#[derive(Clone, Serialize, Deserialize)]
struct SavedParty {
    address: String,
    username: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    fingerprint: String,
}

/// Load the saved-communities list; missing or unreadable files are an empty list
/// (never an error — this is convenience state, not critical data).
fn load_saved_parties(path: &Path) -> Vec<SavedParty> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Best-effort save of the saved-communities list; errors are logged, not fatal.
fn save_saved_parties(path: &Path, list: &[SavedParty]) {
    match serde_json::to_string_pretty(list) {
        Ok(json) => {
            if let Err(e) = std::fs::write(path, json) {
                tracing::warn!("saving communities list failed: {e}");
            }
        }
        Err(e) => tracing::warn!("serializing communities list failed: {e}"),
    }
}

/// Insert or update the entry for `address` (matched case-insensitively).
fn upsert_saved_party(path: &Path, entry: SavedParty) {
    let mut list = load_saved_parties(path);
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

/// The saved communities, for the join screen's one-click rejoin list.
#[tauri::command]
async fn party_saved(state: tauri::State<'_, Bridge>) -> Result<Vec<SavedParty>, String> {
    ensure_ready(&state)?;
    Ok(load_saved_parties(&state.parties_path))
}

/// Leave a community: drop the connection, forget its local state, and remove it
/// from the saved list. The server keeps the membership, so rejoining with the
/// same identity later resumes it.
#[tauri::command]
async fn party_leave(server: String, state: tauri::State<'_, Bridge>) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = Uuid::parse_str(&server).map_err(|e| e.to_string())?;
    let mut party = state.party.lock().await;
    let address = party.server(sid).map(|c| c.address.clone());
    if !party.remove_server(sid) {
        return Err("unknown server".to_string());
    }
    drop(party);
    if let Some(addr) = address {
        let mut list = load_saved_parties(&state.parties_path);
        list.retain(|p| !p.address.eq_ignore_ascii_case(&addr));
        save_saved_parties(&state.parties_path, &list);
    }
    Ok(())
}

/// Open the native file picker (on a blocking thread) and read the chosen file's
/// bytes, returning `(name, mime, data)`. `Ok(None)` if the user cancelled.
async fn pick_upload() -> Result<Option<(String, String, Vec<u8>)>, String> {
    let picked = tokio::task::spawn_blocking(|| rfd::FileDialog::new().pick_file())
        .await
        .map_err(|e| e.to_string())?;
    let Some(path) = picked else {
        return Ok(None);
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let mime = guess_mime(&path);
    let data = std::fs::read(&path).map_err(|e| e.to_string())?;
    Ok(Some((name, mime, data)))
}

/// A minimal extension → MIME guess for common file types; defaults to
/// `application/octet-stream`. The server keeps this only as display metadata.
fn guess_mime(path: &std::path::Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" | "log" | "md" => "text/plain",
        "json" => "application/json",
        "zip" => "application/zip",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Pick a file and post it to a community channel. The picker runs on a blocking
/// thread; a cancelled dialog is a successful no-op.
#[tauri::command]
async fn party_send_file(
    server: String,
    channel: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = Uuid::parse_str(&server).map_err(|e| e.to_string())?;
    let cid = Uuid::parse_str(&channel).map_err(|e| e.to_string())?;
    let Some((name, mime, data)) = pick_upload().await? else {
        return Ok(());
    };
    state
        .party
        .lock()
        .await
        .send_file(sid, cid, name, mime, data)
        .map_err(|e| e.to_string())
}

/// Pick a file and send it as a direct message to another community member.
#[tauri::command]
async fn party_send_file_dm(
    server: String,
    to: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = Uuid::parse_str(&server).map_err(|e| e.to_string())?;
    let peer = Uuid::parse_str(&to).map_err(|e| e.to_string())?;
    let Some((name, mime, data)) = pick_upload().await? else {
        return Ok(());
    };
    state
        .party
        .lock()
        .await
        .send_file_dm(sid, peer, name, mime, data)
        .map_err(|e| e.to_string())
}

/// Download a community file by content hash and save it via the native dialog.
/// `name` seeds the save dialog. The server only returns bytes the caller is
/// allowed to see (access-checked there). A cancelled save dialog is a no-op.
#[tauri::command]
async fn party_download_file(
    server: String,
    hash: String,
    name: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = Uuid::parse_str(&server).map_err(|e| e.to_string())?;
    // Register the download and drop the party lock BEFORE awaiting, so the poll
    // loop can lock the manager, drain the FileData response, and complete us.
    let rx = state
        .party
        .lock()
        .await
        .request_download(sid, hash)
        .map_err(|e| e.to_string())?;
    // Outer error: the sender was dropped (connection torn down). Inner error: the
    // server refused (file gone / not permitted). Either way, surface a message
    // rather than hang.
    let data = match rx.await {
        Ok(Ok(data)) => data,
        Ok(Err(reason)) => return Err(reason),
        Err(_) => return Err("download failed: the connection closed".to_string()),
    };
    let suggested = name;
    let picked = tokio::task::spawn_blocking(move || {
        rfd::FileDialog::new().set_file_name(suggested).save_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(path) = picked else {
        return Ok(()); // user cancelled the save dialog
    };
    std::fs::write(&path, &data).map_err(|e| e.to_string())
}

// ── Entry point ─────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Surface ChatManager / core `tracing` logs to stdout (captured by
    // `tauri dev`). Override with RUST_LOG. Without this the bridge is blind —
    // the egui app installs its own subscriber, so we must too.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new(
                    "info,encodeur_rsa_rust=debug,messenger_core=debug",
                )
            }),
        )
        .try_init();

    // One shared tokio runtime for both the ChatManager's spawned tasks and
    // Tauri's async commands. `tauri::async_runtime::set` makes Tauri schedule
    // async command handlers (and our poll loop) onto this runtime, so the
    // bare `tokio::spawn` calls inside ChatManager find a runtime context.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");
    tauri::async_runtime::set(rt.handle().clone());
    // Keep the runtime alive for the lifetime of the process.
    Box::leak(Box::new(rt));

    let bridge = init_bridge();

    tauri::Builder::default()
        .manage(bridge)
        .invoke_handler(tauri::generate_handler![
            auth_status,
            unlock,
            set_password,
            my_identity,
            list_conversations,
            get_conversation,
            list_transfers,
            get_settings,
            update_settings,
            pick_download_dir,
            send_message,
            send_file,
            start_host,
            connect_peer,
            host_via_relay,
            connect_via_relay,
            confirm_fingerprint,
            rename_chat,
            delete_chat,
            list_contacts,
            my_invite_link,
            import_invite,
            connect_contact,
            pending_fingerprint,
            party_join,
            party_list,
            party_history,
            party_post,
            party_create_channel,
            party_send_dm,
            party_dm_history,
            party_clear_error,
            party_send_file,
            party_send_file_dm,
            party_download_file,
            party_saved,
            party_leave,
        ])
        .setup(|app| {
            let b = app.state::<Bridge>();
            spawn_poll_loop(
                app.handle().clone(),
                b.manager.clone(),
                b.party.clone(),
                b.history_path.clone(),
                b.parties_path.clone(),
                b.pending_fp.clone(),
            );
            Ok(())
        })
        .on_window_event(|window, event| {
            // Flush history synchronously on close so the final session state
            // (last messages, deletions) is never lost between poll-loop ticks.
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                let bridge = window.state::<Bridge>();
                let manager = bridge.manager.clone();
                let path = bridge.history_path.clone();
                tauri::async_runtime::block_on(persist_history(&manager, &path));
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running P2PEM");
}

/// Build the identity + manager, mirroring `gui::App::new`'s data-dir setup.
fn init_bridge() -> Bridge {
    // The data dir holds `identity.json` (this peer's key/fingerprint) and the
    // encrypted history. It uses the desktop app's OWN identifier ("P2PEM"), NOT
    // the egui app's ("EncryptedMessenger"): sharing one dir made both apps load
    // the same identity, so they were the same P2P peer and could not connect to
    // each other (a connection between them was a self-connection). With distinct
    // dirs the two apps are distinct peers and connect normally.
    //
    // `P2PEM_DATA_DIR` overrides it, so extra instances can each run with their
    // own identity/history — handy for testing several peers on one machine:
    //   P2PEM_DATA_DIR=%LOCALAPPDATA%\p2pem-test cargo tauri dev
    let data_dir: Option<PathBuf> = std::env::var_os("P2PEM_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            directories::ProjectDirs::from("com", "chat-p2p", "P2PEM")
                .map(|dirs| dirs.data_dir().to_path_buf())
        });

    let (history_path, identity, is_new) = if let Some(data) = data_dir {
        std::fs::create_dir_all(&data).ok();
        tracing::info!(data_dir = %data.display(), "using data directory");
        let (id, isnew) = Identity::get_or_create(&data, "User").unwrap_or_else(|e| {
            tracing::error!("identity load/create failed: {e}; falling back to plaintext");
            (
                Identity::new_with_plaintext("User".to_string()).expect("identity"),
                true,
            )
        });
        (data.join("history.json.enc"), id, isnew)
    } else {
        tracing::warn!("could not resolve a data directory; using a relative fallback");
        (
            PathBuf::from("history.json.enc"),
            Identity::new_with_plaintext("User".to_string()).expect("identity"),
            true,
        )
    };

    let identity_path = history_path.with_file_name("identity.json");
    let parties_path = history_path.with_file_name("parties.json");
    // Plaintext key in hand (no password) ⇒ force a set-password step, like egui.
    let force_setup = !identity.is_locked();

    let manager = Arc::new(Mutex::new(ChatManager::new(Config::default())));

    Bridge {
        manager,
        party: Arc::new(Mutex::new(PartyManager::new())),
        identity: StdMutex::new(identity),
        history_path,
        identity_path,
        parties_path,
        is_new: StdMutex::new(is_new),
        force_setup: StdMutex::new(force_setup),
        pending_fp: Arc::new(StdMutex::new(None)),
    }
}

fn toast_level_str(l: ToastLevel) -> &'static str {
    match l {
        ToastLevel::Success => "success",
        ToastLevel::Error => "error",
        ToastLevel::Warning | ToastLevel::Info => "info",
    }
}

/// Periodically drain network events and notify the webview to refresh.
fn spawn_poll_loop(
    app: tauri::AppHandle,
    manager: Arc<Mutex<ChatManager>>,
    party: Arc<Mutex<PartyManager>>,
    history_path: PathBuf,
    parties_path: PathBuf,
    pending_fp: Arc<StdMutex<Option<(String, String, String)>>>,
) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        // Seeded from the first observed state so an unchanged session never
        // triggers a spurious write; only real changes (notably peer messages
        // arriving via poll_session_events) are persisted.
        let mut last_saved_sig: Option<u64> = None;
        // Servers already recorded as Joined, so the saved-communities file is
        // touched once per join (to fill in the server's display name), not every tick.
        let mut joined_recorded: std::collections::HashSet<Uuid> = Default::default();
        loop {
            interval.tick().await;
            // Drain Party/Community server events (joins, channel/member updates,
            // incoming messages) so the webview's Communities pane stays live.
            {
                let mut p = party.lock().await;
                p.poll_events();
                // Once a server completes its join, copy its (now known) display
                // name into the saved-communities entry for a nicer rejoin card.
                for sid in p.server_ids() {
                    if joined_recorded.contains(&sid) {
                        continue;
                    }
                    if let Some(conn) = p.server(sid) {
                        if conn.status == PartyStatus::Joined && !conn.server_name.is_empty() {
                            joined_recorded.insert(sid);
                            upsert_saved_party(
                                &parties_path,
                                SavedParty {
                                    address: conn.address.clone(),
                                    username: conn.username.clone(),
                                    name: conn.server_name.clone(),
                                    fingerprint: conn.server_fingerprint.clone(),
                                },
                            );
                        }
                    }
                }
            }
            let (req, toasts, sig) = {
                let mut m = manager.lock().await;
                m.poll_session_events();
                // Drain ChatManager's internal toasts (Connected, errors,
                // "connection refused", …) so the webview can surface them.
                let toasts: Vec<(&'static str, String)> = std::mem::take(&mut m.toasts)
                    .into_iter()
                    .map(|t| (toast_level_str(t.level), t.message))
                    .collect();
                let req = m.fingerprint_verification_request.take();
                let sig = state_signature(&m);
                (req, toasts, sig)
            };
            // Persist when the conversation surface changed (e.g. a received
            // message). User-initiated commands save themselves immediately.
            if last_saved_sig != Some(sig) {
                persist_history(&manager, &history_path).await;
                last_saved_sig = Some(sig);
            }
            for (level, message) in toasts {
                let _ = app.emit(
                    "toast",
                    serde_json::json!({ "level": level, "message": message }),
                );
            }
            if let Some((fingerprint, peer_name, chat_id)) = req {
                let id = chat_id.to_string();
                tracing::info!(peer = %peer_name, session = %id, "TOFU fingerprint verification requested");
                // Persist it as queryable state first, then emit the event.
                *pending_fp.lock().unwrap() =
                    Some((fingerprint.clone(), peer_name.clone(), id.clone()));
                let _ = app.emit(
                    "fingerprint-request",
                    serde_json::json!({
                        "fingerprint": fingerprint,
                        "peer_name": peer_name,
                        "chat_id": id,
                    }),
                );
            }
            let _ = app.emit("state-updated", ());
            // Nudge the Communities pane to re-read the party directory + messages.
            let _ = app.emit("party-updated", ());
        }
    });
}
