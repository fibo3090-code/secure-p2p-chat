//! Harness shared by every end-to-end module: two real `ChatManager`s joined
//! over loopback, and a real `messenger-server` joined by real `PartyManager`s.
//!
//! These used to be copied into each test file — four `pump_until`s, three
//! `start_server`s, two `free_port`s — and the copies had drifted (one fixed
//! the bind race, the others did not). There is one of each now.

use messenger_core::core::generate_rsa_keypair;
use messenger_core::party::MessagePayload;
use messenger_core::types::{Config, MessageContent};
use messenger_core::RSA_KEY_BITS;
use messenger_server::hub::Hub;
use messenger_server::run_accept_loop;
use messenger_server::state::PartyState;
use p2pem_classic::app::party_manager::{PartyJoinOutcome, PartyManager, PartyStatus};
use p2pem_classic::app::ChatManager;
use rsa::RsaPrivateKey;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use uuid::Uuid;

/// How long any one "wait for the other side" step may take before the test
/// fails rather than hangs.
///
/// A hang detector, not a performance bound: these tests run real handshakes in
/// a debug build, several at once under a parallel runner, and a tight bound
/// turns load into a flake — which trains people to re-run instead of read.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(90);

pub fn rsa_key() -> RsaPrivateKey {
    generate_rsa_keypair(RSA_KEY_BITS).expect("rsa key")
}

// ── Peer to peer ────────────────────────────────────────────────────────────

/// A manager with its own download and temp directories, so two peers in one
/// test never write over each other.
pub fn manager_in(dir: &Path) -> ChatManager {
    let download_dir = dir.join("downloads");
    let temp_dir = dir.join("tmp");
    std::fs::create_dir_all(&download_dir).expect("download dir");
    std::fs::create_dir_all(&temp_dir).expect("temp dir");
    ChatManager::new(Config {
        download_dir,
        temp_dir,
        ..Config::default()
    })
}

