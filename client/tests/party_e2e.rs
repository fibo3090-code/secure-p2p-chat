//! End-to-end Communities: a real `messenger-server` on a loopback socket,
//! joined by the real client-side `PartyManager` — the same object the desktop
//! bridge and the TUI drive.
//!
//! Communities are the most involved subsystem in the workspace, and until now
//! the only coverage was unit tests either side of the wire. A join crosses the
//! v3 handshake, the two-step trust decision, the join gate, the hub, the
//! dispatch layer, SQLite and the content-addressed blob store, and the failures
//! that matter are the ones that only appear when those run together:
//!
//! - **No credential moves before the trust step.** A first join must stop and
//!   report the fingerprint + SAS, having sent nothing.
//! - **A pin that no longer matches must refuse**, before the `Join` frame that
//!   carries the username and password.
//! - **History is durable and paged**, and merges rather than replaces.
//! - **A private channel is never visible to a non-member** — not in the channel
//!   list, not in history.
//! - **File access is decided at the download endpoint**, over the `file_refs`
//!   table, because blobs are stored globally for dedup.
//! - **A refused send is taken back off the screen** rather than left looking
//!   delivered.

use messenger_core::core::generate_rsa_keypair;
use messenger_core::party::{ChannelKind, MessagePayload, Role};
use messenger_core::RSA_KEY_BITS;
use messenger_server::hub::Hub;
use messenger_server::run_accept_loop;
use messenger_server::state::PartyState;
use p2pem_classic::app::party_manager::{PartyJoinOutcome, PartyManager, PartyStatus};
use rsa::RsaPrivateKey;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use uuid::Uuid;

/// How long any single "wait for the server to catch up" step may take before
/// the test fails rather than hangs.
///
/// Deliberately generous. It is a hang detector, not a performance assertion:
/// every one of these tests generates real RSA-2048 keys in a debug build, and
/// under a parallel runner several do so at once. A tight bound here turns load
/// into a flake, which is worse than useless — it trains people to re-run.
const STEP_TIMEOUT: Duration = Duration::from_secs(90);

/// A server listening on an ephemeral loopback port, with durable state in a
/// temp dir so the SQLite and blob-store paths are the ones under test.
struct TestServer {
    address: String,
    _dir: tempfile::TempDir,
}

async fn start_server(password: Option<String>) -> TestServer {
    let dir = tempfile::tempdir().expect("server dir");
    let state = PartyState::load("Test Community", password, dir.path()).expect("load state");
    let state = Arc::new(Mutex::new(state));
    let privkey = Arc::new(generate_rsa_keypair(RSA_KEY_BITS).expect("server key"));
    let hub = Arc::new(Hub::new());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("addr").to_string();
    tokio::spawn(async move {
        let _ = run_accept_loop(listener, state, privkey, hub).await;
    });

    TestServer { address, _dir: dir }
}

