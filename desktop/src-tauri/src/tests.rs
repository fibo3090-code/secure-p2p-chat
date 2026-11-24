//! Bridge integration tests: drive the real command handlers through Tauri's
//! mock runtime, over the same IPC path the webview uses.
//!
//! Two invariants matter here:
//!
//! 1. **Invoke keys must bind.** Tauri 2 matches JS payload keys to Rust
//!    parameter names by exact name; a mismatch makes the command fail
//!    argument deserialization (surfaced as an "invalid args" error) — the
//!    root cause of past "messages send but don't arrive" bugs. Every call in
//!    these tests uses the exact keys `desktop/src/lib/bridge.js` sends, so a
//!    rename on either side breaks CI instead of production.
//! 2. **The auth barrier holds.** No state-mutating command may run before
//!    unlock / set-password completes, no matter what the frontend does.
//!
//! The mock runtime has no window and no webview process, so anything driven
//! purely by UI events stays out of scope; commands that open native dialogs
//! (`send_file`, `export_identity`, `pick_download_dir`) are exercised only
//! for their auth gating.

use std::sync::{Arc, Mutex as StdMutex};

use messenger_core::identity::Identity;
use messenger_core::types::Config;
use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody, InvokeResponseBody};
use tauri::test::{mock_builder, mock_context, noop_assets, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tokio::sync::Mutex;

use p2pem_classic::app::party_manager::PartyManager;
use p2pem_classic::app::ChatManager;

use crate::{invoke_handler, Bridge};

/// A bridge over a fresh plaintext identity in a temp dir — the state a brand
/// new install boots into (password setup pending).
fn fresh_bridge(dir: &std::path::Path) -> Bridge {
    let identity = Identity::new_with_plaintext("Tester".to_string()).expect("identity");
    let history_path = dir.join("history.json.enc");
    // Same wiring the real bridge does: without it the manager cannot recognise
    // the user's own invite link, so a harness would silently exercise a
    // different code path from production.
    let mut manager = ChatManager::new(Config::default());
    manager.set_my_fingerprint(identity.fingerprint.clone());
    Bridge {
        manager: Arc::new(Mutex::new(manager)),
        party: Arc::new(Mutex::new(PartyManager::new())),
        identity: Arc::new(StdMutex::new(identity)),
        discovery: Arc::new(StdMutex::new(None)),
        discovered: Arc::new(StdMutex::new(Vec::new())),
        identity_path: history_path.with_file_name("identity.json"),
        parties_path: history_path.with_file_name("parties.json"),
        history_path,
        needs_password_setup: std::sync::atomic::AtomicBool::new(true),
        pending_fp: Arc::new(StdMutex::new(None)),
        init_error: None,
        saved_sig: Arc::new(StdMutex::new(None)),
        _instance_lock: None,
    }
}

/// A bridge that failed to read an existing identity at startup.
fn broken_identity_bridge(dir: &std::path::Path) -> Bridge {
    Bridge {
        init_error: Some("identity file could not be read".to_string()),
        ..fresh_bridge(dir)
    }
}

struct Harness {
    // Held alive for the duration of the test; dropping the app tears down the
    // mock runtime under the webview.
    _app: tauri::App<tauri::test::MockRuntime>,
    webview: tauri::WebviewWindow<tauri::test::MockRuntime>,
    _dir: tempfile::TempDir,
    /// The poll loop's memory of which prompt it last published.
    last_prompt: std::cell::RefCell<Option<uuid::Uuid>>,
}

fn harness_with(bridge: Bridge, dir: tempfile::TempDir) -> Harness {
    let app = mock_builder()
        .manage(bridge)
        .invoke_handler(invoke_handler())
        .build(mock_context(noop_assets()))
        .expect("mock app");
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("mock webview");
    Harness {
        _app: app,
        webview,
        _dir: dir,
        last_prompt: std::cell::RefCell::new(None),
    }
}

fn harness() -> Harness {
    let dir = tempfile::tempdir().expect("tempdir");
    let bridge = fresh_bridge(dir.path());
    harness_with(bridge, dir)
}

/// A harness that has completed password setup and is fully "ready".
fn ready_harness() -> Harness {
    let h = harness();
    h.ipc(
        "set_password",
        json!({ "password": "bridge-test-passphrase" }),
    )
    .expect("set_password");
    h
}

impl Harness {
    /// Where this harness's bridge keeps the encrypted history.
    fn bridge_history_path(&self) -> std::path::PathBuf {
        self._dir.path().join("history.json.enc")
    }

    /// Invoke a command over the mock IPC exactly like the webview would.
    fn ipc(&self, cmd: &str, args: Value) -> Result<Value, Value> {
        tauri::test::get_ipc_response(
            &self.webview,
            InvokeRequest {
                cmd: cmd.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: LOCAL_ORIGIN.parse().unwrap(),
                body: InvokeBody::Json(args),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            },
        )
        .map(|body| match body {
            InvokeResponseBody::Json(s) => {
                serde_json::from_str(&s).expect("command returned invalid JSON")
            }
            InvokeResponseBody::Raw(_) => Value::Null,
        })
    }

    /// Invoke and require that, whatever the outcome, the arguments *bound*:
    /// an "invalid args" error means the payload keys no longer match the
    /// command's parameter names (the silent-no-op footgun in production).
    fn ipc_binds(&self, cmd: &str, args: Value) -> Result<Value, Value> {
        let res = self.ipc(cmd, args);
        if let Err(e) = &res {
            let msg = e.as_str().unwrap_or_default();
            assert!(
                !msg.contains("invalid args"),
                "command `{cmd}`: payload keys no longer bind: {msg}"
            );
        }
        res
    }
}

impl Harness {
    /// Seed a conversation holding `incoming` peer messages, as if they had
    /// arrived over the wire. Returns the chat id.
    fn seed_chat_with_incoming(&self, title: &str, incoming: usize) -> uuid::Uuid {
        use tauri::Manager;
        let id = uuid::Uuid::new_v4();
        let bridge = self._app.state::<Bridge>();
        let manager = bridge.manager.clone();
        tauri::async_runtime::block_on(async move {
            let mut mgr = manager.lock().await;
            mgr.create_local_chat_for_test(id, title.to_string());
            if let Some(chat) = mgr.get_chat_mut(id) {
                for _ in 0..incoming {
                    chat.messages.push(messenger_core::types::Message {
                        id: uuid::Uuid::new_v4(),
                        from_me: false,
                        content: messenger_core::types::MessageContent::Text {
                            text: "hi".to_string(),
                        },
                        timestamp: chrono::Utc::now(),
                        delivered: false,
                    });
                }
            }
        });
        id
    }

    /// The `unread` field the conversation list reports for a chat.
    fn unread_of(&self, id: uuid::Uuid) -> u64 {
        let list = self.ipc("list_conversations", json!({})).unwrap();
        list.as_array()
            .expect("list")
            .iter()
            .find(|c| c["id"] == id.to_string())
            .and_then(|c| c["unread"].as_u64())
            .expect("conversation missing from the list")
    }
}

fn err_text(e: &Value) -> &str {
    e.as_str().unwrap_or_default()
}

/// The webview origin Tauri considers *local* — remote origins are ACL-gated
/// and would reject every command. Windows serves the app from
/// `http://tauri.localhost`; everywhere else it's `tauri://localhost`.
#[cfg(windows)]
const LOCAL_ORIGIN: &str = "http://tauri.localhost";
#[cfg(not(windows))]
const LOCAL_ORIGIN: &str = "tauri://localhost";

// ── Auth lifecycle ──────────────────────────────────────────────────────────

#[test]
fn fresh_identity_requires_password_setup() {
    let h = harness();
    let status = h.ipc("auth_status", json!({})).unwrap();
    assert_eq!(status["state"], "set_password");

    // The barrier: every post-auth command must refuse to run.
    for (cmd, args) in [
        ("get_settings", json!({})),
        ("list_conversations", json!({})),
        ("mark_read", json!({ "id": "x" })),
        (
            "change_password",
            json!({ "current": "a", "new": "bridge-test-passphrase-2" }),
        ),
        ("set_presence", json!({ "focused": true, "chat": null })),
        ("list_contacts", json!({})),
        ("start_host", json!({ "port": 0, "password": null })),
        ("send_message", json!({ "id": "x", "text": "hi" })),
        ("set_display_name", json!({ "name": "Eve" })),
        ("export_identity", json!({})),
        ("export_diagnostics", json!({})),
        ("pick_download_dir", json!({})),
        ("my_invite_link", json!({})),
        ("set_locked", json!({ "locked": true })),
        (
            "open_file",
            json!({ "id": "x", "msg": "y", "reveal": false }),
        ),
        ("file_preview", json!({ "id": "x", "msg": "y" })),
        ("open_url", json!({ "url": "https://example.invalid" })),
    ] {
        let err = h.ipc(cmd, args).expect_err(cmd);
        assert_eq!(
            err_text(&err),
            "Password setup required",
            "command `{cmd}` ran before password setup"
        );
    }

    // Setup completes → the same surface opens up.
    h.ipc(
        "set_password",
        json!({ "password": "bridge-test-passphrase" }),
    )
    .unwrap();
    let status = h.ipc("auth_status", json!({})).unwrap();
    assert_eq!(status["state"], "ready");
    let settings = h.ipc("get_settings", json!({})).unwrap();
    assert_eq!(settings["listen_port"], 12345);
}

/// The password floor is enforced by the core, not merely suggested by the UI —
/// a frontend that skipped its own validation must still be refused, and the
/// error has to be specific enough to show the user.
#[test]
fn set_password_enforces_the_length_floor() {
    let h = harness();
    let min = messenger_core::MIN_PASSWORD_LEN;
    let short = "a".repeat(min - 1);
    let err = h
        .ipc("set_password", json!({ "password": short }))
        .expect_err("a short password was accepted");
    assert!(
        err_text(&err).contains("at least"),
        "unhelpful error for a short password: {}",
        err_text(&err)
    );
    // The rejection must not half-apply: setup is still pending.
    assert_eq!(
        h.ipc("auth_status", json!({})).unwrap()["state"],
        "set_password"
    );

    h.ipc("set_password", json!({ "password": "a".repeat(min) }))
        .expect("an acceptable password was refused");
    assert_eq!(h.ipc("auth_status", json!({})).unwrap()["state"], "ready");
}

/// An identity file that exists but cannot be read must surface as a distinct
/// terminal state — **never** as `set_password`, which would invite the user to
/// create a new identity right over the one that failed to load, abandoning
/// their history and their contacts' verified fingerprint.
#[test]
fn unreadable_identity_reports_an_error_state_and_gates_everything() {
    let dir = tempfile::tempdir().expect("tempdir");
    let h = harness_with(broken_identity_bridge(dir.path()), dir);

    let status = h.ipc("auth_status", json!({})).unwrap();
    assert_eq!(status["state"], "error");
    assert!(
        status["error"]
            .as_str()
            .unwrap_or_default()
            .contains("could not be read"),
        "the reason must reach the user: {status}"
    );

    // Nothing may run in this state — least of all set_password.
    for (cmd, args) in [
        ("set_password", json!({ "password": "a-long-enough-pass" })),
        ("get_settings", json!({})),
        ("list_conversations", json!({})),
        ("export_identity", json!({})),
        ("set_display_name", json!({ "name": "Eve" })),
    ] {
        let err = h.ipc(cmd, args).expect_err(cmd);
        assert!(
            err_text(&err).contains("could not be read"),
            "command `{cmd}` ran with a broken identity: {}",
            err_text(&err)
        );
    }
}

/// The screen reads the floor from the bridge rather than hardcoding one, so
/// `auth_status` has to report it.
/// An existing history file that cannot be read must survive the unlock: the
/// poll loop and every immediate save used to write the empty state over it.
#[test]
fn an_unreadable_history_survives_unlock_and_can_be_set_aside() {
    let h = harness();
    std::fs::write(h.bridge_history_path(), b"written by a future version").unwrap();

    h.ipc(
        "set_password",
        json!({ "password": "bridge-test-passphrase" }),
    )
    .expect("set_password");
    let status = h.ipc("history_status", json!({})).unwrap();
    assert!(
        status["error"].is_string(),
        "the failure must be reported: {status}"
    );

    // A command that saves immediately must not touch the file.
    h.ipc("set_display_name", json!({ "name": "Eve" })).unwrap();
    assert_eq!(
        std::fs::read(h.bridge_history_path()).unwrap(),
        b"written by a future version"
    );

    let moved = h.ipc("set_aside_history", json!({})).unwrap();
    let moved = moved.as_str().expect("the new location is returned");
    assert_eq!(
        std::fs::read(moved).unwrap(),
        b"written by a future version"
    );
    assert!(h.ipc("history_status", json!({})).unwrap()["error"].is_null());
}

/// JOURNEY-03 / DOC-02: deleting everything needs the password and the typed
/// word, removes the files, and nothing written afterwards brings them back.
#[test]
fn delete_all_data_needs_the_password_and_the_word_then_removes_everything() {
    let h = ready_harness();
    let identity_path = h.bridge_history_path().with_file_name("identity.json");
    // Settings save immediately, so this puts a history file on disk.
    let settings = json!({ "settings": {
        "enable_notifications": false,
        "enable_typing_indicators": false,
        "auto_host_on_startup": false,
        "listen_port": 23456,
        "enable_upnp": false,
        "auto_accept_files": false,
        "auto_connect": false,
        "enable_mdns": false,
    }});
    h.ipc("update_settings", settings.clone()).unwrap();
    std::fs::write(identity_path.with_file_name("parties.json"), b"[]").unwrap();
    assert!(identity_path.exists() && h.bridge_history_path().exists());

    let wrong_word = h
        .ipc(
            "delete_all_data",
            json!({ "password": "bridge-test-passphrase", "confirm": "yes" }),
        )
        .expect_err("the typed confirmation is required");
    assert!(err_text(&wrong_word).contains("DELETE"));
    let wrong_password = h
        .ipc(
            "delete_all_data",
            json!({ "password": "not-the-passphrase", "confirm": "DELETE" }),
        )
        .expect_err("the password is required");
    assert!(err_text(&wrong_password).contains("nothing was deleted"));
    assert!(identity_path.exists(), "a refused wipe must delete nothing");

    h.ipc(
        "delete_all_data",
        json!({ "password": "bridge-test-passphrase", "confirm": "DELETE" }),
    )
    .expect("delete_all_data");
    assert!(!identity_path.exists());
    assert!(!h.bridge_history_path().exists());

    // A save attempted afterwards (the close flush does exactly this) must
    // not put the history back.
    let _ = h.ipc("update_settings", settings);
    assert!(!h.bridge_history_path().exists());
    assert!(!identity_path.with_file_name("parties.json").exists());
}

#[test]
fn auth_status_publishes_the_password_floor() {
    let h = harness();
    let status = h.ipc("auth_status", json!({})).unwrap();
    assert_eq!(status["min_password_len"], messenger_core::MIN_PASSWORD_LEN);
}

#[test]
fn locked_identity_requires_unlock() {
    let dir = tempfile::tempdir().expect("tempdir");
    // An identity that already has a password, as saved by a previous run.
    let mut identity = Identity::new_with_plaintext("Tester".to_string()).expect("identity");
    identity.encrypt("right-password-1234").expect("encrypt");
    let mut bridge = fresh_bridge(dir.path());
    identity.save(&bridge.identity_path).expect("save identity");
    bridge.identity = Arc::new(StdMutex::new(identity));
    bridge.needs_password_setup = std::sync::atomic::AtomicBool::new(false);
    let h = harness_with(bridge, dir);

    assert_eq!(h.ipc("auth_status", json!({})).unwrap()["state"], "unlock");
    let err = h.ipc("get_settings", json!({})).expect_err("gated");
    assert_eq!(err_text(&err), "Unlock required");

    let err = h
        .ipc("unlock", json!({ "password": "wrong" }))
        .expect_err("wrong password accepted");
    assert_eq!(err_text(&err), "Wrong password");
    assert_eq!(h.ipc("auth_status", json!({})).unwrap()["state"], "unlock");

    h.ipc("unlock", json!({ "password": "right-password-1234" }))
        .expect("unlock");
    assert_eq!(h.ipc("auth_status", json!({})).unwrap()["state"], "ready");
}

/// TAURI-07: `unlock` ran Argon2 (64 MiB, ~0.5–2 s) with the identity mutex
/// held, on a runtime worker — every command that reads the identity, and the
/// poll loop behind them, waited it out with the window frozen. The key is
/// derived on a blocking thread from a copy; the mutex is free meanwhile.
#[test]
fn unlocking_does_not_hold_the_identity_while_the_key_is_derived() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut identity = Identity::new_with_plaintext("Tester".to_string()).expect("identity");
    identity.encrypt("right-password-1234").expect("encrypt");
    let mut bridge = fresh_bridge(dir.path());
    identity.save(&bridge.identity_path).expect("save identity");
    bridge.identity = Arc::new(StdMutex::new(
        Identity::load(&bridge.identity_path).unwrap(),
    ));
    bridge.needs_password_setup = std::sync::atomic::AtomicBool::new(false);
    let shared = bridge.identity.clone();
    let h = harness_with(bridge, dir);
    assert!(shared.lock().unwrap().is_locked());

    // While `unlock` is in flight, sample the mutex. Free *and* still locked
    // means the key is being derived without holding it. Dispatch overhead
    // alone accounts for a sample or two; a derivation done under the lock
    // gives none at all, so demand a run that only Argon2 is long enough for.
    use std::sync::atomic::{AtomicBool, Ordering};
    let (running, stop) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
    let watcher = {
        let (shared, running, stop) = (shared.clone(), running.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut free_while_locked = 0u32;
            while !stop.load(Ordering::SeqCst) {
                if running.load(Ordering::SeqCst) {
                    if let Ok(id) = shared.try_lock() {
                        if id.is_locked() {
                            free_while_locked += 1;
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            free_while_locked
        })
    };
    let started = std::time::Instant::now();
    running.store(true, Ordering::SeqCst);
    h.ipc("unlock", json!({ "password": "right-password-1234" }))
        .expect("unlock");
    running.store(false, Ordering::SeqCst);
    stop.store(true, Ordering::SeqCst);
    let elapsed = started.elapsed();
    assert!(!shared.lock().unwrap().is_locked());
    let free = watcher.join().unwrap();
    assert!(
        free >= 20,
        "the identity mutex was free for only {free} samples of a {elapsed:?} unlock"
    );
}

/// TAURI-20: every unlock failure read "Wrong password", including a file
/// whose salt or nonce is gone — damage no amount of retyping can fix, sent
/// round in a loop of retries. A wrong password still says so; a damaged
/// file says it is damaged and where to go from there.
#[test]
fn unlock_tells_a_damaged_identity_from_a_wrong_password() {
    let dir = tempfile::tempdir().unwrap();
    let mut identity = Identity::new_with_plaintext("Tester".to_string()).expect("identity");
    identity.encrypt("right-password-1234").expect("encrypt");
    let mut bridge = fresh_bridge(dir.path());
    identity.save(&bridge.identity_path).expect("save identity");
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&bridge.identity_path).unwrap()).unwrap();
    json["nonce"] = json!([1, 2, 3]);
    std::fs::write(&bridge.identity_path, json.to_string()).unwrap();
    bridge.identity = Arc::new(StdMutex::new(
        Identity::load(&bridge.identity_path).unwrap(),
    ));
    bridge.needs_password_setup = std::sync::atomic::AtomicBool::new(false);
    let h = harness_with(bridge, dir);

    let failure = h
        .ipc("unlock", json!({ "password": "right-password-1234" }))
        .unwrap_err();
    let err = err_text(&failure).to_string();
    assert!(err.contains("damaged"), "a damaged file read as: {err}");
    assert!(!err.contains("Wrong password"), "{err}");
}

/// TAURI-17: `change_password` re-wraps a *copy* of the identity (Argon2,
/// off the lock) and swaps it in at the end — so a display name changed
/// while it ran was silently put back, in memory and on disk. Renaming waits
/// for the password change instead.
#[test]
fn a_rename_during_a_password_change_is_kept() {
    let h = ready_harness();
    let webview = h.webview.clone();
    let renamer = std::thread::spawn(move || {
        // Land inside the password change's Argon2 window.
        std::thread::sleep(std::time::Duration::from_millis(30));
        tauri::test::get_ipc_response(
            &webview,
            InvokeRequest {
                cmd: "set_display_name".into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: LOCAL_ORIGIN.parse().unwrap(),
                body: InvokeBody::Json(json!({ "name": "Renamed Midway" })),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            },
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    });
    h.ipc(
        "change_password",
        json!({ "current": "bridge-test-passphrase", "new": "bridge-test-passphrase-2" }),
    )
    .expect("change_password");
    renamer.join().unwrap().expect("rename");

    assert_eq!(
        crate::lock_identity(&h.bridge().identity).name,
        "Renamed Midway",
        "the password change put the old name back"
    );
    let on_disk = Identity::load(&h._dir.path().join("identity.json")).unwrap();
    assert_eq!(on_disk.name, "Renamed Midway");
    // And the new password is what opens the file.
    let mut check = on_disk;
    check
        .decrypt("bridge-test-passphrase-2")
        .expect("new password");
}

// ── Invoke-key contract with bridge.js ──────────────────────────────────────

/// Every argument-taking command, called with the exact payload keys
/// `bridge.js` sends. Bogus-but-well-typed values are fine — a semantic error
/// ("No such conversation") proves the args bound; an "invalid args" error
/// fails the test.
#[test]
fn frontend_payload_keys_bind_to_command_args() {
    let h = ready_harness();
    let uuid = uuid::Uuid::new_v4().to_string();
    let calls: Vec<(&str, Value)> = vec![
        ("set_display_name", json!({ "name": "Alice" })),
        ("set_locked", json!({ "locked": false })),
        ("get_conversation", json!({ "id": uuid })),
        ("send_message", json!({ "id": uuid, "text": "hello" })),
        ("send_file", json!({ "id": "not-a-uuid" })), // fails at Uuid::parse, before the dialog
        ("accept_transfer", json!({ "id": uuid })),
        ("decline_transfer", json!({ "id": uuid })),
        ("cancel_transfer", json!({ "id": uuid })),
        ("rename_chat", json!({ "id": uuid, "title": "T" })),
        ("delete_chat", json!({ "id": uuid })),
        (
            "open_file",
            json!({ "id": uuid, "msg": uuid, "reveal": false }),
        ),
        ("file_preview", json!({ "id": uuid, "msg": uuid })),
        // A non-web scheme so the args bind but nothing is ever launched.
        ("open_url", json!({ "url": "ftp://example.invalid/x" })),
        (
            "confirm_fingerprint",
            json!({ "id": uuid, "accept": false }),
        ),
        ("mark_read", json!({ "id": uuid })),
        ("set_presence", json!({ "focused": true, "chat": uuid })),
        ("set_presence", json!({ "focused": false, "chat": null })),
        ("remove_contact", json!({ "id": uuid })),
        ("block_contact", json!({ "id": uuid })),
        ("unblock_contact", json!({ "id": uuid })),
        ("connect_contact", json!({ "id": uuid })),
        ("import_invite", json!({ "link": "not-a-link" })),
        // Dead endpoints: the connection fails, but only after the args bind.
        (
            "connect_peer",
            json!({ "host": "127.0.0.1", "port": 9, "password": null }),
        ),
        ("host_via_relay", json!({ "relay": "127.0.0.1:9" })),
        (
            "connect_via_relay",
            json!({ "relay": "127.0.0.1:9", "token": "tok" }),
        ),
        (
            "update_settings",
            json!({ "settings": {
                "enable_notifications": true,
                "enable_typing_indicators": true,
                "auto_host_on_startup": false,
                "listen_port": 12345,
                "enable_upnp": false,
                "auto_accept_files": false,
                "auto_connect": false,
                "enable_mdns": false,
            }}),
        ),
        // Communities governance. These fail on "unknown server" — after the
        // args have bound, which is the whole point of this test.
        ("party_leave", json!({ "server": uuid })),
        (
            "party_create_channel_kind",
            json!({ "server": uuid, "name": "news", "kind": "announce", "members": [] }),
        ),
        (
            "party_delete_channel",
            json!({ "server": uuid, "channel": uuid }),
        ),
        (
            "party_set_channel_access",
            json!({ "server": uuid, "channel": uuid, "kind": "private", "members": [uuid] }),
        ),
        (
            "party_set_role",
            json!({ "server": uuid, "member": uuid, "role": "admin" }),
        ),
        ("party_refresh_files", json!({ "server": uuid })),
        (
            "party_delete_file",
            json!({ "server": uuid, "hash": "abc", "location": uuid }),
        ),
        ("party_refresh_audit", json!({ "server": uuid })),
        ("party_clear_notice", json!({ "server": uuid })),
        (
            "party_share_file",
            json!({ "server": uuid, "hash": "abc", "from": uuid, "channel": uuid, "peer": null }),
        ),
        (
            "party_set_file_permissions",
            json!({
                "server": uuid, "hash": "abc", "location": uuid, "member": null,
                "perms": { "view": true, "download": true, "delete": false, "share": false }
            }),
        ),
        ("party_clear_error", json!({ "server": uuid })),
        ("party_history", json!({ "server": uuid, "channel": uuid })),
        ("party_dm_history", json!({ "server": uuid, "peer": uuid })),
        (
            "party_post",
            json!({ "server": uuid, "channel": uuid, "text": "hi" }),
        ),
        (
            "party_send_dm",
            json!({ "server": uuid, "to": uuid, "text": "hi" }),
        ),
        (
            "party_create_channel",
            json!({ "server": uuid, "name": "chat" }),
        ),
    ];
    for (cmd, args) in calls {
        let _ = h.ipc_binds(cmd, args);
    }
    // No-arg query commands used by the frontend must exist and respond.
    for cmd in [
        "list_conversations",
        "list_transfers",
        "list_contacts",
        "list_discovered_peers",
        "my_addresses",
        "my_identity",
        "my_invite_link",
        "lock_state",
        "pending_fingerprint",
        "party_saved",
        "party_list",
    ] {
        h.ipc_binds(cmd, json!({})).unwrap_or_else(|e| {
            panic!("no-arg command `{cmd}` failed: {e}");
        });
    }
}

/// Message text comes from the peer, so `open_url` must launch nothing but
/// plain web URLs — `file:`, `smb:` and custom app schemes have to be refused
/// before anything reaches the OS. Only the rejected half is exercised here:
/// a valid https URL would spawn a real browser under the test runner.
#[test]
fn open_url_rejects_non_web_schemes() {
    let h = ready_harness();
    for url in [
        "file:///etc/passwd",
        "smb://host/share",
        "javascript:alert(1)",
        "p2pem://whatever",
        "HTTPS-but-not-really://example.com",
    ] {
        let err = h
            .ipc("open_url", json!({ "url": url }))
            .expect_err(&format!("`{url}` was accepted"));
        assert_eq!(err_text(&err), "Only http(s) links can be opened");
    }
}

// ── Settings ────────────────────────────────────────────────────────────────

#[test]
fn settings_roundtrip_and_validation() {
    let h = ready_harness();
    let update = json!({ "settings": {
        "enable_notifications": false,
        "enable_typing_indicators": false,
        "auto_host_on_startup": true,
        "listen_port": 23456,
        "enable_upnp": false,
        "auto_accept_files": true,
        "auto_connect": true,
        "enable_mdns": true,
    }});
    h.ipc("update_settings", update).expect("update_settings");
    let s = h.ipc("get_settings", json!({})).unwrap();
    assert_eq!(s["enable_notifications"], false);
    assert_eq!(s["enable_typing_indicators"], false);
    assert_eq!(s["auto_host_on_startup"], true);
    assert_eq!(s["listen_port"], 23456);
    assert_eq!(s["auto_accept_files"], true);
    assert_eq!(s["auto_connect"], true);
    assert_eq!(s["enable_mdns"], true);

    // Port 0 is rejected and must not clobber the stored settings.
    let bad = json!({ "settings": {
        "enable_notifications": false,
        "enable_typing_indicators": false,
        "auto_host_on_startup": true,
        "listen_port": 0,
        "enable_upnp": false,
        "auto_accept_files": true,
        "auto_connect": true,
        "enable_mdns": true,
    }});
    let err = h.ipc("update_settings", bad).expect_err("port 0 accepted");
    assert!(err_text(&err).contains("listen port"));
    let s = h.ipc("get_settings", json!({})).unwrap();
    assert_eq!(s["listen_port"], 23456);

    // PRIV-06: what notifications show. Private by default, settable, and
    // an update that does not mention it (an older frontend) leaves it alone.
    assert_eq!(s["notification_content"], "sender_only");
    let mut with = s.clone();
    with["notification_content"] = json!("hidden");
    h.ipc_binds("update_settings", json!({ "settings": with }))
        .expect("set");
    let mut without = h.ipc("get_settings", json!({})).unwrap();
    without
        .as_object_mut()
        .unwrap()
        .remove("notification_content");
    h.ipc_binds("update_settings", json!({ "settings": without }))
        .expect("update");
    assert_eq!(
        h.ipc("get_settings", json!({})).unwrap()["notification_content"],
        "hidden"
    );
}

/// Changing the password must re-encrypt the identity file and nothing else.
/// The history key is derived from the private key, which a rotation does not
/// touch — so if this ever started losing history it would be silent and total.
#[test]
fn change_password_rotates_the_file_and_keeps_history_readable() {
    let h = ready_harness();
    let id = h.seed_chat_with_incoming("Alice", 2);
    let before = h
        .ipc("get_conversation", json!({ "id": id.to_string() }))
        .expect("conversation before the change");

    let wrong = h
        .ipc(
            "change_password",
            json!({ "current": "not-the-password", "new": "bridge-test-passphrase-2" }),
        )
        .expect_err("a wrong current password must be refused");
    assert!(
        err_text(&wrong).contains("Current password is incorrect"),
        "got: {}",
        err_text(&wrong)
    );

    let too_short = h
        .ipc(
            "change_password",
            json!({ "current": "bridge-test-passphrase", "new": "short" }),
        )
        .expect_err("the length floor applies to a changed password too");
    assert!(
        err_text(&too_short).contains("at least"),
        "got: {}",
        err_text(&too_short)
    );

    h.ipc(
        "change_password",
        json!({ "current": "bridge-test-passphrase", "new": "bridge-test-passphrase-2" }),
    )
    .expect("change_password");

    // Still unlocked, same identity, same conversation.
    let status = h.ipc("auth_status", json!({})).unwrap();
    assert_eq!(status["state"], "ready");
    let after = h
        .ipc("get_conversation", json!({ "id": id.to_string() }))
        .expect("conversation after the change");
    assert_eq!(before, after, "a password change must not touch history");

    // The rotation reached the shared identity, not just a temporary copy: the
    // old password is no longer the current one. (That the *new* one opens the
    // re-wrapped file is proved in core's `identity` tests, which do not pay for
    // a Tauri harness — Argon2 at 64 MiB is ~1s per call.)
    h.ipc(
        "change_password",
        json!({ "current": "bridge-test-passphrase", "new": "bridge-test-passphrase-3" }),
    )
    .expect_err("the old password must no longer be accepted");
}

#[test]
fn display_name_changes_are_persisted_and_validated() {
    let h = ready_harness();
    let status = h
        .ipc("set_display_name", json!({ "name": "  Alice  " }))
        .expect("set_display_name");
    assert_eq!(status["name"], "Alice", "name should be trimmed");
    assert_eq!(h.ipc("my_identity", json!({})).unwrap()["name"], "Alice");

    // Whitespace-only names are refused and leave the identity untouched.
    let err = h
        .ipc("set_display_name", json!({ "name": "   " }))
        .expect_err("blank name accepted");
    assert!(!err_text(&err).is_empty());
    assert_eq!(h.ipc("my_identity", json!({})).unwrap()["name"], "Alice");
}

// ── Hosting & conversation lock ─────────────────────────────────────────────

#[test]
fn lock_stops_hosting_and_unlock_allows_rehost() {
    let h = ready_harness();
    // Port 0 → the OS picks a free port; keeps the test parallel-safe.
    h.ipc("start_host", json!({ "port": 0, "password": null }))
        .expect("start_host");
    let addr = h.ipc("my_addresses", json!({})).unwrap();
    assert_eq!(addr["hosting"], true);
    assert_eq!(
        h.ipc("lock_state", json!({})).unwrap(),
        false,
        "fresh session must start unlocked"
    );

    h.ipc("set_locked", json!({ "locked": true }))
        .expect("set_locked");
    assert_eq!(h.ipc("lock_state", json!({})).unwrap(), true);
    let addr = h.ipc("my_addresses", json!({})).unwrap();
    assert_eq!(
        addr["hosting"], false,
        "locking must stop the live listener"
    );

    h.ipc("set_locked", json!({ "locked": false }))
        .expect("set_locked off");
    assert_eq!(h.ipc("lock_state", json!({})).unwrap(), false);
}

// ── TOFU surface ────────────────────────────────────────────────────────────

#[test]
fn no_pending_fingerprint_on_fresh_session() {
    let h = ready_harness();
    assert_eq!(
        h.ipc("pending_fingerprint", json!({})).unwrap(),
        Value::Null
    );
    // Confirming a session that doesn't exist must fail loudly, not silently
    // trust anything.
    let bogus = uuid::Uuid::new_v4().to_string();
    h.ipc(
        "confirm_fingerprint",
        json!({ "id": bogus, "accept": true }),
    )
    .expect_err("confirmed a fingerprint with no pending session");
}

// ── Invites & contacts ──────────────────────────────────────────────────────

#[test]
fn invite_link_roundtrip_creates_contact() {
    let h = ready_harness();

    // A link from somebody else imports as a contact.
    let peer = messenger_core::identity::Identity::new_with_plaintext("Peer".to_string())
        .expect("peer identity");
    let link = peer
        .generate_signed_invite_link_with_addresses(vec!["10.0.0.2:12345".to_string()], None, None)
        .expect("peer invite");

    let imported = h
        .ipc("import_invite", json!({ "link": link.clone() }))
        .expect("import a peer's invite");
    assert_eq!(imported["contact"]["name"], "Peer");
    assert_eq!(imported["signed"], true);
    // An invite carries a fingerprint but no verification: the contact is
    // Unverified until the safety code is compared on first connection.
    assert_eq!(imported["contact"]["trust"], "unverified");
    let contacts = h.ipc("list_contacts", json!({})).unwrap();
    assert_eq!(contacts.as_array().map(Vec::len), Some(1));

    // Importing the same link again refreshes that contact rather than adding
    // a duplicate card for the same peer.
    h.ipc("import_invite", json!({ "link": link }))
        .expect("re-import is not an error");
    let contacts = h.ipc("list_contacts", json!({})).unwrap();
    assert_eq!(contacts.as_array().map(Vec::len), Some(1));

    // Your own link is not a contact.
    let mine = h.ipc("my_invite_link", json!({})).expect("my_invite_link");
    let mine = mine.as_str().expect("invite link is a string").to_string();
    assert!(!mine.is_empty());
    h.ipc("import_invite", json!({ "link": mine }))
        .expect_err("imported my own invite link as a contact");

    // Garbage links are rejected with a real error, not a panic.
    h.ipc("import_invite", json!({ "link": "p2pem://garbage" }))
        .expect_err("imported a garbage link");
}

/// TAURI-18: the invite advertised the *settings* port while the app was
/// listening on another — a host started on a free port, or one the user
/// picked — so every peer who used the link dialled a closed port. A live
/// listener's port is what goes in the link.
#[test]
fn the_invite_advertises_the_port_actually_listening() {
    let h = ready_harness();
    assert!(
        messenger_core::util::primary_local_ipv4().is_some(),
        "this test needs a LAN address to put in the invite"
    );
    h.ipc("start_host", json!({ "port": 0, "password": null }))
        .expect("host on a free port");
    let manager = h.bridge().manager.clone();
    let (live, configured) = tauri::async_runtime::block_on(async {
        let m = manager.lock().await;
        (m.hosting_port.expect("listening"), m.config.listen_port)
    });
    assert_ne!(live, configured, "the test needs the two ports to differ");
    let link = h.ipc("my_invite_link", json!({})).unwrap();
    let contact = tauri::async_runtime::block_on(async {
        manager
            .lock()
            .await
            .parse_invite_link(link.as_str().unwrap())
            .unwrap()
    });
    let advertised: Vec<String> = contact
        .address
        .into_iter()
        .chain(contact.addresses)
        .collect();
    assert!(
        advertised.iter().any(|a| a.ends_with(&format!(":{live}"))),
        "the live port {live} is not in {advertised:?}"
    );
    assert!(
        !advertised
            .iter()
            .any(|a| a.ends_with(&format!(":{configured}"))),
        "the settings port {configured} is advertised: {advertised:?}"
    );
}

// ── Conversations ───────────────────────────────────────────────────────────

#[test]
fn conversation_commands_reject_unknown_ids() {
    let h = ready_harness();
    let list = h.ipc("list_conversations", json!({})).unwrap();
    assert_eq!(list.as_array().map(Vec::len), Some(0));

    let bogus = uuid::Uuid::new_v4().to_string();
    let err = h
        .ipc("get_conversation", json!({ "id": bogus }))
        .expect_err("got a nonexistent conversation");
    assert_eq!(err_text(&err), "No such conversation");
    // send_message with no live session MUST fail. It used to return Ok and
    // only push a toast, which the frontend reads as "sent": it cleared the
    // composer and added no message row, so the user's text was destroyed with
    // a four-second toast as its only trace.
    let err = h
        .ipc("send_message", json!({ "id": bogus, "text": "hi" }))
        .expect_err("a send with no session must not report success");
    assert!(
        err_text(&err).to_lowercase().contains("not connected"),
        "the error must say why nothing was sent, got: {}",
        err_text(&err)
    );
    h.ipc("accept_transfer", json!({ "id": bogus }))
        .expect_err("accepted a nonexistent transfer");
    h.ipc("decline_transfer", json!({ "id": bogus }))
        .expect_err("declined a nonexistent transfer");

    // Malformed UUIDs are a parse error, never a panic.
    h.ipc("get_conversation", json!({ "id": "not-a-uuid" }))
        .expect_err("parsed a malformed uuid");
}

// ── Unread accounting ───────────────────────────────────────────────────────

/// The list must report messages the user has never seen as unread, and only
/// stop doing so once the conversation is explicitly marked read. The previous
/// frontend derived this from the message count at first load, which silently
/// marked everything that arrived while the app was closed as already read.
#[test]
fn unread_reflects_the_persisted_read_mark() {
    let h = ready_harness();
    let id = h.seed_chat_with_incoming("Alice", 3);
    assert_eq!(h.unread_of(id), 3, "unseen peer messages must badge");

    h.ipc("mark_read", json!({ "id": id.to_string() }))
        .expect("mark_read");
    assert_eq!(h.unread_of(id), 0);

    // A later arrival badges again, from the mark — not from zero.
    let bridge_manager = {
        use tauri::Manager;
        h._app.state::<Bridge>().manager.clone()
    };
    tauri::async_runtime::block_on(async {
        let mut mgr = bridge_manager.lock().await;
        if let Some(chat) = mgr.get_chat_mut(id) {
            chat.messages.push(messenger_core::types::Message {
                id: uuid::Uuid::new_v4(),
                from_me: false,
                content: messenger_core::types::MessageContent::Text {
                    text: "later".to_string(),
                },
                timestamp: chrono::Utc::now(),
                delivered: false,
            });
        }
    });
    assert_eq!(h.unread_of(id), 1);
}

/// Marking an unknown conversation read is a no-op, not an error: the shell can
/// race a deletion, and that must not surface as a failure to the user.
#[test]
fn mark_read_tolerates_unknown_and_malformed_ids() {
    let h = ready_harness();
    h.ipc(
        "mark_read",
        json!({ "id": uuid::Uuid::new_v4().to_string() }),
    )
    .expect("unknown id should be a no-op");
    h.ipc("mark_read", json!({ "id": "not-a-uuid" }))
        .expect_err("malformed id should be rejected");
}

/// Presence is best-effort telemetry from the shell, so a malformed chat id
/// must degrade to "nothing open" rather than fail the call.
#[test]
fn set_presence_accepts_partial_information() {
    let h = ready_harness();
    let id = uuid::Uuid::new_v4().to_string();
    h.ipc("set_presence", json!({ "focused": true, "chat": id }))
        .expect("focused with a chat");
    h.ipc("set_presence", json!({ "focused": false, "chat": null }))
        .expect("blurred with nothing open");
    h.ipc(
        "set_presence",
        json!({ "focused": true, "chat": "garbage" }),
    )
    .expect("a malformed chat id must not fail the call");
}

// ── Auth re-entry (TAURI-02) ────────────────────────────────────────────────

/// `set_password` is for first setup only. On a ready app the plaintext key
/// is in memory, so without a guard it re-wrapped `identity.json` under any
/// password the caller chose — no current password needed. Anything that can
/// invoke a command (a script injected into the webview) could lock the owner
/// out of their identity, and with it their whole history. Changing the
/// password is `change_password`, which asks for the current one.
#[test]
fn set_password_cannot_rekey_an_identity_that_already_has_one() {
    let h = ready_harness();
    let path = h._dir.path().join("identity.json");
    let before = std::fs::read(&path).expect("identity saved");

    let err = h
        .ipc(
            "set_password",
            json!({ "password": "attacker-chosen-pass" }),
        )
        .expect_err("a second set_password re-keyed the identity");
    assert!(
        err_text(&err).to_lowercase().contains("already"),
        "the refusal should say a password is already set: {}",
        err_text(&err)
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "identity.json was rewritten"
    );

    let mut on_disk = Identity::load(&path).expect("load");
    assert!(on_disk.decrypt("attacker-chosen-pass").is_err());
    on_disk
        .decrypt("bridge-test-passphrase")
        .expect("the owner's password still opens it");
}

/// A second `unlock` on an unlocked app is a no-op. It used to re-run Argon2
/// and reload the history file over the running state — `apply_history`
/// *replaces*, so whatever had not been saved yet (saves are coalesced) was
/// dropped while its sessions stayed live. A double-submitted form does it.
#[test]
fn a_second_unlock_does_not_reload_history_over_unsaved_state() {
    let h = ready_harness();
    // A history already on disk (a rename saves immediately)…
    let saved = h.seed_chat_with_incoming("Saved", 0);
    h.ipc(
        "rename_chat",
        json!({ "id": saved.to_string(), "title": "Saved" }),
    )
    .expect("rename");
    assert!(
        h.bridge_history_path().exists(),
        "the rename wrote the history"
    );
    // …and a conversation newer than it, not yet written.
    let chat = h.seed_chat_with_incoming("Unsaved", 1);
    h.ipc("unlock", json!({ "password": "bridge-test-passphrase" }))
        .expect("unlocking an unlocked app is not an error");
    let conversations = h.ipc("list_conversations", json!({})).unwrap();
    assert!(
        conversations
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == chat.to_string()),
        "the unsaved conversation was dropped by a re-entrant unlock"
    );
}

// ── Files (TAURI-27) ────────────────────────────────────────────────────────

impl Harness {
    /// Seed a file message whose bytes are at `path`, as received from the
    /// peer (`from_me == false`) or as sent by the user.
    fn seed_file_message(&self, path: &std::path::Path, from_me: bool) -> (uuid::Uuid, uuid::Uuid) {
        use tauri::Manager;
        let chat = self.seed_chat_with_incoming("Files", 0);
        let msg = uuid::Uuid::new_v4();
        let bridge = self._app.state::<Bridge>();
        let manager = bridge.manager.clone();
        let path = path.to_path_buf();
        tauri::async_runtime::block_on(async move {
            let mut mgr = manager.lock().await;
            let chat = mgr.get_chat_mut(chat).unwrap();
            chat.messages.push(messenger_core::types::Message {
                id: msg,
                from_me,
                content: messenger_core::types::MessageContent::File {
                    filename: path.file_name().unwrap().to_string_lossy().into(),
                    size: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                    path: Some(path),
                },
                timestamp: chrono::Utc::now(),
                delivered: true,
            });
        });
        (chat, msg)
    }
}

/// "Open" on a file received from a peer that would *execute* is held back
/// until the user confirms: on a chat file card, a click runs attacker-chosen
/// code. Only the refusal is exercised — the paths that really open a file
/// would launch a program on the test machine.
#[test]
fn opening_a_received_executable_needs_confirmation() {
    let _serial = opened_test();
    let h = ready_harness();
    let files = tempfile::tempdir().unwrap();
    // `runs`: the OS would execute it (the warning says so). Otherwise the
    // type is simply not one known to be inert — macro documents, archives,
    // HTML, disk images and anything unnamed can all run code, but saying
    // "this is a program" about a .docx would be false (TAURI-06).
    for (name, kind, runs) in [
        ("setup.exe", "exe", true),
        ("holiday.JPG.Lnk", "lnk", true),
        ("launcher.desktop", "desktop", true),
        ("disk.iso", "iso", true),
        ("budget.xlsm", "xlsm", true),
        ("page.html", "html", true),
        ("vector.svg", "svg", true),
        ("Open me.terminal", "terminal", true),
        ("report.docx", "docx", false),
        ("archive.zip", "zip", false),
        ("README", "", false),
    ] {
        let path = files.path().join(name);
        std::fs::write(&path, b"payload").unwrap();
        let (chat, msg) = h.seed_file_message(&path, false);
        let outcome = h
            .ipc_binds(
                "open_file",
                json!({ "id": chat.to_string(), "msg": msg.to_string(), "reveal": false }),
            )
            .expect("a held-back open is an outcome, not an error");
        assert_eq!(outcome["opened"], false, "{name} was opened without asking");
        assert_eq!(outcome["blocked"], kind, "{name}");
        assert_eq!(outcome["runs"], runs, "{name}");
        assert_eq!(outcome["filename"], name);
    }
    assert!(opened::take().is_empty(), "a held-back file reached the OS");

    // Known-inert content opens on click; a confirmed risky one opens too;
    // revealing never asks; the user's own sends are not gated.
    let mut expect = Vec::new();
    for name in [
        "photo.JPG",
        "report.pdf",
        "notes.txt",
        "song.mp3",
        "clip.mp4",
    ] {
        let path = files.path().join(name);
        std::fs::write(&path, b"content").unwrap();
        let (chat, msg) = h.seed_file_message(&path, false);
        let outcome = h
            .ipc(
                "open_file",
                json!({ "id": chat.to_string(), "msg": msg.to_string(), "reveal": false }),
            )
            .unwrap();
        assert_eq!(outcome["opened"], true, "{name} was held back");
        expect.push((path, false));
    }
    let risky = files.path().join("setup.exe");
    let (chat, msg) = h.seed_file_message(&risky, false);
    for (reveal, confirm) in [(false, true), (true, false)] {
        let outcome = h
            .ipc(
                "open_file",
                json!({ "id": chat.to_string(), "msg": msg.to_string(), "reveal": reveal, "confirm": confirm }),
            )
            .unwrap();
        assert_eq!(outcome["opened"], true);
        expect.push((risky.clone(), reveal));
    }
    let (chat, msg) = h.seed_file_message(&risky, true);
    let outcome = h
        .ipc(
            "open_file",
            json!({ "id": chat.to_string(), "msg": msg.to_string(), "reveal": false }),
        )
        .unwrap();
    assert_eq!(outcome["opened"], true, "the user's own send was gated");
    expect.push((risky.clone(), false));
    assert_eq!(opened::take(), expect);

    // On Unix the exec bit decides, whatever the name.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = files.path().join("notes");
        std::fs::write(&path, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (chat, msg) = h.seed_file_message(&path, false);
        let outcome = h
            .ipc(
                "open_file",
                json!({ "id": chat.to_string(), "msg": msg.to_string(), "reveal": false }),
            )
            .unwrap();
        assert_eq!(outcome["blocked"], "executable");
    }

    // A file that is gone is an error that says so, not a silent no-op.
    let gone = files.path().join("gone.pdf");
    std::fs::write(&gone, b"x").unwrap();
    let (chat, msg) = h.seed_file_message(&gone, false);
    std::fs::remove_file(&gone).unwrap();
    let err = h
        .ipc(
            "open_file",
            json!({ "id": chat.to_string(), "msg": msg.to_string(), "reveal": false }),
        )
        .unwrap_err();
    assert!(
        err_text(&err).contains("no longer exists"),
        "{}",
        err_text(&err)
    );

    // A text message is not a file.
    let (chat, _) = h.seed_file_message(&files.path().join("setup.exe"), false);
    let text = h.seed_chat_with_incoming("Text", 1);
    let _ = chat;
    let err = h
        .ipc("open_file", json!({ "id": text.to_string(), "msg": uuid::Uuid::new_v4().to_string(), "reveal": false }))
        .unwrap_err();
    assert_eq!(err_text(&err), "No such message");
}

/// Inline previews: a small raster image comes back as a data URL; SVG (it
/// can carry script, and the file came from a peer), non-images, and images
/// over the 4 MiB cap come back as `None`.
#[test]
fn file_previews_are_raster_images_under_the_cap_only() {
    let h = ready_harness();
    let files = tempfile::tempdir().unwrap();
    let preview = |name: &str, bytes: &[u8]| {
        let path = files.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        let (chat, msg) = h.seed_file_message(&path, false);
        h.ipc_binds(
            "file_preview",
            json!({ "id": chat.to_string(), "msg": msg.to_string() }),
        )
        .expect("preview")
    };

    let png = b"\x89PNG\r\n\x1a\nnot really an image but small";
    let url = preview("photo.png", png);
    use base64::Engine;
    assert_eq!(
        url.as_str().unwrap(),
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(png)
        )
    );
    assert_eq!(
        preview("drawing.svg", b"<svg onload=alert(1)/>"),
        Value::Null
    );
    assert_eq!(preview("notes.txt", b"hello"), Value::Null);
    assert_eq!(
        preview("huge.png", &vec![0u8; 4 * 1024 * 1024 + 1]),
        Value::Null,
        "an image over the cap would balloon the webview"
    );
}

