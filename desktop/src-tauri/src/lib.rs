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
use messenger_core::network::{DiscoveredPeer, Discovery};
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
    /// Shared with the poll loop, which needs the private key for auto-rehost.
    identity: Arc<StdMutex<Identity>>,
    /// mDNS browse/advertise handle, created lazily when `enable_mdns` is on.
    discovery: Arc<StdMutex<Option<Discovery>>>,
    /// Peers found on the local network (mDNS), refreshed on demand.
    discovered: Arc<StdMutex<Vec<DiscoveredPeer>>>,
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
    /// Set when startup found an identity file it could not read. The app boots
    /// into a dead state that only reports this, rather than generating a
    /// replacement identity and quietly abandoning the user's history.
    init_error: Option<String>,
    /// Shared with the poll loop so a command's save and the loop's save do not
    /// both write the same bytes. See [`SavedSig`].
    saved_sig: SavedSig,
}

impl Bridge {
    fn identity_save_path(&self) -> PathBuf {
        self.identity_path.clone()
    }
}

/// Lock the identity mutex, recovering from poisoning instead of panicking.
///
/// `Mutex::lock().unwrap()` turns one panic anywhere in the process into a
/// permanent, cascading failure: every later access panics too, and in the poll
/// loop that kills event processing entirely — the app looks frozen rather than
/// reporting anything. The `Identity` behind this lock is only ever read or
/// wholly replaced, so a poisoned lock does not imply torn state; carrying on is
/// strictly better than dying.
fn lock_identity(identity: &StdMutex<Identity>) -> std::sync::MutexGuard<'_, Identity> {
    identity.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("identity mutex was poisoned by an earlier panic; recovering");
        poisoned.into_inner()
    })
}

/// Signature of the state as of the last successful save, shared between the
/// poll loop and the commands. Both write the *whole* encrypted history, so
/// without a shared marker every user action cost two full rewrites: the command
/// saved, then the next poll tick saw a signature it had not recorded and saved
/// the identical bytes again. That write is O(total history) and fsynced.
type SavedSig = Arc<StdMutex<Option<u64>>>;

/// Best-effort encrypted history save. A no-op while the identity is still
/// locked (no history key) or when there is nothing to persist. Errors are
/// logged, never propagated — a transient disk failure must not break a live
/// command.
///
/// Without this the desktop app never wrote history back: `unlock` loaded it,
/// but every message sent or received in a session was lost on restart.
///
/// On success the signature of exactly what was written is recorded in
/// `saved_sig`. It is captured under the same lock as the snapshot, so it
/// describes the bytes that actually went to disk even if state changes while
/// the encrypt+write runs off-thread. A failed save deliberately leaves the
/// marker alone, so the poll loop retries.
async fn persist_history(manager: &Arc<Mutex<ChatManager>>, path: &Path, saved_sig: &SavedSig) {
    // `history_snapshot()` fails only when the identity is still locked (no
    // history key); in that state there is nothing to persist. An unlocked but
    // empty history IS saved on purpose, so deleting the last chat sticks.
    let (snapshot, sig) = {
        let m = manager.lock().await;
        (m.history_snapshot(), state_signature(&m))
    };
    let Ok((history, key)) = snapshot else {
        return;
    };
    let path = path.to_path_buf();
    match tokio::task::spawn_blocking(move || history.save_encrypted(&path, &key)).await {
        Ok(Ok(())) => {
            *saved_sig.lock().unwrap_or_else(|e| e.into_inner()) = Some(sig);
        }
        Ok(Err(e)) => tracing::warn!("desktop history save failed: {e}"),
        Err(e) => tracing::warn!("desktop history save task join failed: {e}"),
    }
}

