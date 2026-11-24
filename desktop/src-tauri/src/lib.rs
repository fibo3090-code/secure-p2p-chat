//! P2PEM desktop bridge: a thin Tauri layer over the UI-agnostic `ChatManager`,
//! sharing one tokio runtime with Tauri and running a poll loop that drains
//! `SessionEvent`s and notifies the webview.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard};
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

use p2pem_classic::app::party_manager::{PartyJoinOutcome, PartyManager, PartyStatus};
use p2pem_classic::app::{ChatManager, FingerprintWarning, PendingFingerprint};

/// The TOFU prompt currently published to the frontend, held for it to poll.
type PendingFp = Arc<StdMutex<Option<PendingFingerprint>>>;

/// Shared application state managed by Tauri.
struct Bridge {
    manager: Arc<Mutex<ChatManager>>,
    /// Community server connections; ephemeral, re-fetched on join.
    party: Arc<Mutex<PartyManager>>,
    /// Shared with the poll loop, which needs the private key for auto-rehost.
    identity: Arc<StdMutex<Identity>>,
    /// mDNS handle, created lazily when `enable_mdns` is on.
    discovery: Arc<StdMutex<Option<Discovery>>>,
    discovered: Arc<StdMutex<Vec<DiscoveredPeer>>>,
    history_path: PathBuf,
    identity_path: PathBuf,
    /// Saved communities with their pinned server fingerprints.
    parties_path: PathBuf,
    /// No password yet (new identity or legacy plaintext key).
    needs_password_setup: AtomicBool,
    /// Queryable copy of the TOFU prompt, so a missed event never strands it.
    pending_fp: PendingFp,
    /// Startup could not read the identity file; every command refuses rather
    /// than replace it with a new identity.
    init_error: Option<String>,
    saved_sig: SavedSig,
}

impl Bridge {
    fn needs_password_setup(&self) -> bool {
        self.needs_password_setup.load(Ordering::SeqCst)
    }

    fn password_setup_done(&self) {
        self.needs_password_setup.store(false, Ordering::SeqCst);
    }
}

/// Lock a std mutex, recovering from poisoning instead of panicking. Every
/// bridge mutex guards a value that is only read or wholly replaced, so a
/// poisoned lock implies no torn state, and panicking would freeze the poll loop.
fn lock_or_recover<T>(mutex: &StdMutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("a bridge mutex was poisoned by an earlier panic; recovering");
        poisoned.into_inner()
    })
}

/// Parse an id the frontend sent, as a command error the UI can show.
fn parse_id(id: &str) -> Result<Uuid, String> {
    Uuid::parse_str(id).map_err(|e| e.to_string())
}

/// Signature of the last successful save, shared by the poll loop and the
/// commands so one change is not written twice.
type SavedSig = Arc<StdMutex<Option<u64>>>;

/// Minimum gap between poll-loop history writes. Each save re-encrypts and
/// fsyncs the whole history, so saving per message would be O(n²); structural
/// commands still save immediately and window close flushes.
const HISTORY_SAVE_MIN_INTERVAL: Duration = Duration::from_secs(3);

/// Best-effort encrypted history save; errors are logged, never propagated. A
/// no-op while locked. The recorded signature is taken under the same lock as
/// the snapshot, so it describes exactly what was written; a failed save leaves
/// it alone so the poll loop retries. An empty unlocked history IS saved, so
/// deleting the last chat sticks.
async fn persist_history(manager: &Arc<Mutex<ChatManager>>, path: &Path, saved_sig: &SavedSig) {
    let (snapshot, sig) = {
        let m = manager.lock().await;
        (m.history_snapshot(), state_signature(&m))
    };
    let Ok((history, key)) = snapshot else {
        return;
    };
    let path = path.to_path_buf();
    match tokio::task::spawn_blocking(move || history.save_encrypted(&path, &key)).await {
        Ok(Ok(())) => *lock_or_recover(saved_sig) = Some(sig),
        Ok(Err(e)) => tracing::warn!("desktop history save failed: {e}"),
        Err(e) => tracing::warn!("desktop history save task join failed: {e}"),
    }
}