// ── Communities, end to end (TAURI-01 / TAURI-27) ───────────────────────────

/// A real community server on a loopback port, on the app's own runtime.
struct PartyServer {
    address: String,
    state: Arc<Mutex<messenger_server::state::PartyState>>,
    _dir: tempfile::TempDir,
}

fn start_party_server(password: Option<&str>) -> PartyServer {
    let dir = tempfile::tempdir().unwrap();
    let state = messenger_server::state::PartyState::load(
        "Bridge Test Community",
        password.map(str::to_owned),
        dir.path(),
    )
    .unwrap();
    let state = Arc::new(Mutex::new(state));
    let served = state.clone();
    let address = tauri::async_runtime::block_on(async move {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let key = messenger_core::core::generate_rsa_keypair(messenger_core::RSA_KEY_BITS).unwrap();
        tauri::async_runtime::spawn(messenger_server::run_accept_loop(
            listener,
            served,
            Arc::new(key),
            Arc::new(messenger_server::hub::Hub::new()),
        ));
        address
    });
    PartyServer {
        address,
        state,
        _dir: dir,
    }
}

impl Harness {
    fn bridge(&self) -> tauri::State<'_, Bridge> {
        use tauri::Manager;
        self._app.state::<Bridge>()
    }

    /// Drain community events (what the poll loop does) until `done`, or fail.
    fn pump_party(&self, label: &str, mut done: impl FnMut(&Harness) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let party = self.bridge().party.clone();
            tauri::async_runtime::block_on(async move { party.lock().await.poll_events() });
            if done(self) {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "[{label}] timed out");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    fn party_server(&self, sid: &str) -> Option<Value> {
        self.ipc("party_list", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == sid)
            .cloned()
    }

    /// What the poll loop does on a tick: record a community once it is joined.
    fn poll_parties_once(&self) {
        let ctx = crate::PollContext::from_bridge(&self.bridge());
        let mut recorded = std::collections::HashSet::new();
        tauri::async_runtime::block_on(ctx.poll_parties(&mut recorded));
    }
}