/// A cheap signature of the persisted-state surface (chat count + per-chat
/// message count, delivered count, and title length). Used by the poll loop to
/// save only when something actually changed, so received messages — and
/// delivery receipts, which flip `Message.delivered` without changing any
/// count — are persisted without rewriting the encrypted history on every
/// idle tick.
fn state_signature(mgr: &ChatManager) -> u64 {
    let ids = mgr.chat_ids();
    let mut sig = ids.len() as u64;
    for id in &ids {
        if let Some(c) = mgr.get_chat(*id) {
            let delivered = c.messages.iter().filter(|m| m.delivered).count() as u64;
            sig = sig
                .wrapping_mul(1_000_003)
                .wrapping_add(c.messages.len() as u64)
                .wrapping_add(delivered.wrapping_mul(65_537))
                // The read mark is persisted state and drives the unread badge,
                // so a change to it must both save and refresh the UI.
                .wrapping_add((c.read_count as u64).wrapping_mul(31))
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
    // A broken identity blocks everything: any command that ran here would
    // operate on the throwaway identity created just to render the error.
    if let Some(err) = &state.init_error {
        return Err(err.clone());
    }
    if lock_identity(&state.identity).is_locked() {
        return Err("Unlock required".to_string());
    }
    if *state.is_new.lock().unwrap() || *state.force_setup.lock().unwrap() {
        return Err("Password setup required".to_string());
    }
    Ok(())
}

mod commands;
use commands::party::{upsert_saved_party, SavedParty};

// Not on Windows — see the dev-dependencies note in Cargo.toml.
#[cfg(all(test, not(windows)))]
mod tests;

/// The full command registration, shared by the real app and the test harness
/// so a command can never be reachable in tests but unregistered in the app
/// (or vice versa).
fn invoke_handler<R: tauri::Runtime>() -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync {
    tauri::generate_handler![
        commands::auth::auth_status,
        commands::auth::unlock,
        commands::auth::set_password,
        commands::auth::my_identity,
        commands::auth::set_display_name,
        commands::auth::export_identity,
        commands::auth::export_diagnostics,
        commands::auth::open_data_dir,
        commands::connect::lock_state,
        commands::connect::set_locked,
        commands::chats::list_conversations,
        commands::chats::get_conversation,
        commands::chats::mark_read,
        commands::chats::set_presence,
        commands::chats::list_transfers,
        commands::chats::accept_transfer,
        commands::chats::decline_transfer,
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
        commands::chats::open_file,
        commands::chats::file_preview,
        commands::chats::open_url,
        commands::contacts::list_contacts,
        commands::contacts::remove_contact,
        commands::contacts::block_contact,
        commands::contacts::unblock_contact,
        commands::contacts::my_invite_link,
        commands::contacts::import_invite,
        commands::contacts::connect_contact,
        commands::connect::pending_fingerprint,
        commands::connect::list_discovered_peers,
        commands::connect::my_addresses,
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
    ]
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
        .invoke_handler(invoke_handler())
        .setup(|app| {
            let b = app.state::<Bridge>();
            spawn_poll_loop(app.handle().clone(), PollContext::from_bridge(&b));
            Ok(())
        })
        .on_window_event(|window, event| {
            // Flush history synchronously on close so the final session state
            // (last messages, deletions) is never lost between poll-loop ticks.
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                let bridge = window.state::<Bridge>();
                let manager = bridge.manager.clone();
                let path = bridge.history_path.clone();
                let sig = bridge.saved_sig.clone();
                tauri::async_runtime::block_on(persist_history(&manager, &path, &sig));
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

    // A failure to load an *existing* identity must never be papered over by
    // generating a new one: the history key is derived from the private key, so
    // a replacement identity silently makes every stored message unreadable and
    // changes the fingerprint every contact verified. When that happens the app
    // starts in an error state that the shell surfaces, and touches nothing.
    let mut init_error: Option<String> = None;
    let (history_path, identity, is_new) = if let Some(data) = data_dir {
        std::fs::create_dir_all(&data).ok();
        tracing::info!(data_dir = %data.display(), "using data directory");
        match Identity::get_or_create(&data, "User") {
            Ok((id, isnew)) => (data.join("history.json.enc"), id, isnew),
            Err(e) => {
                tracing::error!("identity load failed: {e}");
                init_error = Some(e.to_string());
                // A throwaway in-memory identity so the app can boot far enough
                // to *show* the error. Every command is refused while
                // `init_error` is set, so it is never saved or used on the wire.
                (
                    data.join("history.json.enc"),
                    Identity::new_with_plaintext("User".to_string()).expect("identity"),
                    true,
                )
            }
        }
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
    // Plaintext key in hand (no password) ⇒ force a set-password step.
    let force_setup = !identity.is_locked();

    let manager = Arc::new(Mutex::new(ChatManager::new(Config::default())));

    Bridge {
        manager,
        party: Arc::new(Mutex::new(PartyManager::new())),
        identity: Arc::new(StdMutex::new(identity)),
        discovery: Arc::new(StdMutex::new(None)),
        discovered: Arc::new(StdMutex::new(Vec::new())),
        history_path,
        identity_path,
        parties_path,
        is_new: StdMutex::new(is_new),
        force_setup: StdMutex::new(force_setup),
        pending_fp: Arc::new(StdMutex::new(None)),
        init_error,
        saved_sig: Arc::new(StdMutex::new(None)),
    }
}

/// Signature of everything the chat UI renders (a superset of
/// `state_signature`): connection states, transfer progress, hosting and lock
/// state. The poll loop emits `state-updated` only when this changes, so an
/// idle app no longer makes the webview refetch conversations four times a
/// second.
fn ui_signature(mgr: &ChatManager) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    state_signature(mgr).hash(&mut h);
    for id in mgr.chat_ids() {
        mgr.is_connected(&id).hash(&mut h);
        // TOFU trust: accepting a fingerprint flips the conversation's
        // "verified" badge without changing any message count, so it must
        // perturb the signature or the warning UI goes stale.
        if let Some(chat) = mgr.get_chat(id) {
            chat.peer_fingerprint.hash(&mut h);
        }
    }
    for t in mgr.active_transfers_snapshot() {
        t.id.hash(&mut h);
        t.received.hash(&mut h);
        std::mem::discriminant(&t.status).hash(&mut h);
    }
    mgr.is_hosting.hash(&mut h);
    mgr.hosting_port.hash(&mut h);
    mgr.external_address.is_some().hash(&mut h);
    mgr.is_conversation_locked().hash(&mut h);
    mgr.contacts.len().hash(&mut h);
    h.finish()
}

/// Signature of the Communities surface, so `party-updated` fires only when
/// the directory or a thread actually changed.
fn party_signature(p: &PartyManager) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    for sid in p.server_ids() {
        sid.hash(&mut h);
        if let Some(conn) = p.server(sid) {
            format!("{:?}", conn.status).hash(&mut h);
            conn.server_name.hash(&mut h);
            conn.channels.len().hash(&mut h);
            conn.members.len().hash(&mut h);
            conn.last_error.is_some().hash(&mut h);
            let total_msgs: usize = conn.messages.values().map(|v| v.len()).sum();
            total_msgs.hash(&mut h);
        }
    }
    h.finish()
}

fn toast_level_str(l: ToastLevel) -> &'static str {
    match l {
        ToastLevel::Success => "success",
        ToastLevel::Error => "error",
        ToastLevel::Warning | ToastLevel::Info => "info",
    }
}

/// Everything the poll loop needs from the [`Bridge`], cloned once at startup so
/// the loop owns its handles rather than borrowing Tauri state on every tick.
struct PollContext {
    manager: Arc<Mutex<ChatManager>>,
    party: Arc<Mutex<PartyManager>>,
    identity: Arc<StdMutex<Identity>>,
    history_path: PathBuf,
    parties_path: PathBuf,
    pending_fp: PendingFp,
    saved_sig: SavedSig,
}

impl PollContext {
    fn from_bridge(b: &Bridge) -> Self {
        Self {
            manager: b.manager.clone(),
            party: b.party.clone(),
            identity: b.identity.clone(),
            history_path: b.history_path.clone(),
            parties_path: b.parties_path.clone(),
            pending_fp: b.pending_fp.clone(),
            saved_sig: b.saved_sig.clone(),
        }
    }
}

/// Periodically drain network events and notify the webview to refresh.
fn spawn_poll_loop(app: tauri::AppHandle, ctx: PollContext) {
    let PollContext {
        manager,
        party,
        identity,
        history_path,
        parties_path,
        pending_fp,
        saved_sig,
    } = ctx;
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        // Change-only event emission: the webview refetches on `state-updated`
        // and `party-updated`, so an idle tick must not fire them.
        let mut last_ui_sig: Option<u64> = None;
        let mut last_party_sig: Option<u64> = None;
        // Rate-limits the auto-rehost check (mirrors the egui app's 1.5s timer).
        let mut last_rehost = std::time::Instant::now();
        // Servers already recorded as Joined, so the saved-communities file is
        // touched once per join (to fill in the server's display name), not every tick.
        let mut joined_recorded: std::collections::HashSet<Uuid> = Default::default();
        loop {
            interval.tick().await;
            // Drain Party/Community server events (joins, channel/member updates,
            // incoming messages) so the webview's Communities pane stays live.
            let party_sig;
            {
                let mut p = party.lock().await;
                p.poll_events();
                party_sig = party_signature(&p);
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
            // Auto-rehost: with auto-host enabled, an accepted connection
            // consumes the listening placeholder — without this, the app
            // silently stops accepting new peers after the first one (the
            // egui app has the same loop). No-op while the identity is
            // locked (private key unavailable, config not loaded yet).
            if last_rehost.elapsed() >= Duration::from_millis(1500) {
                last_rehost = std::time::Instant::now();
                let pk = {
                    let id = identity.lock().unwrap();
                    if id.is_locked() {
                        None
                    } else {
                        id.private_key().ok()
                    }
                };
                if let Some(pk) = pk {
                    let mut m = manager.lock().await;
                    if m.config.auto_host_on_startup && m.check_rehost_needed() {
                        let port = m.config.listen_port;
                        match m.start_host(port, pk).await {
                            Ok(_) => {
                                tracing::info!(port, "auto-rehosted after a consumed listener")
                            }
                            Err(e) => tracing::warn!("auto-rehost failed: {e}"),
                        }
                    }
                }
            }
            let (req, toasts, sig, ui_sig) = {
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
                let ui_sig = ui_signature(&m);
                (req, toasts, sig, ui_sig)
            };
            // Persist when the conversation surface changed (e.g. a received
            // message). The marker is shared with the commands, which also save
            // immediately — so a user action no longer costs a second identical
            // rewrite of the whole encrypted history on the next tick.
            let already_saved = *saved_sig.lock().unwrap_or_else(|e| e.into_inner()) == Some(sig);
            if !already_saved {
                persist_history(&manager, &history_path, &saved_sig).await;
            }
            for (level, message) in toasts {
                let _ = app.emit(
                    "toast",
                    serde_json::json!({ "level": level, "message": message }),
                );
            }
            let had_fp_request = req.is_some();
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
            // Emit refresh events only when the rendered surface changed (or a
            // TOFU prompt appeared, so its fallback polling always runs).
            if last_ui_sig != Some(ui_sig) || had_fp_request {
                last_ui_sig = Some(ui_sig);
                let _ = app.emit("state-updated", ());
            }
            // Nudge the Communities pane only when the directory/messages changed.
            if last_party_sig != Some(party_sig) {
                last_party_sig = Some(party_sig);
                let _ = app.emit("party-updated", ());
            }
        }
    });
}