/// Signature of the persisted state (shared with the terminal client's
/// autosave), so the poll loop saves only on a real change.
fn state_signature(mgr: &ChatManager) -> u64 {
    mgr.persisted_signature()
}

/// Refuse post-auth commands until the identity is loaded, unlocked and has a
/// password. The shell hides the UI in those states; this enforces it.
fn ensure_ready(state: &Bridge) -> Result<(), String> {
    if let Some(err) = &state.init_error {
        return Err(err.clone());
    }
    if lock_or_recover(&state.identity).is_locked() {
        return Err("Unlock required".to_string());
    }
    if state.needs_password_setup() {
        return Err("Password setup required".to_string());
    }
    Ok(())
}

/// Run a native file dialog on a blocking thread, parented to the app window.
/// An unowned rfd dialog can open behind the window and the app looks frozen.
/// `build` gets a parented dialog and calls `pick_file`/`save_file`/`pick_folder`.
async fn native_file_dialog<R: tauri::Runtime, T, F>(
    window: tauri::WebviewWindow<R>,
    build: F,
) -> Result<T, String>
where
    F: FnOnce(rfd::FileDialog) -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(move || build(rfd::FileDialog::new().set_parent(&window)))
        .await
        .map_err(|e| e.to_string())
}

mod commands;
use commands::party::{upsert_saved_party, SavedParty};

// Not on Windows — see the dev-dependencies note in Cargo.toml.
#[cfg(all(test, not(windows)))]
mod tests;

/// The command registration, shared by the app and the test harness.
fn invoke_handler<R: tauri::Runtime>() -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync {
    tauri::generate_handler![
        commands::auth::auth_status,
        commands::auth::unlock,
        commands::auth::set_password,
        commands::auth::change_password,
        commands::auth::my_identity,
        commands::auth::set_display_name,
        commands::auth::export_identity,
        commands::auth::history_status,
        commands::auth::set_aside_history,
        commands::auth::export_diagnostics,
        commands::auth::open_data_dir,
        commands::auth::delete_all_data,
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
        commands::party::party_create_channel_kind,
        commands::party::party_delete_channel,
        commands::party::party_set_channel_access,
        commands::party::party_set_role,
        commands::party::party_refresh_files,
        commands::party::party_delete_file,
        commands::party::party_refresh_audit,
        commands::party::party_clear_notice,
        commands::party::party_share_file,
        commands::party::party_set_file_permissions,
    ]
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,p2pem_classic=debug,messenger_core=debug")
            }),
        )
        .try_init();

    // One runtime for ChatManager's bare `tokio::spawn`s and Tauri's async
    // commands; leaked so it lives as long as the process.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");
    tauri::async_runtime::set(rt.handle().clone());
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
            // Flush history synchronously so nothing is lost between ticks.
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