/// The join is two steps and nothing moves before the second. Step one
/// returns the server's fingerprint and SAS and has joined nobody; a second
/// call with a fingerprint the user did NOT verify is refused before the
/// `Join` frame (username + password) goes out; only the verified one joins.
/// The pin is written by the poll loop once the server has accepted us —
/// never at connect time.
#[test]
fn party_join_is_two_steps_and_pins_only_after_a_real_join() {
    let h = ready_harness();
    let srv = start_party_server(Some("community-pass"));
    let join = |fingerprint: Value| {
        h.ipc_binds(
            "party_join",
            json!({ "address": srv.address, "username": "alice",
                    "password": "community-pass", "fingerprint": fingerprint }),
        )
    };
    let members =
        || tauri::async_runtime::block_on(async { srv.state.lock().await.members().len() });

    let step1 = join(Value::Null).expect("first contact");
    assert_eq!(step1["status"], "verify");
    let fingerprint = step1["fingerprint"].as_str().unwrap().to_string();
    assert_eq!(fingerprint.len(), 64);
    assert!(!step1["sas"].as_str().unwrap_or_default().is_empty());
    assert_eq!(members(), 0, "step one joined somebody");

    let err = join(json!("ff".repeat(32))).expect_err("an unverified key was joined");
    assert!(
        err_text(&err).contains("not the one you verified"),
        "{}",
        err_text(&err)
    );
    assert_eq!(
        members(),
        0,
        "credentials reached a server the user did not verify"
    );

    let step2 = join(json!(fingerprint)).expect("join with the verified key");
    assert_eq!(step2["status"], "joined");
    let sid = step2["server"].as_str().unwrap().to_string();
    assert_eq!(
        h.ipc("party_saved", json!({})).unwrap(),
        json!([]),
        "pinned before joining"
    );

    h.pump_party("joined", |h| {
        h.party_server(&sid)
            .is_some_and(|s| s["status"] == "joined")
    });
    h.poll_parties_once();
    let saved = h.ipc("party_saved", json!({})).unwrap();
    assert_eq!(
        saved[0]["fingerprint"],
        fingerprint.as_str(),
        "the joined server was not pinned"
    );
    assert_eq!(saved[0]["address"], srv.address.as_str());

    // Post and read back through the commands the Communities pane uses.
    let server = h.party_server(&sid).unwrap();
    let channel = server["channels"][0]["id"].as_str().unwrap().to_string();
    h.ipc_binds(
        "party_post",
        json!({ "server": sid, "channel": channel, "text": "hello" }),
    )
    .expect("post");
    h.pump_party("post stored", |h| {
        h.ipc(
            "party_history",
            json!({ "server": sid, "channel": channel }),
        )
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["text"] == "hello")
    });

    // Leaving removes the server and its saved entry.
    h.ipc_binds("party_leave", json!({ "server": sid }))
        .expect("leave");
    assert!(h.party_server(&sid).is_none());
    assert_eq!(
        h.ipc("party_saved", json!({})).unwrap(),
        json!([]),
        "leaving kept the pin"
    );
    let err = h.ipc("party_leave", json!({ "server": sid })).unwrap_err();
    assert_eq!(err_text(&err), "unknown server");
}

