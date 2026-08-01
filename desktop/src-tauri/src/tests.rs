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
    Bridge {
        manager: Arc::new(Mutex::new(ChatManager::new(Config::default()))),
        party: Arc::new(Mutex::new(PartyManager::new())),
        identity: Arc::new(StdMutex::new(identity)),
        discovery: Arc::new(StdMutex::new(None)),
        discovered: Arc::new(StdMutex::new(Vec::new())),
        identity_path: history_path.with_file_name("identity.json"),
        parties_path: history_path.with_file_name("parties.json"),
        history_path,
        is_new: StdMutex::new(true),
        force_setup: StdMutex::new(true),
        pending_fp: Arc::new(StdMutex::new(None)),
        init_error: None,
        saved_sig: Arc::new(StdMutex::new(None)),
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
    bridge.is_new = StdMutex::new(false);
    bridge.force_setup = StdMutex::new(false);
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
    let link = h.ipc("my_invite_link", json!({})).expect("my_invite_link");
    let link = link.as_str().expect("invite link is a string").to_string();
    assert!(!link.is_empty());

    let contact = h
        .ipc("import_invite", json!({ "link": link }))
        .expect("import own invite");
    assert_eq!(contact["name"], "Tester");
    let contacts = h.ipc("list_contacts", json!({})).unwrap();
    assert_eq!(contacts.as_array().map(Vec::len), Some(1));

    // Garbage links are rejected with a real error, not a panic.
    h.ipc("import_invite", json!({ "link": "p2pem://garbage" }))
        .expect_err("imported a garbage link");
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
    // send_message without a live session is deliberately Ok: the manager
    // reports "not delivered / connecting" through a toast instead of an
    // error, so the UI keeps its optimistic-send flow.
    h.ipc("send_message", json!({ "id": bogus, "text": "hi" }))
        .expect("send_message surfaces session problems via toasts, not errors");
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
