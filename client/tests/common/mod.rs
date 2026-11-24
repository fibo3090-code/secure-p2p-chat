//! Setup shared by the networked integration tests: loopback `ChatManager`
//! pairs and an in-process community server joined by the real `PartyManager`.
//! Each test binary uses a different subset, hence the `dead_code` allowance.
#![allow(dead_code)]

use messenger_core::core::generate_rsa_keypair;
use messenger_core::party::MessagePayload;
use messenger_core::types::Config;
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
use tokio::sync::oneshot::{self, error::TryRecvError};
use tokio::sync::Mutex;
use uuid::Uuid;

/// How long one "wait for the other side" step may take. A hang detector, not
/// a performance bound: debug-build RSA handshakes are slow and several run at
/// once under a parallel runner, so a tight bound turns load into flakes.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(90);

// ── Two ChatManagers over loopback ──────────────────────────────────────────

/// Reserve an ephemeral loopback port, then release it for the host to bind.
pub fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    listener.local_addr().expect("local addr").port()
}

/// A manager with its own download/temp directories, so two peers in one test
/// never write over each other.
pub fn manager_in(dir: &Path, auto_accept: bool) -> ChatManager {
    let download = dir.join("downloads");
    let temp = dir.join("tmp");
    std::fs::create_dir_all(&download).expect("download dir");
    std::fs::create_dir_all(&temp).expect("temp dir");
    ChatManager::new(Config {
        download_dir: download,
        temp_dir: temp,
        auto_accept_files: auto_accept,
        ..Config::default()
    })
}

/// Drive both managers' event loops until `done`, or fail rather than hang.
/// Both sides must be pumped: a transfer only advances as the receiver drains
/// its events, and the sender's progress is mirrored by the same poll.
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

// ── A community server ──────────────────────────────────────────────────────

/// A server on an ephemeral loopback port, with durable state in a temp dir so
/// the SQLite and blob-store paths are the ones under test.
pub struct TestServer {
    pub address: String,
    /// The live state, for setup a client could not do quickly.
    pub state: Arc<Mutex<PartyState>>,
    /// The one-time owner claim token printed at startup; the operator joins
    /// with it in the password field.
    pub claim_token: Option<String>,
    _dir: tempfile::TempDir,
}

pub async fn start_server(password: Option<String>) -> TestServer {
    let dir = tempfile::tempdir().expect("server dir");
    let state = PartyState::load("Test Community", password, dir.path()).expect("load state");
    let claim_token = state.owner_claim_token().map(str::to_owned);
    let state = Arc::new(Mutex::new(state));
    let privkey = Arc::new(generate_rsa_keypair(RSA_KEY_BITS).expect("server key"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("addr").to_string();
    let served = state.clone();
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

pub fn client_key() -> RsaPrivateKey {
    generate_rsa_keypair(RSA_KEY_BITS).expect("client key")
}

/// Pump the manager until `done`, or fail rather than hang.
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

/// Poll every manager for `rounds` × `ms`, for asserting that nothing happens.
pub async fn idle(mgrs: &mut [&mut PartyManager], rounds: usize, ms: u64) {
    for _ in 0..rounds {
        for m in mgrs.iter_mut() {
            m.poll_events();
        }
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
}

/// Step one of a first join: connect without credentials and learn the
/// server's fingerprint, as the user would see it before accepting.
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

/// Connect with the server's real fingerprint pinned, expecting `Joining`
/// (whether the join is then accepted is up to the server).
pub async fn connect_pinned(
    mgr: &mut PartyManager,
    server: &TestServer,
    username: &str,
    password: Option<String>,
    key: &RsaPrivateKey,
) -> Uuid {
    let pin = learn_fingerprint(&server.address, key).await;
    match mgr
        .connect_and_join(&server.address, username, password, key, Some(&pin))
        .await
        .expect("the connection itself succeeds")
    {
        PartyJoinOutcome::Joining { server_id, .. } => server_id,
        other => panic!("expected Joining, got {other:?}"),
    }
}

/// Join `server` as `username` with a fresh key, through the two-step trust
/// decision: the first contact stops (nothing sent), the second carries the
/// verified fingerprint.
pub async fn join(
    server: &TestServer,
    username: &str,
    password: Option<String>,
) -> (PartyManager, Uuid) {
    let key = client_key();
    let mut mgr = PartyManager::new();
    let outcome = mgr
        .connect_and_join(&server.address, username, password.clone(), &key, None)
        .await
        .expect("first contact");
    let fingerprint = match outcome {
        PartyJoinOutcome::NeedsVerification { fingerprint, sas } => {
            assert!(
                !sas.is_empty(),
                "the SAS is what the user actually compares"
            );
            assert_eq!(fingerprint.len(), 64, "a full SHA-256 fingerprint");
            fingerprint
        }
        other => panic!("a first join must stop for verification, got {other:?}"),
    };

    let server_id = match mgr
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
        m.server(server_id)
            .is_some_and(|s| s.status == PartyStatus::Joined && s.member_id.is_some())
    })
    .await;
    assert_eq!(
        mgr.server(server_id).unwrap().server_fingerprint,
        fingerprint,
        "the identity that was verified must be the one that was joined"
    );
    (mgr, server_id)
}

/// Join as the community's owner, with the claim token as the password.
pub async fn join_owner(server: &TestServer, username: &str) -> (PartyManager, Uuid) {
    join(server, username, server.claim_token.clone()).await
}

pub fn channel_named(mgr: &PartyManager, server_id: Uuid, name: &str) -> Option<Uuid> {
    mgr.server(server_id)?
        .channels
        .iter()
        .find(|c| c.name == name)
        .map(|c| c.id)
}

/// Wait for the channel called `name` to be listed, and return its id.
pub async fn wait_channel(mgr: &mut PartyManager, server_id: Uuid, name: &str) -> Uuid {
    pump_until(mgr, name, |m| channel_named(m, server_id, name).is_some()).await;
    channel_named(mgr, server_id, name).unwrap()
}

pub async fn wait_members(mgr: &mut PartyManager, server_id: Uuid, n: usize) {
    pump_until(mgr, "members", |m| {
        m.server(server_id).is_some_and(|s| s.members.len() >= n)
    })
    .await;
}

pub fn member_named(mgr: &PartyManager, server_id: Uuid, name: &str) -> Option<Uuid> {
    mgr.server(server_id)?
        .members
        .iter()
        .find(|m| m.username == name)
        .map(|m| m.id)
}

pub fn has_file(mgr: &PartyManager, server_id: Uuid, name: &str) -> bool {
    mgr.server(server_id)
        .is_some_and(|s| s.files.iter().any(|f| f.name == name))
}

pub fn texts_in(mgr: &PartyManager, server_id: Uuid, channel: Uuid) -> Vec<String> {
    mgr.server(server_id)
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

/// Drive the manager until a download resolves (`None` on timeout).
pub async fn download_result(
    mgr: &mut PartyManager,
    mut rx: oneshot::Receiver<Result<Vec<u8>, String>>,
    timeout: Duration,
) -> Option<Result<Vec<u8>, String>> {
    let deadline = Instant::now() + timeout;
    loop {
        mgr.poll_events();
        match rx.try_recv() {
            Ok(result) => return Some(result),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Closed) => {
                return Some(Err(
                    "the connection dropped before the download resolved".to_string()
                ))
            }
        }
        if Instant::now() > deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