/// A wrong password is a rejection the pane can show — and a community the
/// user never got into is not pinned.
/// TAURI-03: hosting and dialling shared one password field. Dialling with
/// no password cleared it, so the next auto-rehost listened with none — the
/// host's protection gone without a word — and the hosting password was the
/// one offered to every host dialled. Each now has its own.
#[test]
fn dialling_without_a_password_leaves_the_host_s_in_place() {
    let h = ready_harness();
    h.ipc_binds(
        "start_host",
        json!({ "port": 0, "password": "the-host-password" }),
    )
    .expect("host");
    let dead = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let _ = h.ipc_binds(
        "connect_peer",
        json!({ "host": "127.0.0.1", "port": dead, "password": null }),
    );
    let manager = h.bridge().manager.clone();
    let (host_pw, dial_pw) = tauri::async_runtime::block_on(async move {
        let m = manager.lock().await;
        (m.connection_password.clone(), m.dial_password.clone())
    });
    assert_eq!(
        host_pw.as_deref(),
        Some("the-host-password"),
        "the host lost its password"
    );
    assert_eq!(
        dial_pw, None,
        "the hosting password would be offered to the dialled host"
    );
}

/// TAURI-04: `party_join` held the community lock across the connect and
/// handshake. The poll loop takes that lock first every tick, so while a join
/// waited on an address that never answers — up to `PARTY_CONNECT_TIMEOUT` —
/// no session event, toast, save or community update went anywhere. The dial
/// runs without the lock now; only attaching the result takes it.
#[test]
fn a_join_waiting_on_a_silent_server_does_not_hold_the_community_lock() {
    let h = ready_harness();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    // Accepts, then says nothing.
    std::thread::spawn(move || {
        let _held: Vec<_> = listener.incoming().take(1).collect();
        std::thread::sleep(std::time::Duration::from_secs(60));
    });
    // Watch the lock while the join waits on the test thread.
    let party = h.bridge().party.clone();
    let watcher = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if party.try_lock().is_ok() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        false
    });
    let res = h.ipc(
        "party_join",
        json!({ "address": address, "username": "u", "password": "", "fingerprint": null }),
    );
    assert!(res.is_err(), "a silent server cannot be joined");
    assert!(
        watcher.join().unwrap(),
        "the community lock was held for the whole dial"
    );
}