/// Build the identity + manager from the desktop app's data directory.
fn init_bridge() -> Bridge {
    // The desktop app's OWN dir ("P2PEM"), not the terminal client's: sharing
    // one would make both apps the same peer. `P2PEM_DATA_DIR` overrides it to
    // run extra test peers on one machine.
    let data_dir: Option<PathBuf> = std::env::var_os("P2PEM_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            directories::ProjectDirs::from("com", "chat-p2p", "P2PEM")
                .map(|dirs| dirs.data_dir().to_path_buf())
        });

    // Never replace an identity that failed to load: the history key derives
    // from it. Boot into an error state instead.
    let mut init_error: Option<String> = None;
    let (history_path, identity, is_new) = if let Some(data) = data_dir {
        std::fs::create_dir_all(&data).ok();
        tracing::info!(data_dir = %data.display(), "using data directory");
        match Identity::get_or_create(&data, "User") {
            Ok((id, isnew)) => (data.join("history.json.enc"), id, isnew),
            Err(e) => {
                tracing::error!("identity load failed: {e}");
                init_error = Some(e.to_string());
                // Throwaway identity, only so the error can be shown; every
                // command refuses while `init_error` is set.
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

    Bridge::new(identity, history_path, is_new, init_error)
}

impl Bridge {
    /// The bridge over `identity`, with its files beside `history_path`.
    fn new(
        identity: Identity,
        history_path: PathBuf,
        is_new: bool,
        init_error: Option<String>,
    ) -> Self {
        let mut chat_manager = ChatManager::new(Config::default());
        // Lets the manager refuse the user's own invite link.
        chat_manager.set_my_fingerprint(identity.fingerprint.clone());
        Bridge {
            manager: Arc::new(Mutex::new(chat_manager)),
            party: Arc::new(Mutex::new(PartyManager::new())),
            needs_password_setup: AtomicBool::new(is_new || !identity.is_locked()),
            identity: Arc::new(StdMutex::new(identity)),
            discovery: Arc::new(StdMutex::new(None)),
            discovered: Arc::new(StdMutex::new(Vec::new())),
            identity_path: history_path.with_file_name("identity.json"),
            parties_path: history_path.with_file_name("parties.json"),
            history_path,
            pending_fp: Arc::new(StdMutex::new(None)),
            init_error,
            saved_sig: Arc::new(StdMutex::new(None)),
        }
    }
}

/// Signature of everything the chat UI renders (a superset of
/// `state_signature`); `state-updated` fires only when it changes.
fn ui_signature(mgr: &ChatManager) -> u64 {
    let mut h = DefaultHasher::new();
    state_signature(mgr).hash(&mut h);
    for id in mgr.chat_ids() {
        mgr.is_connected(&id).hash(&mut h);
        // Accepting a fingerprint flips the "verified" badge without changing
        // any count.
        if let Some(chat) = mgr.get_chat(id) {
            chat.peer_fingerprint.hash(&mut h);
        }
    }
    // Status changes only; byte progress is `transfer_signature`'s (PERF-01).
    for t in mgr.active_transfers_snapshot() {
        t.id.hash(&mut h);
        std::mem::discriminant(&t.status).hash(&mut h);
    }
    mgr.is_hosting.hash(&mut h);
    mgr.hosting_port.hash(&mut h);
    mgr.external_address.is_some().hash(&mut h);
    mgr.is_conversation_locked().hash(&mut h);
    mgr.contacts.len().hash(&mut h);
    h.finish()
}

/// Transfer progress. When only this moved, the poll loop emits the light
/// `transfers-updated` instead of a full `state-updated` (PERF-01).
fn transfer_signature(mgr: &ChatManager) -> u64 {
    let mut h = DefaultHasher::new();
    for t in mgr.active_transfers_snapshot() {
        t.id.hash(&mut h);
        t.received.hash(&mut h);
    }
    h.finish()
}

/// Signature of the Communities surface, so `party-updated` fires only on change.
fn party_signature(p: &PartyManager) -> u64 {
    let mut h = DefaultHasher::new();
    for sid in p.server_ids() {
        sid.hash(&mut h);
        if let Some(conn) = p.server(sid) {
            format!("{:?}", conn.status).hash(&mut h);
            conn.server_name.hash(&mut h);
            // Hash what is shown, not counts: presence changes keep counts equal.
            for c in &conn.channels {
                c.id.hash(&mut h);
                c.name.hash(&mut h);
                c.kind.hash(&mut h);
                c.members.hash(&mut h);
            }
            for m in &conn.members {
                m.id.hash(&mut h);
                m.username.hash(&mut h);
                m.online.hash(&mut h);
                m.role.hash(&mut h);
            }
            conn.last_error.hash(&mut h);
            conn.last_notice.hash(&mut h);
            // Drive listing, quota and audit arrive asynchronously.
            conn.files.len().hash(&mut h);
            for f in &conn.files {
                f.hash.hash(&mut h);
                f.location.hash(&mut h);
                f.name.hash(&mut h);
                f.perms.hash(&mut h);
            }
            conn.quota.map(|q| (q.used, q.server_used)).hash(&mut h);
            conn.audit.len().hash(&mut h);
            conn.audit.first().map(|a| a.at).hash(&mut h);
            // Length *and* last seq: a retraction plus an arrival keeps the length.
            let mut threads: Vec<_> = conn.messages.iter().collect();
            threads.sort_by_key(|(id, _)| **id);
            for (thread, msgs) in threads {
                thread.hash(&mut h);
                msgs.len().hash(&mut h);
                msgs.last().map(|e| (e.seq, e.timestamp)).hash(&mut h);
            }
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

/// The poll loop's handles, cloned once from the [`Bridge`] at startup.
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

const REHOST_CHECK_INTERVAL: Duration = Duration::from_millis(1500);

/// What the poll loop remembers between ticks, to emit and write only on change.
#[derive(Default)]
struct PollMemory {
    last_ui_sig: Option<u64>,
    last_transfer_sig: Option<u64>,
    last_party_sig: Option<u64>,
    /// Session id of the TOFU prompt currently published to the frontend.
    last_fp_session: Option<Uuid>,
    /// Servers already recorded as Joined, so parties.json is touched once per join.
    joined_recorded: std::collections::HashSet<Uuid>,
}

/// Periodically drain network events and notify the webview to refresh.
fn spawn_poll_loop(app: tauri::AppHandle, ctx: PollContext) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        let mut memory = PollMemory::default();
        let mut last_rehost = std::time::Instant::now();
        let mut last_save = std::time::Instant::now() - HISTORY_SAVE_MIN_INTERVAL;
        loop {
            interval.tick().await;
            let party_sig = ctx.poll_parties(&mut memory.joined_recorded).await;
            if last_rehost.elapsed() >= REHOST_CHECK_INTERVAL {
                last_rehost = std::time::Instant::now();
                ctx.auto_rehost().await;
            }
            let (prompt, toasts, sig, ui_sig, transfer_sig) = {
                let mut m = ctx.manager.lock().await;
                m.poll_session_events();
                let toasts: Vec<(&'static str, String)> = std::mem::take(&mut m.toasts)
                    .into_iter()
                    .map(|t| (toast_level_str(t.level), t.message))
                    .collect();
                // Peek, never take: `confirm_fingerprint` needs the queued entry.
                let prompt = m.pending_fingerprint().cloned();
                (
                    prompt,
                    toasts,
                    state_signature(&m),
                    ui_signature(&m),
                    transfer_signature(&m),
                )
            };
            if ctx.save_history_if_changed(sig, last_save).await {
                last_save = std::time::Instant::now();
            }
            for (level, message) in toasts {
                let _ = app.emit(
                    "toast",
                    serde_json::json!({ "level": level, "message": message }),
                );
            }
            let new_prompt =
                ctx.publish_fingerprint_prompt(&app, prompt, &mut memory.last_fp_session);
            let transfers_moved = memory.last_transfer_sig != Some(transfer_sig);
            memory.last_transfer_sig = Some(transfer_sig);
            if memory.last_ui_sig != Some(ui_sig) || new_prompt {
                memory.last_ui_sig = Some(ui_sig);
                // A full refresh refetches the transfers too.
                let _ = app.emit("state-updated", ());
            } else if transfers_moved {
                let _ = app.emit("transfers-updated", ());
            }
            if memory.last_party_sig != Some(party_sig) {
                memory.last_party_sig = Some(party_sig);
                let _ = app.emit("party-updated", ());
            }
        }
    });
}

impl PollContext {
    /// Drain community events and return their signature. A community is saved
    /// (and its fingerprint pinned) only once the server has accepted us, so a
    /// typo'd address or wrong password leaves no entry behind.
    async fn poll_parties(&self, joined_recorded: &mut std::collections::HashSet<Uuid>) -> u64 {
        let mut p = self.party.lock().await;
        p.poll_events();
        for sid in p.server_ids() {
            let Some(conn) = p.server(sid) else { continue };
            if conn.status != PartyStatus::Joined || !joined_recorded.insert(sid) {
                continue;
            }
            upsert_saved_party(
                &self.parties_path,
                SavedParty {
                    address: conn.address.clone(),
                    username: conn.username.clone(),
                    name: conn.server_name.clone(),
                    fingerprint: conn.server_fingerprint.clone(),
                },
            );
        }
        party_signature(&p)
    }

    /// Re-open the listener after an accepted peer consumed it. Not gated on
    /// `auto_host_on_startup` (that is about launch); `check_rehost_needed()`
    /// requires `is_hosting` and an unlocked conversation. No-op while locked.
    async fn auto_rehost(&self) {
        let pk = {
            let id = lock_or_recover(&self.identity);
            if id.is_locked() {
                return;
            }
            match id.private_key() {
                Ok(pk) => pk,
                Err(_) => return,
            }
        };
        let mut m = self.manager.lock().await;
        if !m.check_rehost_needed() {
            return;
        }
        // Rebind the port in use, not the settings default: the shared address
        // must stay valid.
        let port = m.hosting_port.unwrap_or(m.config.listen_port);
        match m.start_host(port, pk).await {
            Ok(_) => tracing::info!(port, "auto-rehosted after a consumed listener"),
            Err(e) => tracing::warn!("auto-rehost failed: {e}"),
        }
    }

    /// Persist on change, at most once per `HISTORY_SAVE_MIN_INTERVAL`.
    /// Returns whether it wrote.
    async fn save_history_if_changed(&self, sig: u64, last_save: std::time::Instant) -> bool {
        let already_saved = *lock_or_recover(&self.saved_sig) == Some(sig);
        if already_saved || last_save.elapsed() < HISTORY_SAVE_MIN_INTERVAL {
            return false;
        }
        persist_history(&self.manager, &self.history_path, &self.saved_sig).await;
        true
    }

    /// Publish the head of the manager's TOFU queue. Fires only when the head's
    /// identity changes, or a closed dialog would reopen every tick. Returns
    /// `true` when a new prompt appeared.
    fn publish_fingerprint_prompt(
        &self,
        app: &tauri::AppHandle,
        prompt: Option<PendingFingerprint>,
        last_session: &mut Option<Uuid>,
    ) -> bool {
        let head = prompt.as_ref().map(|p| p.session_id);
        if head == *last_session {
            return false;
        }
        *last_session = head;
        let Some(prompt) = prompt else {
            // Queue drained: stop the fallback poll resurfacing a dead prompt.
            *lock_or_recover(&self.pending_fp) = None;
            return false;
        };
        tracing::info!(peer = %prompt.peer_name, session = %prompt.session_id, "TOFU fingerprint verification requested");
        let payload = fingerprint_prompt_json(&prompt);
        *lock_or_recover(&self.pending_fp) = Some(prompt);
        let _ = app.emit("fingerprint-request", payload);
        true
    }
}

/// The shape the frontend expects for a TOFU prompt (event and poll alike).
///
/// `warning` is `null` for an ordinary first use, otherwise
/// `{ kind: "key_changed", previous }` or
/// `{ kind: "invite_mismatch", contact, expected }` (CRYPTO-04). `Verify`
/// leads with it, so a key that is not the invite's never reads as a first
/// contact.
fn fingerprint_prompt_json(prompt: &PendingFingerprint) -> serde_json::Value {
    let warning = match &prompt.warning {
        None => serde_json::Value::Null,
        Some(FingerprintWarning::KeyChanged { previous }) => serde_json::json!({
            "kind": "key_changed",
            "previous": previous,
        }),
        Some(FingerprintWarning::InviteMismatch {
            contact_name,
            expected,
            ..
        }) => serde_json::json!({
            "kind": "invite_mismatch",
            "contact": contact_name,
            "expected": expected,
        }),
    };
    serde_json::json!({
        "fingerprint": prompt.fingerprint,
        "peer_name": prompt.peer_name,
        "sas": prompt.sas,
        "chat_id": prompt.session_id.to_string(),
        "warning": warning,
    })
}