/// Pump the manager until `done`, or fail rather than hang.
async fn pump_until<F>(mgr: &mut PartyManager, label: &str, mut done: F)
where
    F: FnMut(&mut PartyManager) -> bool,
{
    let deadline = Instant::now() + STEP_TIMEOUT;
    loop {
        mgr.poll_events();
        if done(mgr) {
            return;
        }
        if Instant::now() > deadline {
            panic!("[{label}] timed out");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Join `server` as `username`, completing the two-step trust decision.
async fn join(
    server: &TestServer,
    username: &str,
    password: Option<String>,
    key: &RsaPrivateKey,
) -> (PartyManager, Uuid) {
    let mut mgr = PartyManager::new();

    // Step one: no pin, no trust yet. Nothing is sent.
    let outcome = mgr
        .connect_and_join(
            &server.address,
            username,
            password.clone(),
            key,
            None,
            false,
        )
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

    // Step two: the user has compared the code.
    let server_id = match mgr
        .connect_and_join(&server.address, username, password, key, None, true)
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

fn channel_named(mgr: &PartyManager, server_id: Uuid, name: &str) -> Option<Uuid> {
    mgr.server(server_id)?
        .channels
        .iter()
        .find(|c| c.name == name)
        .map(|c| c.id)
}

fn texts_in(mgr: &PartyManager, server_id: Uuid, channel: Uuid) -> Vec<String> {
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

/// The core loop: join, see the default channel, create one, post to it, and
/// read the history back from the server's durable store.
#[tokio::test]
async fn a_member_joins_creates_a_channel_posts_and_reads_history_back() {
    let server = start_server(None).await;
    let key = generate_rsa_keypair(RSA_KEY_BITS).expect("client key");
    let (mut mgr, sid) = join(&server, "alice", None, &key).await;

    // The directory arrives on join.
    pump_until(&mut mgr, "channel list", |m| {
        channel_named(m, sid, "general").is_some()
    })
    .await;
    assert_eq!(
        mgr.my_role(sid),
        Some(Role::Owner),
        "the first member to join owns the server — nothing else can bootstrap an admin"
    );

    mgr.create_channel(sid, "planning".to_string())
        .expect("create channel");
    pump_until(&mut mgr, "channel created", |m| {
        channel_named(m, sid, "planning").is_some()
    })
    .await;
    let planning = channel_named(&mgr, sid, "planning").expect("planning channel");

    mgr.post(sid, planning, "first post".to_string())
        .expect("post");
    mgr.post(sid, planning, "second post".to_string())
        .expect("post");
    pump_until(&mut mgr, "posts acknowledged", |m| {
        texts_in(m, sid, planning).len() == 2
    })
    .await;

    // Read it back from the server rather than from the optimistic local copy:
    // durable history is the whole point of a community server.
    let mut fresh = PartyManager::new();
    let fingerprint = mgr.server(sid).unwrap().server_fingerprint.clone();
    let key2 = generate_rsa_keypair(RSA_KEY_BITS).expect("second key");
    let sid2 = match fresh
        .connect_and_join(
            &server.address,
            "bob",
            None,
            &key2,
            Some(&fingerprint),
            false,
        )
        .await
        .expect("join with a matching pin")
    {
        PartyJoinOutcome::Joining { server_id, .. } => server_id,
        other => panic!("a matching pin must join straight through, got {other:?}"),
    };

    pump_until(&mut fresh, "bob sees history", |m| {
        channel_named(m, sid2, "planning")
            .map(|c| texts_in(m, sid2, c).len() >= 2)
            .unwrap_or(false)
    })
    .await;
    let planning2 = channel_named(&fresh, sid2, "planning").unwrap();
    assert_eq!(
        texts_in(&fresh, sid2, planning2),
        vec!["first post".to_string(), "second post".to_string()],
        "history must come back in order"
    );
}

/// A pinned fingerprint that no longer matches must refuse *before* the `Join`
/// frame — that frame carries the username and password, so checking afterwards
/// means a server that swapped its key has already been handed them.
#[tokio::test]
async fn a_changed_server_identity_is_refused_before_any_credential_is_sent() {
    let server = start_server(Some("hunter2hunter2".to_string())).await;
    let key = generate_rsa_keypair(RSA_KEY_BITS).expect("client key");
    let mut mgr = PartyManager::new();

    let wrong_pin = "ff".repeat(32);
    let err = mgr
        .connect_and_join(
            &server.address,
            "alice",
            Some("hunter2hunter2".to_string()),
            &key,
            Some(&wrong_pin),
            false,
        )
        .await
        .expect_err("a mismatched pin must refuse");

    let msg = err.to_string();
    assert!(msg.contains("identity changed"), "unexpected error: {msg}");
    assert!(
        msg.contains("NOT sent"),
        "the message must tell the user their credentials are safe: {msg}"
    );
    assert!(
        mgr.is_empty(),
        "a refused join must not leave a server behind"
    );
}

/// The join gate: a wrong password is refused, and the refusal is surfaced
/// rather than leaving the client sitting in "Connecting…" forever.
#[tokio::test]
async fn a_wrong_server_password_is_refused_and_reported() {
    let server = start_server(Some("correct-horse-battery".to_string())).await;
    let key = generate_rsa_keypair(RSA_KEY_BITS).expect("client key");
    let mut mgr = PartyManager::new();

    let sid = match mgr
        .connect_and_join(
            &server.address,
            "alice",
            Some("wrong-password-entirely".to_string()),
            &key,
            None,
            true,
        )
        .await
        .expect("the connection itself succeeds")
    {
        PartyJoinOutcome::Joining { server_id, .. } => server_id,
        other => panic!("expected Joining, got {other:?}"),
    };

    pump_until(&mut mgr, "join rejected", |m| {
        m.server(sid)
            .is_some_and(|s| matches!(s.status, PartyStatus::Rejected(_)))
    })
    .await;
    let reason = match &mgr.server(sid).unwrap().status {
        PartyStatus::Rejected(reason) => reason.clone(),
        other => panic!("expected Rejected, got {other:?}"),
    };
    assert!(
        reason.to_lowercase().contains("password"),
        "the refusal must say what was wrong, got {reason:?}"
    );
}

/// A private channel must not appear in a non-member's channel list, and its
/// history must not be readable by guessing the id. The hub sends one identical
/// frame to every connection, so the *listing* is what has to be per member.
#[tokio::test]
async fn a_private_channel_is_invisible_to_a_non_member() {
    let server = start_server(None).await;
    let owner_key = generate_rsa_keypair(RSA_KEY_BITS).expect("owner key");
    let (mut owner, owner_sid) = join(&server, "owner", None, &owner_key).await;

    let bob_key = generate_rsa_keypair(RSA_KEY_BITS).expect("bob key");
    let (mut bob, bob_sid) = join(&server, "bob", None, &bob_key).await;

    // The owner makes a private channel with nobody else in it.
    pump_until(&mut owner, "owner sees members", |m| {
        m.server(owner_sid).is_some_and(|s| s.members.len() >= 2)
    })
    .await;
    owner
        .create_channel_of_kind(
            owner_sid,
            "secret".to_string(),
            ChannelKind::Private,
            Vec::new(),
        )
        .expect("create private channel");
    pump_until(&mut owner, "private channel exists", |m| {
        channel_named(m, owner_sid, "secret").is_some()
    })
    .await;
    let secret = channel_named(&owner, owner_sid, "secret").expect("secret channel");
    owner
        .post(owner_sid, secret, "for my eyes only".to_string())
        .expect("post");
    pump_until(&mut owner, "post lands", |m| {
        !texts_in(m, owner_sid, secret).is_empty()
    })
    .await;

    // Bob's directory refreshes — and must not contain it.
    for _ in 0..40 {
        bob.poll_events();
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        channel_named(&bob, bob_sid, "secret").is_none(),
        "a private channel must never be broadcast to a non-member"
    );

    // Nor may he read it by asking for the id directly.
    bob.fetch_history(bob_sid, secret).ok();
    for _ in 0..40 {
        bob.poll_events();
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        texts_in(&bob, bob_sid, secret).is_empty(),
        "a non-member must not be able to read a private channel by guessing its id"
    );
}

/// Files: upload, see it in the Drive listing, download it back byte-exact, and
/// have a member who cannot see it be refused at the download endpoint.
///
/// Blobs are content-addressed and stored globally for dedup, so the download
/// endpoint is the only thing deciding who may read a file.
#[tokio::test]
async fn a_file_is_shared_listed_and_downloadable_only_by_those_who_may_see_it() {
    let server = start_server(None).await;
    let owner_key = generate_rsa_keypair(RSA_KEY_BITS).expect("owner key");
    let (mut owner, sid) = join(&server, "owner", None, &owner_key).await;
    pump_until(&mut owner, "channels", |m| {
        channel_named(m, sid, "general").is_some()
    })
    .await;
    let general = channel_named(&owner, sid, "general").expect("general");

    let bytes: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
    owner
        .send_file(
            sid,
            general,
            "notes.bin".to_string(),
            "application/octet-stream".to_string(),
            bytes.clone(),
        )
        .expect("upload");

    owner.refresh_files(sid).expect("list files");
    pump_until(&mut owner, "file listed", |m| {
        m.server(sid)
            .is_some_and(|s| s.files.iter().any(|f| f.name == "notes.bin"))
    })
    .await;
    let entry = owner
        .server(sid)
        .unwrap()
        .files
        .iter()
        .find(|f| f.name == "notes.bin")
        .cloned()
        .expect("the uploaded file");
    assert_eq!(entry.size as usize, bytes.len());

    // Download it back. The client re-hashes what arrives, so a mismatch fails
    // the download rather than saving the wrong bytes.
    let rx = owner
        .request_download(sid, entry.hash.clone())
        .expect("request download");
    let downloaded = pump_for_download(&mut owner, rx).await;
    assert_eq!(downloaded, bytes, "the downloaded bytes must be the file");

    // A member who joined later, into a channel they can read, may also fetch
    // it — access follows the file reference, not who uploaded it.
    let bob_key = generate_rsa_keypair(RSA_KEY_BITS).expect("bob key");
    let (mut bob, bob_sid) = join(&server, "bob", None, &bob_key).await;
    bob.refresh_files(bob_sid).expect("list files");
    pump_until(&mut bob, "bob sees the file", |m| {
        m.server(bob_sid)
            .is_some_and(|s| s.files.iter().any(|f| f.name == "notes.bin"))
    })
    .await;
    let rx = bob
        .request_download(bob_sid, entry.hash.clone())
        .expect("request download");
    assert_eq!(pump_for_download(&mut bob, rx).await, bytes);

    // Deleting the reference stops the download, while the message stays put —
    // sequence numbers are what clients merge history on, so removing the
    // envelope would renumber the channel for everyone.
    owner
        .delete_file(sid, entry.hash.clone(), general)
        .expect("delete the share");
    // The listing is pulled, not pushed, so ask again while waiting.
    let deadline = Instant::now() + STEP_TIMEOUT;
    loop {
        owner.poll_events();
        if owner
            .server(sid)
            .is_some_and(|s| !s.files.iter().any(|f| f.name == "notes.bin"))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the deleted file must leave the listing"
        );
        owner.refresh_files(sid).ok();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let rx = bob
        .request_download(bob_sid, entry.hash.clone())
        .expect("request download");
    let after = pump_for_download_result(&mut bob, rx).await;
    assert!(
        after.is_err(),
        "a released file must stop downloading, got {} bytes",
        after.map(|b| b.len()).unwrap_or(0)
    );
}

/// Drive the manager until a download resolves, asserting it succeeded.
async fn pump_for_download(
    mgr: &mut PartyManager,
    rx: tokio::sync::oneshot::Receiver<Result<Vec<u8>, String>>,
) -> Vec<u8> {
    pump_for_download_result(mgr, rx)
        .await
        .expect("download succeeds")
}

/// As above, but hands back the result so a refusal can be asserted.
async fn pump_for_download_result(
    mgr: &mut PartyManager,
    mut rx: tokio::sync::oneshot::Receiver<Result<Vec<u8>, String>>,
) -> Result<Vec<u8>, String> {
    let deadline = Instant::now() + STEP_TIMEOUT;
    loop {
        mgr.poll_events();
        match rx.try_recv() {
            Ok(result) => return result,
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                return Err("the connection dropped before the download resolved".to_string())
            }
        }
        if Instant::now() > deadline {
            panic!("timed out waiting for a download");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A post the server refuses is taken back off the screen. Outgoing messages are
/// appended optimistically, so leaving one there tells the user something was
/// delivered that was never stored.
#[tokio::test]
async fn a_refused_post_is_removed_rather_than_left_looking_delivered() {
    let server = start_server(None).await;
    let owner_key = generate_rsa_keypair(RSA_KEY_BITS).expect("owner key");
    let (mut owner, owner_sid) = join(&server, "owner", None, &owner_key).await;

    let bob_key = generate_rsa_keypair(RSA_KEY_BITS).expect("bob key");
    let (mut bob, bob_sid) = join(&server, "bob", None, &bob_key).await;
    pump_until(&mut owner, "owner sees bob", |m| {
        m.server(owner_sid).is_some_and(|s| s.members.len() >= 2)
    })
    .await;

    // An announce channel is readable by all and writable by admins only.
    owner
        .create_channel_of_kind(
            owner_sid,
            "news".to_string(),
            ChannelKind::Announce,
            Vec::new(),
        )
        .expect("create announce channel");
    pump_until(&mut bob, "bob sees news", |m| {
        channel_named(m, bob_sid, "news").is_some()
    })
    .await;
    let news = channel_named(&bob, bob_sid, "news").expect("news channel");

    bob.post(bob_sid, news, "let me in".to_string())
        .expect("the send itself is queued");
    // Optimistically on screen for a moment...
    pump_until(&mut bob, "post refused", |m| {
        texts_in(m, bob_sid, news).is_empty() && m.server(bob_sid).unwrap().last_error.is_some()
    })
    .await;

    assert!(
        texts_in(&bob, bob_sid, news).is_empty(),
        "a refused post must not stay on screen"
    );
    assert!(
        bob.server(bob_sid).unwrap().last_error.is_some(),
        "and the user must be told why"
    );
}

/// Roles are ordered and enforced server-side. The UI hides what the caller
/// cannot do, but that is politeness — the server refuses regardless.
#[tokio::test]
async fn a_member_cannot_grant_themselves_a_role() {
    let server = start_server(None).await;
    let owner_key = generate_rsa_keypair(RSA_KEY_BITS).expect("owner key");
    let (mut owner, owner_sid) = join(&server, "owner", None, &owner_key).await;

    let bob_key = generate_rsa_keypair(RSA_KEY_BITS).expect("bob key");
    let (mut bob, bob_sid) = join(&server, "bob", None, &bob_key).await;
    pump_until(&mut owner, "owner sees bob", |m| {
        m.server(owner_sid).is_some_and(|s| s.members.len() >= 2)
    })
    .await;
    pump_until(&mut bob, "bob sees himself", |m| {
        m.server(bob_sid).is_some_and(|s| s.member_id.is_some())
    })
    .await;

    let bob_id = bob.server(bob_sid).unwrap().member_id.unwrap();
    assert_eq!(bob.my_role(bob_sid), Some(Role::Member));

    bob.set_role(bob_sid, bob_id, Role::Admin)
        .expect("the request is sent");
    for _ in 0..40 {
        bob.poll_events();
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        bob.my_role(bob_sid),
        Some(Role::Member),
        "the server must refuse a self-promotion"
    );
}