#[test]
fn a_rejected_party_join_is_reported_and_never_pinned() {
    let h = ready_harness();
    let srv = start_party_server(Some("community-pass"));
    let args = |fingerprint: Value| {
        json!({ "address": srv.address, "username": "alice",
                "password": "not-the-password", "fingerprint": fingerprint })
    };
    let fp = h.ipc("party_join", args(Value::Null)).unwrap()["fingerprint"].clone();
    let sid = h.ipc("party_join", args(fp)).unwrap()["server"]
        .as_str()
        .unwrap()
        .to_string();
    h.pump_party("rejected", |h| {
        h.party_server(&sid)
            .is_some_and(|s| s["status"] == "rejected")
    });
    let detail = h.party_server(&sid).unwrap()["status_detail"].clone();
    assert!(
        detail
            .as_str()
            .unwrap_or_default()
            .to_lowercase()
            .contains("password"),
        "{detail}"
    );
    h.poll_parties_once();
    assert_eq!(h.ipc("party_saved", json!({})).unwrap(), json!([]));
}

/// The halves of `party_send_file` / `party_download_file` that the native
/// dialogs wrap: an upload is refused on size before it is read, and a
/// download returns exactly the stored bytes — or the server's refusal.
#[test]
fn community_file_upload_and_download_cores() {
    use crate::commands::party::{fetch_party_file, read_upload};
    let h = ready_harness();
    let srv = start_party_server(None);
    let fp = h
        .ipc("party_join", json!({ "address": srv.address, "username": "bob", "password": "", "fingerprint": null }))
        .unwrap()["fingerprint"]
        .clone();
    let sid = h
        .ipc(
            "party_join",
            json!({ "address": srv.address, "username": "bob", "password": "", "fingerprint": fp }),
        )
        .unwrap()["server"]
        .as_str()
        .unwrap()
        .to_string();
    h.pump_party("joined", |h| {
        h.party_server(&sid)
            .is_some_and(|s| s["status"] == "joined")
    });
    let channel = h.party_server(&sid).unwrap()["channels"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let files = tempfile::tempdir().unwrap();
    // Over the cap: refused from the metadata, without reading a byte.
    let huge = files.path().join("huge.bin");
    std::fs::File::create(&huge)
        .unwrap()
        .set_len(messenger_core::party::MAX_PARTY_FILE_BYTES + 1)
        .unwrap();
    let err = tauri::async_runtime::block_on(read_upload(&huge)).unwrap_err();
    assert!(err.contains("limited to"), "{err}");

    let notes = files.path().join("notes.txt");
    let bytes: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&notes, &bytes).unwrap();
    let (name, mime, data) = tauri::async_runtime::block_on(read_upload(&notes)).unwrap();
    assert_eq!((name.as_str(), mime.as_str()), ("notes.txt", "text/plain"));

    let party = h.bridge().party.clone();
    let (sid_u, cid_u) = (
        uuid::Uuid::parse_str(&sid).unwrap(),
        uuid::Uuid::parse_str(&channel).unwrap(),
    );
    tauri::async_runtime::block_on(async {
        party
            .lock()
            .await
            .send_file(sid_u, cid_u, name, mime, data)
            .unwrap()
    });
    let hash = messenger_core::party::blob_hash(&bytes);
    h.pump_party("file stored", |h| {
        h.ipc(
            "party_history",
            json!({ "server": sid, "channel": channel }),
        )
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["hash"] == hash.as_str())
    });

    // The poll loop is what completes a download; run it beside the wait.
    let pumping = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let pump = {
        let (party, pumping) = (party.clone(), pumping.clone());
        std::thread::spawn(move || {
            while pumping.load(std::sync::atomic::Ordering::SeqCst) {
                let party = party.clone();
                tauri::async_runtime::block_on(async move { party.lock().await.poll_events() });
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        })
    };
    let timeout = std::time::Duration::from_secs(30);
    let got = tauri::async_runtime::block_on(fetch_party_file(&party, sid_u, hash, timeout));
    let missing =
        tauri::async_runtime::block_on(fetch_party_file(&party, sid_u, "0".repeat(64), timeout));
    pumping.store(false, std::sync::atomic::Ordering::SeqCst);
    pump.join().unwrap();
    assert_eq!(got.expect("download"), bytes);
    assert_eq!(
        missing.unwrap_err(),
        messenger_core::party::UNKNOWN_FILE_ERROR
    );
}