/// Drive both managers' event loops until `done` holds, or fail.
///
/// Both sides have to be pumped: a transfer advances only when the receiver
/// drains its session events, and the sender's progress is mirrored out of the
/// streaming task by the same poll.
pub async fn pump_pair<F>(a: &mut ChatManager, b: &mut ChatManager, label: &str, mut done: F)
where
    F: FnMut(&mut ChatManager, &mut ChatManager) -> bool,
{
    let deadline = Instant::now() + STEP_TIMEOUT;
    loop {
        a.poll_session_events();
        b.poll_session_events();
        if done(a, b) {
            return;
        }
        assert!(Instant::now() < deadline, "[{label}] timed out");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Start hosting on an OS-assigned port and return the port actually bound.
///
/// Port 0 rather than "find a free port, release it, bind it": that pattern is
/// a race between two tests under a parallel runner, and it was the only
/// reason the nextest retry existed for these files.
pub async fn host_on_any_port(host: &mut ChatManager, key: RsaPrivateKey) -> u16 {
    host.start_host(0, key).await.expect("start host");
    host.hosting_port
        .expect("start_host records the port it bound")
}

/// Dial `host` from `client`, accept the safety-code prompt on both ends, and
/// return `(host_chat, client_chat)` once both sides have a live conversation.
///
/// The host creates a *new* chat per incoming connection, keyed by the
/// client's own chat id, so the host-side conversation is found by the
/// connection that is live — never assumed from the listener placeholder.
pub async fn dial_and_verify(
    host: &mut ChatManager,
    client: &mut ChatManager,
    port: u16,
    client_key: RsaPrivateKey,
) -> (Uuid, Uuid) {
    let client_chat = client
        .connect_to_host("127.0.0.1", port, None, client_key)
        .await
        .expect("dial");
    pump_pair(host, client, "both safety-code prompts", |h, c| {
        h.pending_fingerprint().is_some() && c.pending_fingerprint().is_some()
    })
    .await;
    // The prompt is a queue read by peeking; `confirm_fingerprint` is what
    // removes the entry and persists the trust.
    let prompt = host.pending_fingerprint().expect("host prompt").session_id;
    host.confirm_fingerprint(prompt, true)
        .expect("host accepts");
    let prompt = client
        .pending_fingerprint()
        .expect("client prompt")
        .session_id;
    client
        .confirm_fingerprint(prompt, true)
        .expect("client accepts");

    let mut host_chat = None;
    pump_pair(host, client, "both sessions live", |h, c| {
        host_chat = conversations(h).into_iter().find(|id| h.is_connected(id));
        host_chat.is_some() && c.is_connected(&client_chat)
    })
    .await;
    (host_chat.expect("host-side conversation"), client_chat)
}

/// Conversations, not listeners.
pub fn conversations(m: &ChatManager) -> Vec<Uuid> {
    m.chats
        .values()
        .filter(|c| !c.is_host_placeholder)
        .map(|c| c.id)
        .collect()
}

pub fn texts(m: &ChatManager, chat: Uuid) -> Vec<String> {
    m.chats[&chat]
        .messages
        .iter()
        .filter_map(|msg| match &msg.content {
            MessageContent::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

// ── Communities ─────────────────────────────────────────────────────────────

/// A server on an ephemeral loopback port, with durable state in a temp dir so
/// the SQLite and blob-store paths are the ones under test.
pub struct TestServer {
    pub address: String,
    /// The live server state. Negative assertions ("the guest did not create a
    /// channel") read it directly: absence observed through a client is only
    /// absence *so far*.
    pub state: Arc<Mutex<PartyState>>,
    /// The one-time owner claim token the server prints at startup. The
    /// operator joins with it in the password field.
    pub claim_token: Option<String>,
    _dir: tempfile::TempDir,
}

pub async fn start_server(password: Option<String>) -> TestServer {
    let dir = tempfile::tempdir().expect("server dir");
    let state = PartyState::load("Test Community", password, dir.path()).expect("load state");
    let claim_token = state.owner_claim_token().map(str::to_owned);
    let state = Arc::new(Mutex::new(state));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("addr").to_string();
    let served = state.clone();
    let privkey = Arc::new(rsa_key());
    tokio::spawn(async move {
        let _ = run_accept_loop(listener, served, privkey, Arc::new(Hub::new())).await;
    });
    TestServer {
        address,
        state,
        claim_token,
        _dir: dir,
    }
}

/// Pump `mgr` until `done` holds, or fail rather than hang.
pub async fn pump_until<F>(mgr: &mut PartyManager, label: &str, mut done: F)
where
    F: FnMut(&mut PartyManager) -> bool,
{
    let deadline = Instant::now() + STEP_TIMEOUT;
    loop {
        mgr.poll_events();
        if done(mgr) {
            return;
        }
        assert!(Instant::now() < deadline, "[{label}] timed out");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Step one of a first join: connect with no pin and learn the server's
/// fingerprint, as the user would see it before accepting.
pub async fn learn_fingerprint(address: &str, key: &RsaPrivateKey) -> String {
    match PartyManager::new()
        .connect_and_join(address, "probe", None, key, None)
        .await
        .expect("first contact")
    {
        PartyJoinOutcome::NeedsVerification { fingerprint, .. } => fingerprint,
        other => panic!("a first contact must stop for verification, got {other:?}"),
    }
}

/// Join `server` as `username`, through the two-step trust decision, and wait
/// until the member is in.
pub async fn join(
    server: &TestServer,
    username: &str,
    password: Option<String>,
) -> (PartyManager, Uuid) {
    let key = rsa_key();
    let mut mgr = PartyManager::new();

    // Step one: no pin, so nothing is sent but the handshake.
    let fingerprint = match mgr
        .connect_and_join(&server.address, username, password.clone(), &key, None)
        .await
        .expect("first contact")
    {
        PartyJoinOutcome::NeedsVerification { fingerprint, sas } => {
            assert!(!sas.is_empty(), "the SAS is what the user compares");
            assert_eq!(fingerprint.len(), 64, "a full SHA-256 fingerprint");
            fingerprint
        }
        other => panic!("a first join must stop for verification, got {other:?}"),
    };

    // Step two: the user has compared the code.
    let sid = match mgr
        .connect_and_join(
            &server.address,
            username,
            password,
            &key,
            Some(&fingerprint),
        )
        .await
        .expect("join after verification")
    {
        PartyJoinOutcome::Joining { server_id, .. } => server_id,
        other => panic!("expected Joining, got {other:?}"),
    };
    pump_until(&mut mgr, "joined", |m| {
        m.server(sid).is_some_and(|s| {
            s.status == PartyStatus::Joined
                && s.member_id.is_some()
                && s.channels.iter().any(|c| c.name == "general")
        })
    })
    .await;
    assert_eq!(
        mgr.server(sid).unwrap().server_fingerprint,
        fingerprint,
        "the identity that was verified must be the one that was joined"
    );
    (mgr, sid)
}

/// Join as the community's owner: with the claim token as the password.
pub async fn join_owner(server: &TestServer, username: &str) -> (PartyManager, Uuid) {
    join(server, username, server.claim_token.clone()).await
}

/// Wait until the server has answered every request `mgr` sent before this
/// call, and the client has applied those answers.
///
/// This replaces "poll for a second, then assert nothing happened", which
/// passes just as happily when the server has not got round to the request
/// yet. The server answers one connection's requests in order, so the answer
/// to a download of a hash that cannot exist is a barrier for everything
/// queued ahead of it. It runs twice because an answer can make the client
/// *send* (a `DirectoryChanged` push makes it re-fetch the channel list): the
/// first round flushes the answers, the second the requests they triggered.
pub async fn settle(mgr: &mut PartyManager, sid: Uuid) {
    for round in 0..2 {
        let mut rx = mgr
            .request_download(sid, "0".repeat(64))
            .expect("barrier request");
        let deadline = Instant::now() + STEP_TIMEOUT;
        loop {
            mgr.poll_events();
            match rx.try_recv() {
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
                Ok(Err(_)) => break,
                Ok(Ok(_)) => panic!("a file with an all-zero hash was served"),
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    panic!("the connection closed during barrier round {round}")
                }
            }
            assert!(Instant::now() < deadline, "barrier round {round} timed out");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    // The barrier's own refusal is not an error the test caused.
    mgr.clear_server_error(sid);
}

/// Drive `mgr` until a download resolves, and hand back its result.
pub async fn download(mgr: &mut PartyManager, sid: Uuid, hash: String) -> Result<Vec<u8>, String> {
    let mut rx = mgr.request_download(sid, hash).expect("request download");
    let deadline = Instant::now() + STEP_TIMEOUT;
    loop {
        mgr.poll_events();
        match rx.try_recv() {
            Ok(result) => return result,
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                return Err("the connection dropped before the download resolved".into())
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for a download"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub fn channel_named(mgr: &PartyManager, sid: Uuid, name: &str) -> Option<Uuid> {
    mgr.server(sid)?
        .channels
        .iter()
        .find(|c| c.name == name)
        .map(|c| c.id)
}

pub fn member_named(mgr: &PartyManager, sid: Uuid, name: &str) -> Option<Uuid> {
    mgr.server(sid)?
        .members
        .iter()
        .find(|m| m.username == name)
        .map(|m| m.id)
}

pub fn texts_in(mgr: &PartyManager, sid: Uuid, channel: Uuid) -> Vec<String> {
    mgr.server(sid)
        .and_then(|s| s.messages.get(&channel))
        .map(|msgs| {
            msgs.iter()
                .filter_map(|e| match &e.payload {
                    MessagePayload::Text(t) => Some(t.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}
