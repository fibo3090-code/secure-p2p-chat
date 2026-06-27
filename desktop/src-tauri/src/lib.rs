//! P2PEM desktop bridge (Phase 1).
//!
//! A thin Tauri layer over the existing, UI-agnostic `ChatManager` from the
//! client crate. It mirrors what the egui `App` does — owns an `Identity` plus
//! an `Arc<Mutex<ChatManager>>`, shares a single tokio runtime with Tauri, and
//! runs a background poll loop that drains `SessionEvent`s and notifies the
//! webview. The frontend (static `dist/`) talks to it over the commands below.

use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use messenger_core::identity::Identity;
use messenger_core::types::{Config, MessageContent, ToastLevel};
use serde::Serialize;
use tauri::{Emitter, Manager};
use tokio::sync::Mutex;
use uuid::Uuid;

use encodeur_rsa_rust::app::ChatManager;

/// Shared application state managed by Tauri.
struct Bridge {
    manager: Arc<Mutex<ChatManager>>,
    identity: StdMutex<Identity>,
    history_path: PathBuf,
    /// Identity file (sibling of the history file).
    identity_path: PathBuf,
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
        id.decrypt(&password).map_err(|_| "Wrong password".to_string())?;
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
    Ok(())
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
    state.manager.lock().await.set_history_key(key);
    Ok(())
}

#[tauri::command]
fn my_identity(state: tauri::State<'_, Bridge>) -> AuthStatus {
    auth_status(state)
}

#[tauri::command]
async fn list_conversations(
    state: tauri::State<'_, Bridge>,
) -> Result<Vec<ConvSummary>, String> {
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
            });
        }
    }
    Ok(out)
}

/// Return the full conversation (with messages) as JSON for the chat pane.
#[tauri::command]
async fn get_conversation(
    id: String,
    state: tauri::State<'_, Bridge>,
) -> Result<serde_json::Value, String> {
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
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    state
        .manager
        .lock()
        .await
        .send_message(uuid, text)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn start_host(port: u16, state: tauri::State<'_, Bridge>) -> Result<(), String> {
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
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    let result = state
        .manager
        .lock()
        .await
        .confirm_fingerprint(uuid, accept)
        .map_err(|e| e.to_string());
    // Resolved either way — clear the pending prompt so the UI doesn't re-show it.
    *state.pending_fp.lock().unwrap() = None;
    result
}

/// The pending TOFU fingerprint prompt, if any. The frontend polls this so a
/// dropped `fingerprint-request` event never leaves a session stuck unverified.
#[tauri::command]
fn pending_fingerprint(state: tauri::State<'_, Bridge>) -> Option<serde_json::Value> {
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
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    state
        .manager
        .lock()
        .await
        .rename_chat(uuid, title)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn delete_chat(id: String, state: tauri::State<'_, Bridge>) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    state.manager.lock().await.delete_chat(uuid);
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
    let mgr = state.manager.lock().await;
    Ok(mgr.contacts.values().map(contact_dto).collect())
}

/// The current user's signed invite link (address derived from the local IP +
/// the configured listen port, when resolvable).
#[tauri::command]
async fn my_invite_link(state: tauri::State<'_, Bridge>) -> Result<String, String> {
    let address = {
        let mgr = state.manager.lock().await;
        let port = mgr.config.listen_port;
        messenger_core::util::primary_local_ipv4()
            .map(|ip| messenger_core::util::format_host_port(&ip, port))
    };
    let id = state.identity.lock().unwrap();
    id.generate_signed_invite_link(address).map_err(|e| e.to_string())
}

/// Parse an invite link and store it as a contact.
#[tauri::command]
async fn import_invite(link: String, state: tauri::State<'_, Bridge>) -> Result<ContactDto, String> {
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
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    let address = {
        let mgr = state.manager.lock().await;
        mgr.get_contact(uuid).and_then(|c| c.address.clone())
    };
    let address = address.ok_or_else(|| "Contact has no saved address".to_string())?;
    let (host, port) = messenger_core::util::parse_host_port(&address, Some(messenger_core::PORT_DEFAULT))
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
            send_message,
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
        ])
        .setup(|app| {
            let b = app.state::<Bridge>();
            spawn_poll_loop(app.handle().clone(), b.manager.clone(), b.pending_fp.clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running P2PEM");
}

/// Build the identity + manager, mirroring `gui::App::new`'s data-dir setup.
fn init_bridge() -> Bridge {
    let proj = directories::ProjectDirs::from("com", "chat-p2p", "EncryptedMessenger");
    let (history_path, identity, is_new) = if let Some(dirs) = proj {
        let data = dirs.data_dir();
        std::fs::create_dir_all(data).ok();
        let (id, isnew) = Identity::get_or_create(data, "User").unwrap_or_else(|e| {
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
    // Plaintext key in hand (no password) ⇒ force a set-password step, like egui.
    let force_setup = !identity.is_locked();

    let manager = Arc::new(Mutex::new(ChatManager::new(Config::default())));

    Bridge {
        manager,
        identity: StdMutex::new(identity),
        history_path,
        identity_path,
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
    pending_fp: Arc<StdMutex<Option<(String, String, String)>>>,
) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        loop {
            interval.tick().await;
            let (req, toasts) = {
                let mut m = manager.lock().await;
                m.poll_session_events();
                // Drain ChatManager's internal toasts (Connected, errors,
                // "connection refused", …) so the webview can surface them.
                let toasts: Vec<(&'static str, String)> = std::mem::take(&mut m.toasts)
                    .into_iter()
                    .map(|t| (toast_level_str(t.level), t.message))
                    .collect();
                let req = m.fingerprint_verification_request.take();
                (req, toasts)
            };
            for (level, message) in toasts {
                let _ = app.emit("toast", serde_json::json!({ "level": level, "message": message }));
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
        }
    });
}