// ── The poll loop's steps ───────────────────────────────────────────────────

/// History writes are coalesced: the loop writes when the state changed and
/// the floor has passed, and never rewrites a state already on disk.
#[test]
fn the_poll_loop_saves_changed_history_once_and_skips_an_unchanged_one() {
    let h = ready_harness();
    let ctx = crate::PollContext::from_bridge(&h.bridge());
    let sig = |h: &Harness| {
        let manager = h.bridge().manager.clone();
        tauri::async_runtime::block_on(
            async move { crate::state_signature(&*manager.lock().await) },
        )
    };
    let long_ago = std::time::Instant::now() - crate::HISTORY_SAVE_MIN_INTERVAL;

    h.seed_chat_with_incoming("New", 1);
    let changed = sig(&h);
    assert!(tauri::async_runtime::block_on(
        ctx.save_history_if_changed(changed, long_ago)
    ));
    assert!(h.bridge_history_path().exists());
    assert!(
        !tauri::async_runtime::block_on(ctx.save_history_if_changed(changed, long_ago)),
        "an unchanged state was written again"
    );
    h.seed_chat_with_incoming("Newer", 1);
    assert!(
        !tauri::async_runtime::block_on(
            ctx.save_history_if_changed(sig(&h), std::time::Instant::now())
        ),
        "the save floor was ignored"
    );
}

/// The signature the loop saves on sees every persisted change — contacts and
/// their trust included, which it once ignored, so an imported invite or a
/// block survived only as far as the next clean shutdown.
#[test]
fn the_persisted_signature_sees_contacts_trust_and_titles() {
    let h = ready_harness();
    let manager = h.bridge().manager.clone();
    tauri::async_runtime::block_on(async move {
        let mut m = manager.lock().await;
        let mut last = crate::state_signature(&m);
        let mut moved = |m: &ChatManager, what: &str| {
            let now = crate::state_signature(m);
            assert_ne!(now, last, "{what} did not change the signature");
            last = now;
        };
        let contact = m.add_contact(
            "Ann".into(),
            Some("10.0.0.2:1".into()),
            Some("ab".repeat(32)),
            None,
        );
        moved(&m, "adding a contact");
        m.block_contact(contact).unwrap();
        moved(&m, "blocking a contact");
        let chat = uuid::Uuid::new_v4();
        m.create_local_chat_for_test(chat, "Chat".into());
        moved(&m, "a new conversation");
        m.rename_chat(chat, "Renamed".into()).unwrap();
        moved(&m, "a rename");
    });
}

/// The fingerprint prompt is published once per prompt — a closed dialog must
/// not spring back open every tick — and the queryable copy is cleared when
/// the queue drains, so nothing keeps resurfacing an answered prompt.
#[test]
fn the_poll_loop_publishes_each_fingerprint_prompt_once() {
    let h = ready_harness();
    let ctx = crate::PollContext::from_bridge(&h.bridge());
    let handle = h._app.handle().clone();
    let prompt = p2pem_classic::app::PendingFingerprint {
        fingerprint: "cd".repeat(32),
        peer_name: "Peer".into(),
        sas: "12-34-56 🐔 🍎 🌻".into(),
        session_id: uuid::Uuid::new_v4(),
    };
    let mut last = None;
    assert!(ctx.publish_fingerprint_prompt(&handle, Some(prompt.clone()), &mut last));
    assert_eq!(
        h.ipc("pending_fingerprint", json!({})).unwrap()["fingerprint"],
        prompt.fingerprint.as_str()
    );
    assert!(
        !ctx.publish_fingerprint_prompt(&handle, Some(prompt), &mut last),
        "republished"
    );
    assert!(!ctx.publish_fingerprint_prompt(&handle, None, &mut last));
    assert_eq!(
        h.ipc("pending_fingerprint", json!({})).unwrap(),
        Value::Null
    );
}

/// Auto-rehost re-opens a direct listener a peer consumed, on the port that
/// was in use, and leaves a relay rendezvous alone.
#[test]
fn the_poll_loop_rehosts_a_consumed_direct_listener_only() {
    let h = ready_harness();
    let ctx = crate::PollContext::from_bridge(&h.bridge());
    let manager = h.bridge().manager.clone();
    // A relay rendezvous: hosting, but with no direct port.
    tauri::async_runtime::block_on(async {
        let mut m = manager.lock().await;
        m.is_hosting = true;
        m.hosting_port = None;
    });
    tauri::async_runtime::block_on(ctx.auto_rehost());
    assert!(
        tauri::async_runtime::block_on(async { manager.lock().await.sessions_len() }) == 0,
        "a relay rendezvous was replaced by a TCP listener"
    );
    // A direct listener that a peer consumed.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    tauri::async_runtime::block_on(async { manager.lock().await.hosting_port = Some(port) });
    tauri::async_runtime::block_on(ctx.auto_rehost());
    let m = tauri::async_runtime::block_on(manager.lock());
    assert_eq!(m.hosting_port, Some(port), "rehosted somewhere else");
    assert!(!m.check_rehost_needed(), "the listener was not re-opened");
}

/// PERF-09: the poll loop woke four times a second for the life of the app,
/// window in front or not — for a messenger left open all day, that wakeup is
/// most of what it costs. In the background and quiet it slows to once a
/// second; anything moving (a message, a transfer, a prompt) or the window
/// coming forward puts it straight back.
#[test]
fn the_poll_loop_slows_down_only_when_backgrounded_and_quiet() {
    use crate::{poll_delay, POLL_FAST, POLL_IDLE_AFTER, POLL_SLOW};
    let just_now = std::time::Duration::ZERO;
    let long_ago = POLL_IDLE_AFTER * 3;
    assert_eq!(
        poll_delay(true, long_ago),
        POLL_FAST,
        "in front of the user"
    );
    assert_eq!(
        poll_delay(false, just_now),
        POLL_FAST,
        "something just moved"
    );
    assert_eq!(
        poll_delay(false, POLL_IDLE_AFTER - std::time::Duration::from_millis(1)),
        POLL_FAST
    );
    assert_eq!(poll_delay(false, POLL_IDLE_AFTER), POLL_SLOW);
    assert_eq!(poll_delay(false, long_ago), POLL_SLOW);
    assert!(
        POLL_SLOW <= std::time::Duration::from_secs(1),
        "a backgrounded message must still notify promptly"
    );
}

// ── Start-up (init_bridge) ──────────────────────────────────────────────────

/// Run the real start-up path over `dir` (`P2PEM_DATA_DIR`), restoring the
/// environment afterwards.
fn boot_in(dir: &std::path::Path) -> Bridge {
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            std::env::remove_var("P2PEM_DATA_DIR");
        }
    }
    let _restore = Restore;
    std::env::set_var("P2PEM_DATA_DIR", dir);
    crate::init_bridge()
}

/// The rule the whole data model rests on: `identity.json` is the only key to
/// the history, so an existing file that does not load puts the app in an
/// error state and is left exactly as it was. Generating a replacement would
/// look like a fresh install while making every stored message unreadable
/// and breaking trust with every contact. (The other bridge tests build this
/// state by hand; this runs the real start-up.)
#[test]
fn start_up_never_replaces_an_identity_it_could_not_read() {
    let dir = tempfile::tempdir().unwrap();
    let damaged = b"{ this is not an identity".to_vec();
    std::fs::write(dir.path().join("identity.json"), &damaged).unwrap();

    let bridge = boot_in(dir.path());
    assert!(
        bridge.init_error.is_some(),
        "a damaged identity booted as if fine"
    );
    assert_eq!(
        std::fs::read(dir.path().join("identity.json")).unwrap(),
        damaged,
        "the unreadable identity was replaced"
    );
    let h = harness_with(bridge, dir);
    assert_eq!(h.ipc("auth_status", json!({})).unwrap()["state"], "error");
    assert!(h
        .ipc("set_password", json!({ "password": "a-new-password-x" }))
        .is_err());
    assert!(h.ipc("list_conversations", json!({})).is_err());
}

/// A fresh directory boots into password setup, with paths inside it; an
/// existing password-protected identity boots locked, asking to unlock.
#[test]
fn start_up_resolves_the_data_dir_and_the_auth_state() {
    let fresh = tempfile::tempdir().unwrap();
    let bridge = boot_in(fresh.path());
    assert!(bridge.init_error.is_none());
    assert!(bridge.needs_password_setup());
    assert_eq!(bridge.identity_path, fresh.path().join("identity.json"));
    assert_eq!(bridge.history_path, fresh.path().join("history.json.enc"));
    assert_eq!(bridge.parties_path, fresh.path().join("parties.json"));

    let existing = tempfile::tempdir().unwrap();
    let mut identity = Identity::new_with_plaintext("Owner".into()).unwrap();
    identity.encrypt("the-owners-password").unwrap();
    identity
        .save(&existing.path().join("identity.json"))
        .unwrap();
    let bridge = boot_in(existing.path());
    assert!(bridge.init_error.is_none());
    assert!(
        !bridge.needs_password_setup(),
        "a protected identity was offered setup"
    );
    assert!(crate::lock_identity(&bridge.identity).is_locked());
    let h = harness_with(bridge, existing);
    assert_eq!(h.ipc("auth_status", json!({})).unwrap()["state"], "unlock");
}

