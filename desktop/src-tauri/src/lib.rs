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
use messenger_core::types::{
    Config, MessageContent, ToastLevel, TransferDirection, TransferStatus,
};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};
use tokio::sync::Mutex;
use uuid::Uuid;

use p2pem_classic::app::party_manager::{PartyManager, PartyStatus};
use p2pem_classic::app::ChatManager;

/// A pending TOFU fingerprint prompt held for the frontend to poll:
/// `(fingerprint, peer_name, sas, session_id)`.
type PendingFp = Arc<StdMutex<Option<(String, String, String, String)>>>;

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
    /// Pending TOFU fingerprint request, held as queryable state so a missed
    /// `fingerprint-request` event never strands a session awaiting verification.
    pending_fp: PendingFp,
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

mod commands;
use commands::party::{upsert_saved_party, SavedParty};

// ── Entry point ─────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Surface ChatManager / core `tracing` logs to stdout (captured by
    // `tauri dev`). Override with RUST_LOG. Without this the bridge is blind —
    // the egui app installs its own subscriber, so we must too.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,p2pem_classic=debug,messenger_core=debug")
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
            commands::auth::auth_status,
            commands::auth::unlock,
            commands::auth::set_password,
            commands::auth::my_identity,
            commands::chats::list_conversations,
            commands::chats::get_conversation,
            commands::chats::list_transfers,
            commands::chats::cancel_transfer,
            commands::auth::get_settings,
            commands::auth::update_settings,
            commands::auth::pick_download_dir,
            commands::chats::send_message,
            commands::chats::send_file,
            commands::connect::start_host,
            commands::connect::connect_peer,
            commands::connect::host_via_relay,
            commands::connect::connect_via_relay,
            commands::connect::confirm_fingerprint,
            commands::chats::rename_chat,
            commands::chats::delete_chat,
            commands::contacts::list_contacts,
            commands::contacts::my_invite_link,
            commands::contacts::import_invite,
            commands::contacts::connect_contact,
            commands::connect::pending_fingerprint,
            commands::party::party_join,
            commands::party::party_list,
            commands::party::party_history,
            commands::party::party_post,
            commands::party::party_create_channel,
            commands::party::party_send_dm,
            commands::party::party_dm_history,
            commands::party::party_clear_error,
            commands::party::party_send_file,
            commands::party::party_send_file_dm,
            commands::party::party_download_file,
            commands::party::party_saved,
            commands::party::party_leave,
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
    pending_fp: PendingFp,
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
            if let Some(pending) = req {
                let id = pending.session_id.to_string();
                let (fingerprint, peer_name, sas) =
                    (pending.fingerprint, pending.peer_name, pending.sas);
                tracing::info!(peer = %peer_name, session = %id, "TOFU fingerprint verification requested");
                // Persist it as queryable state first, then emit the event.
                *pending_fp.lock().unwrap() = Some((
                    fingerprint.clone(),
                    peer_name.clone(),
                    sas.clone(),
                    id.clone(),
                ));
                let _ = app.emit(
                    "fingerprint-request",
                    serde_json::json!({
                        "fingerprint": fingerprint,
                        "peer_name": peer_name,
                        "sas": sas,
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