/// TAURI-05: a second launch over the same data folder used to load the same
/// identity and history, after which each process rewrote the whole history
/// from its own copy — the last writer won and the other's messages were
/// gone. The second instance now boots into the error state and writes
/// nothing; once the first exits, the folder is usable again.
#[test]
fn a_second_instance_on_the_same_data_folder_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut identity = Identity::new_with_plaintext("Owner".into()).unwrap();
    identity.encrypt("the-owners-password").unwrap();
    identity.save(&dir.path().join("identity.json")).unwrap();
    let first = boot_in(dir.path());
    assert!(first.init_error.is_none());
    let identity_before = std::fs::read(dir.path().join("identity.json")).unwrap();

    let second = boot_in(dir.path());
    let err = second
        .init_error
        .clone()
        .expect("a second instance booted over a folder already in use");
    assert!(err.contains("already running"), "unexpected error: {err}");
    assert_eq!(
        std::fs::read(dir.path().join("identity.json")).unwrap(),
        identity_before,
        "the second instance touched the identity"
    );
    drop(second);

    // Another folder is unaffected (P2PEM_DATA_DIR test peers).
    let other = tempfile::tempdir().unwrap();
    assert!(boot_in(other.path()).init_error.is_none());

    drop(first);
    let third = boot_in(dir.path());
    assert!(
        third.init_error.is_none(),
        "the folder stayed locked after its instance exited"
    );

    // "Delete all data" still leaves nothing behind, lock file included.
    let h = harness_with(third, dir);
    h.ipc("unlock", json!({ "password": "the-owners-password" }))
        .expect("unlock");
    h.ipc(
        "delete_all_data",
        json!({ "password": "the-owners-password", "confirm": "DELETE" }),
    )
    .expect("wipe");
    assert!(
        !h._dir.path().exists(),
        "the wipe left the data folder behind: {:?}",
        std::fs::read_dir(h._dir.path())
            .map(|d| d.flatten().map(|e| e.file_name()).collect::<Vec<_>>())
    );
}

// ── Backups and diagnostics ─────────────────────────────────────────────────

/// The identity backup is the encrypted file itself, byte for byte (so it is
/// as safe as the original and opens with the same password), and making one
/// is recorded and persisted — the app nags about a missing backup from that
/// record, so it must survive a restart.
#[test]
fn an_identity_backup_is_the_encrypted_file_and_is_recorded() {
    let h = ready_harness();
    let dest = h._dir.path().join("backup.json");
    tauri::async_runtime::block_on(crate::commands::auth::backup_identity_to(
        &h.bridge(),
        &dest,
    ))
    .expect("backup");
    let original = std::fs::read(h._dir.path().join("identity.json")).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), original);
    let mut restored = Identity::load(&dest).unwrap();
    assert!(restored.is_locked());
    restored
        .decrypt("bridge-test-passphrase")
        .expect("the backup opens with the password");

    let key = crate::lock_identity(&h.bridge().identity)
        .history_key()
        .unwrap();
    let saved = p2pem_classic::app::persistence::HistoryFile::load_encrypted(
        &h.bridge_history_path(),
        &key,
    )
    .expect("the record was persisted");
    assert!(saved.config.identity_backed_up_at.is_some());
}

/// TAURI-31: nothing stopped the backup's destination being the identity
/// file itself — the save dialog opens wherever the user points it, and on
/// Linux copying a file onto itself truncates it — destroying the only key to
/// the history while recording "backed up". The source is refused as a
/// destination, through any path that names it, and nothing is recorded.
#[test]
fn backing_up_onto_the_identity_itself_is_refused() {
    let h = ready_harness();
    let identity = h._dir.path().join("identity.json");
    let before = std::fs::read(&identity).unwrap();
    for dest in [
        identity.clone(),
        h._dir.path().join(".").join("identity.json"),
    ] {
        let err = tauri::async_runtime::block_on(crate::commands::auth::backup_identity_to(
            &h.bridge(),
            &dest,
        ))
        .unwrap_err();
        assert!(err.contains("itself"), "{err}");
    }
    assert_eq!(
        std::fs::read(&identity).unwrap(),
        before,
        "the identity was damaged"
    );
    assert!(h.ipc("get_settings", json!({})).unwrap()["identity_backed_up_at"].is_null());
}

/// A desktop diagnostics bundle is support metadata only: it lands in the
/// data directory and carries no message text, no peer fingerprint, no
/// private key, and only a prefix of the user's own fingerprint.
#[test]
fn the_desktop_diagnostics_bundle_carries_no_secrets() {
    let h = ready_harness();
    let chat = h.seed_chat_with_incoming("Peer", 1);
    let peer_fp = "ef".repeat(32);
    let manager = h.bridge().manager.clone();
    tauri::async_runtime::block_on(async {
        let mut m = manager.lock().await;
        let c = m.get_chat_mut(chat).unwrap();
        c.peer_fingerprint = Some(peer_fp.clone());
        c.messages.push(messenger_core::types::Message {
            id: uuid::Uuid::new_v4(),
            from_me: true,
            content: messenger_core::types::MessageContent::Text {
                text: "the secret text".into(),
            },
            timestamp: chrono::Utc::now(),
            delivered: true,
        });
    });
    let bundle = h.ipc("export_diagnostics", json!({})).expect("export");
    let bundle = std::path::PathBuf::from(bundle.as_str().unwrap());
    assert!(bundle.starts_with(h._dir.path().join("diagnostics")));
    let mut everything = String::new();
    for f in std::fs::read_dir(&bundle).unwrap().flatten() {
        everything.push_str(&std::fs::read_to_string(f.path()).unwrap());
    }
    let own_fp = crate::lock_identity(&h.bridge().identity)
        .fingerprint
        .clone();
    for secret in [
        "the secret text",
        peer_fp.as_str(),
        own_fp.as_str(),
        "PRIVATE KEY",
    ] {
        assert!(
            !everything.contains(secret),
            "the bundle leaked {secret:.24}…"
        );
    }
    assert!(
        everything.contains(&own_fp[..16]),
        "the fingerprint prefix support needs is missing"
    );
}

/// QUAL-07 / JOURNEY-10: the shipped app logged to stdout only — which a
/// release build on Windows does not have — and its diagnostics bundle said in
/// so many words that it contained no logs. What the app logged is in the
/// bundle now, scrubbed like the terminal client's.
#[test]
fn the_diagnostics_bundle_carries_the_app_log() {
    use tracing_subscriber::layer::SubscriberExt;
    let h = ready_harness();
    let addr = "198.51.100.23";
    tracing::subscriber::with_default(
        tracing_subscriber::registry().with(crate::app_logs().clone()),
        || {
            tracing::warn!(
                peer = addr,
                error = "history save failed: disk full",
                "save failed"
            )
        },
    );
    let bundle = h.ipc("export_diagnostics", json!({})).expect("export");
    let logs = std::fs::read_to_string(
        std::path::PathBuf::from(bundle.as_str().unwrap()).join("logs.txt"),
    )
    .unwrap();
    assert!(
        logs.contains("save failed") && logs.contains("disk full"),
        "the bundle has no log: {logs:?}"
    );
    assert!(
        !logs.contains(addr),
        "an address reached the bundle: {logs}"
    );
}

// ── The core flow, over IPC ─────────────────────────────────────────────────

impl Harness {
    /// One poll-loop tick for the P2P side: drain session events, then
    /// publish the head fingerprint prompt the way the loop does.
    fn tick(&self) {
        let ctx = crate::PollContext::from_bridge(&self.bridge());
        let manager = self.bridge().manager.clone();
        let prompt = tauri::async_runtime::block_on(async move {
            let mut m = manager.lock().await;
            m.poll_session_events();
            m.pending_fingerprint().cloned()
        });
        let handle = self._app.handle().clone();
        ctx.publish_fingerprint_prompt(&handle, prompt, &mut self.last_prompt.borrow_mut());
    }
}

fn tick_both(
    a: &Harness,
    b: &Harness,
    label: &str,
    mut done: impl FnMut(&Harness, &Harness) -> bool,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        a.tick();
        b.tick();
        if done(a, b) {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "[{label}] timed out");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn texts_from_peer(h: &Harness, id: &str) -> Vec<String> {
    h.ipc("get_conversation", json!({ "id": id })).unwrap()["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["from_me"] == false)
        .filter_map(|m| {
            m.pointer("/content/text")
                .and_then(|t| t.as_str())
                .map(str::to_owned)
        })
        .collect()
}

/// Two ready apps, connected and verified through the commands the webview
/// calls: host, dial, both safety-code prompts answered. Returns the host's
/// and the client's conversation ids.
fn connected_pair() -> (Harness, Harness, String, String) {
    let host = ready_harness();
    let client = ready_harness();

    host.ipc_binds("start_host", json!({ "port": 0, "password": null }))
        .expect("host");
    let manager = host.bridge().manager.clone();
    let port = tauri::async_runtime::block_on(async move { manager.lock().await.hosting_port })
        .expect("hosting");
    client
        .ipc_binds(
            "connect_peer",
            json!({ "host": "127.0.0.1", "port": port, "password": null }),
        )
        .expect("dial");

    tick_both(&host, &client, "both prompted", |h, c| {
        !h.ipc("pending_fingerprint", json!({})).unwrap().is_null()
            && !c.ipc("pending_fingerprint", json!({})).unwrap().is_null()
    });
    for h in [&host, &client] {
        let prompt = h.ipc("pending_fingerprint", json!({})).unwrap();
        assert!(
            !prompt["sas"].as_str().unwrap().is_empty(),
            "no safety code shown"
        );
        h.ipc_binds(
            "confirm_fingerprint",
            json!({ "id": prompt["chat_id"], "accept": true }),
        )
        .expect("accept");
    }
    let connected = |h: &Harness| -> Option<String> {
        h.ipc("list_conversations", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["connected"] == true && c["placeholder"] != true)
            .and_then(|c| c["id"].as_str().map(str::to_owned))
    };
    tick_both(&host, &client, "both connected", |h, c| {
        connected(h).is_some() && connected(c).is_some()
    });
    let (host_chat, client_chat) = (connected(&host).unwrap(), connected(&client).unwrap());
    (host, client, host_chat, client_chat)
}

/// The desktop app's central path, through the commands the webview calls:
/// a verified connection, a message sent and received, and the sender's copy
/// marked delivered by the peer's receipt. Everything else in this file
/// checks commands one at a time.
#[test]
fn two_desktop_apps_connect_verify_and_exchange_a_message() {
    let (host, client, host_chat, client_chat) = connected_pair();
    client
        .ipc_binds(
            "send_message",
            json!({ "id": client_chat, "text": "hello desktop" }),
        )
        .expect("send");
    tick_both(&host, &client, "message and receipt", |h, c| {
        texts_from_peer(h, &host_chat) == ["hello desktop"]
            && c.ipc("get_conversation", json!({ "id": client_chat }))
                .unwrap()["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["from_me"] == true && m["delivered"] == true)
    });
}

/// mDNS reveals presence on the LAN, so with the setting off the command
/// starts no browsing at all and reports no peers; on, it lists what has
/// been discovered.
#[test]
fn nearby_peers_are_not_browsed_unless_the_user_opted_in() {
    let h = ready_harness();
    let off = h.ipc("list_discovered_peers", json!({})).unwrap();
    assert_eq!(off, json!({ "enabled": false, "peers": [] }));
    assert!(
        crate::lock_or_recover(&h.bridge().discovery).is_none(),
        "browsing started while off"
    );

    let manager = h.bridge().manager.clone();
    tauri::async_runtime::block_on(async move { manager.lock().await.config.enable_mdns = true });
    crate::lock_or_recover(&h.bridge().discovered).push(messenger_core::network::DiscoveredPeer {
        name: "Nearby".into(),
        address: "192.168.1.7".into(),
        port: 12345,
        fingerprint: None,
        fullname: "Nearby._p2p-messenger._tcp.local.".into(),
    });
    let on = h.ipc("list_discovered_peers", json!({})).unwrap();
    assert_eq!(on["enabled"], true);
    assert!(on["peers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["name"] == "Nearby" && p["port"] == 12345));
    assert!(crate::lock_or_recover(&h.bridge().discovery).is_some());

    // TAURI-08: turning it off used to change only the flag — the daemon kept
    // browsing and any registration kept announcing the user's name and port.
    let mut settings = h.ipc("get_settings", json!({})).unwrap();
    settings["enable_mdns"] = json!(false);
    h.ipc_binds("update_settings", json!({ "settings": settings }))
        .expect("update");
    assert!(
        crate::lock_or_recover(&h.bridge().discovery).is_none(),
        "mDNS still running after the user turned it off"
    );
    assert!(crate::lock_or_recover(&h.bridge().discovered).is_empty());
}

// ── Commands behind a native dialog ─────────────────────────────────────────
//
// `native_file_dialog` is answered from here under `cfg(test)`: each request
// is recorded, and the next scripted answer returned (`None` = cancelled, and
// also what an unscripted dialog gets). So the commands around a dialog run
// for real — what a cancel does, where a pick goes, which name a save
// suggests. The rfd call itself is the one line left that only a desktop
// session can exercise.

pub(crate) mod dialogs {
    use crate::Dialog;
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::Mutex;

    static ANSWERS: Mutex<VecDeque<Option<PathBuf>>> = Mutex::new(VecDeque::new());
    static ASKED: Mutex<Vec<Dialog>> = Mutex::new(Vec::new());
    /// nextest runs each test in its own process; this keeps plain
    /// `cargo test`, which does not, from sharing one script between tests.
    pub(super) static SERIAL: Mutex<()> = Mutex::new(());

    pub(crate) fn answer(dialog: Dialog) -> Option<PathBuf> {
        ASKED.lock().unwrap().push(dialog);
        ANSWERS.lock().unwrap().pop_front().flatten()
    }

    pub(super) fn will_answer(answer: Option<PathBuf>) {
        ANSWERS.lock().unwrap().push_back(answer);
    }

    /// Every dialog opened so far, and forget them.
    pub(super) fn asked() -> Vec<Dialog> {
        std::mem::take(&mut *ASKED.lock().unwrap())
    }

    pub(super) fn reset() {
        ANSWERS.lock().unwrap().clear();
        ASKED.lock().unwrap().clear();
    }
}

/// What `open_path_os` would have handed the OS, recorded instead of
/// launched — a test that really opened a file would start a program on the
/// machine running it.
pub(crate) mod opened {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    static OPENED: Mutex<Vec<(PathBuf, bool)>> = Mutex::new(Vec::new());
    pub(super) static SERIAL: Mutex<()> = Mutex::new(());

    pub(crate) fn record(path: &Path, reveal: bool) {
        OPENED.lock().unwrap().push((path.to_path_buf(), reveal));
    }

    pub(super) fn take() -> Vec<(PathBuf, bool)> {
        std::mem::take(&mut *OPENED.lock().unwrap())
    }
}

fn opened_test() -> std::sync::MutexGuard<'static, ()> {
    let guard = opened::SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    opened::take();
    guard
}

fn dialog_test() -> std::sync::MutexGuard<'static, ()> {
    let guard = dialogs::SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    dialogs::reset();
    guard
}

/// Cancelling is the most common answer to any dialog, and must be a quiet
/// no-op: not an error toast, not a half-done action.
#[test]
fn a_cancelled_dialog_changes_nothing() {
    use crate::Dialog;
    let _serial = dialog_test();
    let h = ready_harness();
    let before = h.ipc("get_settings", json!({})).unwrap();
    let chat = h.seed_chat_with_incoming("Peer", 0).to_string();
    let some_id = uuid::Uuid::new_v4().to_string();

    assert_eq!(
        h.ipc_binds("pick_download_dir", json!({})).unwrap(),
        Value::Null
    );
    assert_eq!(
        h.ipc_binds("export_identity", json!({})).unwrap(),
        Value::Null
    );
    assert_eq!(
        h.ipc_binds("send_file", json!({ "id": chat })).unwrap(),
        Value::Null
    );
    assert_eq!(
        h.ipc_binds(
            "party_send_file",
            json!({ "server": some_id, "channel": some_id })
        )
        .unwrap(),
        Value::Null
    );
    assert_eq!(
        h.ipc_binds(
            "party_send_file_dm",
            json!({ "server": some_id, "to": some_id })
        )
        .unwrap(),
        Value::Null
    );

    assert_eq!(
        dialogs::asked(),
        vec![
            Dialog::Folder {
                title: "Choose download folder"
            },
            Dialog::Save {
                title: "Save identity backup",
                file_name: "p2pem-identity-backup.json".into()
            },
            Dialog::Open {
                title: "Send a file"
            },
            Dialog::Open {
                title: "Share a file"
            },
            Dialog::Open {
                title: "Share a file"
            },
        ],
        "each command asks for the right kind of dialog"
    );
    assert_eq!(
        h.ipc("get_settings", json!({})).unwrap(),
        before,
        "a cancel changed settings"
    );
    assert!(h
        .ipc("list_transfers", json!({}))
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn the_chosen_folder_and_backup_location_are_used() {
    let _serial = dialog_test();
    let h = ready_harness();
    let out = tempfile::tempdir().unwrap();

    let folder = out.path().join("incoming");
    std::fs::create_dir(&folder).unwrap();
    dialogs::will_answer(Some(folder.clone()));
    assert_eq!(
        h.ipc_binds("pick_download_dir", json!({})).unwrap(),
        json!(folder.display().to_string())
    );
    assert_eq!(
        h.ipc("get_settings", json!({})).unwrap()["download_dir"],
        json!(folder.display().to_string())
    );

    let dest = out.path().join("backup.json");
    dialogs::will_answer(Some(dest.clone()));
    assert_eq!(
        h.ipc_binds("export_identity", json!({})).unwrap(),
        json!(dest.display().to_string())
    );
    assert_eq!(
        std::fs::read(&dest).unwrap(),
        std::fs::read(h._dir.path().join("identity.json")).unwrap(),
        "the backup is the (encrypted) identity file, byte for byte"
    );
    assert!(h.ipc("get_settings", json!({})).unwrap()["identity_backed_up_at"].is_string());
}

/// A file picked in the dialog reaches the peer — held for acceptance, then
/// written to the folder the peer chose, intact. The desktop file flow end to
/// end, through the commands the webview calls.
#[test]
fn a_file_picked_in_the_dialog_reaches_the_peer() {
    let _serial = dialog_test();
    let (host, client, _host_chat, client_chat) = connected_pair();
    let out = tempfile::tempdir().unwrap();
    let downloads = out.path().join("downloads");
    std::fs::create_dir(&downloads).unwrap();
    dialogs::will_answer(Some(downloads.clone()));
    host.ipc_binds("pick_download_dir", json!({})).unwrap();

    let file = out.path().join("report.bin");
    let bytes: Vec<u8> = (0..200_000u32).map(|i| (i % 253) as u8).collect();
    std::fs::write(&file, &bytes).unwrap();
    dialogs::will_answer(Some(file));
    client
        .ipc_binds("send_file", json!({ "id": client_chat }))
        .expect("send");

    let offer = |h: &Harness| {
        h.ipc("list_transfers", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["direction"] == "incoming" && t["status"] == "awaiting")
            .cloned()
    };
    tick_both(&host, &client, "offer held", |h, _| offer(h).is_some());
    let offer = offer(&host).unwrap();
    assert_eq!(offer["filename"], "report.bin");
    assert!(
        std::fs::read_dir(&downloads).unwrap().all(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("tmp_")),
        "a held offer reached the download folder before it was accepted"
    );

    host.ipc_binds("accept_transfer", json!({ "id": offer["id"] }))
        .expect("accept");
    let landed = downloads.join("report.bin");
    tick_both(&host, &client, "file written", |_, _| {
        std::fs::read(&landed).is_ok_and(|b| b == bytes)
    });
}

/// A community file is shared from the dialog and saved through it — under
/// the name the uploader chose, *sanitised before it fills the save dialog*:
/// a U+202E override makes `photo\u{202E}gnp.exe` read as `photoexe.png`.
#[test]
fn a_community_file_is_shared_and_saved_under_a_name_that_shows_what_it_is() {
    use crate::Dialog;
    let _serial = dialog_test();
    let h = ready_harness();
    let srv = start_party_server(None);
    let fp = h
        .ipc("party_join", json!({ "address": srv.address, "username": "bob", "password": "", "fingerprint": null }))
        .unwrap()["fingerprint"]
        .clone();
    let sid = h
        .ipc(
            "party_join",
            json!({ "address": srv.address, "username": "bob", "password": "", "fingerprint": fp }),
        )
        .unwrap()["server"]
        .as_str()
        .unwrap()
        .to_string();
    h.pump_party("joined", |h| {
        h.party_server(&sid)
            .is_some_and(|s| s["status"] == "joined")
    });
    let channel = h.party_server(&sid).unwrap()["channels"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let out = tempfile::tempdir().unwrap();
    let tricky = out.path().join("photo\u{202E}gnp.exe");
    let bytes = b"MZ not really a program".to_vec();
    std::fs::write(&tricky, &bytes).unwrap();
    dialogs::will_answer(Some(tricky));
    h.ipc_binds(
        "party_send_file",
        json!({ "server": sid, "channel": channel }),
    )
    .expect("share");

    let hash = messenger_core::party::blob_hash(&bytes);
    let mut stored_name = String::new();
    h.pump_party("file stored", |h| {
        let history = h
            .ipc(
                "party_history",
                json!({ "server": sid, "channel": channel }),
            )
            .unwrap();
        match history
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["hash"] == hash.as_str())
        {
            Some(m) => {
                stored_name = m["text"].as_str().unwrap_or_default().to_string();
                true
            }
            None => false,
        }
    });

    // Download it. The poll loop completes downloads, so run it alongside.
    let party = h.bridge().party.clone();
    let pumping = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let pump = {
        let (party, pumping) = (party.clone(), pumping.clone());
        std::thread::spawn(move || {
            while pumping.load(std::sync::atomic::Ordering::SeqCst) {
                let party = party.clone();
                tauri::async_runtime::block_on(async move { party.lock().await.poll_events() });
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        })
    };
    let dest = out.path().join("saved.bin");
    dialogs::will_answer(Some(dest.clone()));
    let res = h.ipc_binds(
        "party_download_file",
        json!({ "server": sid, "hash": hash, "name": stored_name }),
    );
    pumping.store(false, std::sync::atomic::Ordering::SeqCst);
    pump.join().unwrap();
    res.expect("download");
    assert_eq!(std::fs::read(&dest).unwrap(), bytes);

    let asked = dialogs::asked();
    let Some(Dialog::Save { file_name, .. }) = asked.last() else {
        panic!("no save dialog: {asked:?}");
    };
    assert!(
        !file_name.contains('\u{202E}'),
        "the save dialog was pre-filled with a direction override: {file_name:?}"
    );
    assert!(
        file_name.ends_with(".exe"),
        "sanitising must not hide what it is: {file_name:?}"
    );
}

/// PRIV-05: a release build logged at debug level by default — file names,
/// peer addresses and frame metadata on stdout, which on Linux can land in the
/// systemd journal. Only a development build does now.
#[test]
fn a_release_build_does_not_log_at_debug_level_by_default() {
    let release = crate::default_log_filter(false);
    assert!(
        !release.contains("debug") && !release.contains("trace"),
        "{release}"
    );
    assert!(tracing_subscriber::EnvFilter::try_new(release).is_ok());
    assert!(crate::default_log_filter(true).contains("debug"));
}
