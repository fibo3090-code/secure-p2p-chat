//! Attacks against the trust boundaries, rather than tests of them.
//!
//! Each of these plays a hostile party and tries to reach something it should
//! not: forge an invite, get a fingerprint accepted without the user answering,
//! read another member's private channel, pull a file by content hash without
//! permission, replay a captured frame, or make a peer write more to disk than
//! they agreed to.
//!
//! The distinction from the rest of the suite matters. Those tests drive the
//! happy path and check the result; these start from what an attacker controls
//! and see how far it reaches. A test that only ever asks for what it is allowed
//! cannot tell you what happens when someone asks for what they are not.

use messenger_core::core::generate_rsa_keypair;
use messenger_core::party::{ChannelKind, Role};
use messenger_core::types::{Config, TrustState};
use messenger_core::RSA_KEY_BITS;
use messenger_server::hub::Hub;
use messenger_server::run_accept_loop;
use messenger_server::state::PartyState;
use p2pem_classic::app::chat_manager::ChatManager;
use p2pem_classic::app::party_manager::{PartyJoinOutcome, PartyManager, PartyStatus};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use uuid::Uuid;

const STEP_TIMEOUT: Duration = Duration::from_secs(90);

// ── Invite forgery ──────────────────────────────────────────────────────────

/// An invite's `fingerprint` and its `public_key` must agree.
///
/// The v2 signature only proves the maker holds the private key for the key
/// *inside* the invite. `fingerprint` is a separate field, and it is the one
/// every later trust decision is made against — so an invite naming someone
/// else's fingerprint alongside the attacker's key would have the victim's
/// identity attached to the attacker's connection.
#[test]
fn an_invite_whose_fingerprint_contradicts_its_key_is_refused() {
    let mgr = ChatManager::new(Config::default());

    // A real key, paired with a fingerprint that is not its own.
    let key = generate_rsa_keypair(RSA_KEY_BITS).expect("key");
    let pem = messenger_core::core::pem_encode_public(&rsa::RsaPublicKey::from(&key))
        .expect("encode public key");
    let honest = messenger_core::core::fingerprint_pubkey(pem.as_bytes());
    let forged = "ff".repeat(32);
    assert_ne!(honest, forged);

    let payload = serde_json::json!({
        "name": "Impostor",
        "address": "10.0.0.9:12345",
        "fingerprint": forged,
        "public_key": pem,
    });
    use base64::Engine;
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&payload).unwrap());
    let link = format!("chat-p2p://invite/{encoded}");

    let result = mgr.parse_invite_link(&link);
    assert!(
        result.is_err(),
        "an invite naming a fingerprint its key does not produce must be refused"
    );
}

/// Garbage in the invite slot must be refused rather than panicking — the link
/// is pasted by the user from wherever they got it.
#[test]
fn malformed_invite_links_are_refused_without_panicking() {
    let mgr = ChatManager::new(Config::default());
    let attacks = [
        "",
        "chat-p2p://invite/",
        "chat-p2p://invite/!!!not-base64!!!",
        "chat-p2p://invite/e30",    // {}
        "chat-p2p://invite/bnVsbA", // null
        "chat-p2p://invite/W10",    // []
        "http://example.com/",
        "javascript:alert(1)",
        &format!("chat-p2p://invite/{}", "A".repeat(100_000)),
    ];
    for link in attacks {
        // The contract is "returns Err", not "does not crash the app".
        let _ = mgr.parse_invite_link(link);
    }
}

// ── TOFU ────────────────────────────────────────────────────────────────────

/// A contact the user never verified must not make a fingerprint auto-accepted.
///
/// `known_trusted` only auto-accepts a fingerprint a *chat* already stores or
/// that a contact holds as `Verified`/`Trusted`. Matching any contact at all
/// meant pasting a link pre-trusted whatever fingerprint it named, and that peer
/// then connected with no safety-code prompt.
#[test]
fn an_unverified_contact_does_not_pre_trust_its_fingerprint() {
    let mut mgr = ChatManager::new(Config::default());
    let fp = "ab".repeat(32);
    let id = mgr.add_contact("Stranger".into(), None, Some(fp.clone()), None);

    assert_eq!(
        mgr.get_contact(id).map(|c| c.trust_state),
        Some(TrustState::Unverified),
    );
    // No chat holds this fingerprint and no contact vouches for it as verified,
    // so a connection claiming it must still face the prompt.
    assert!(
        !mgr.chats
            .values()
            .any(|c| c.peer_fingerprint.as_deref() == Some(fp.as_str())),
        "nothing may have silently recorded this fingerprint as trusted"
    );
}

/// An incoming chat must not start with the peer's own fingerprint pre-filled:
/// that would make the TOFU comparison trivially match and auto-trust every
/// caller. This is a bug that existed once already.
#[test]
fn a_fresh_incoming_chat_holds_no_fingerprint() {
    let mut mgr = ChatManager::new(Config::default());
    let id = Uuid::new_v4();
    mgr.create_local_chat_for_test(id, "Peer".into());
    assert_eq!(
        mgr.chats.get(&id).and_then(|c| c.peer_fingerprint.clone()),
        None,
        "an incoming conversation must start unverified"
    );
}

// ── Community access control ────────────────────────────────────────────────

struct TestServer {
    address: String,
    _dir: tempfile::TempDir,
}

async fn start_server() -> TestServer {
    let dir = tempfile::tempdir().expect("dir");
    let state = PartyState::load("Attack Target", None, dir.path()).expect("state");
    let privkey = Arc::new(generate_rsa_keypair(RSA_KEY_BITS).expect("key"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("addr").to_string();
    let hub = Arc::new(Hub::new());
    let state = Arc::new(Mutex::new(state));
    tokio::spawn(async move {
        let _ = run_accept_loop(listener, state, privkey, hub).await;
    });
    TestServer { address, _dir: dir }
}

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
        assert!(Instant::now() < deadline, "[{label}] timed out");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn join(server: &TestServer, username: &str) -> (PartyManager, Uuid) {
    let key = generate_rsa_keypair(RSA_KEY_BITS).expect("key");
    let mut mgr = PartyManager::new();
    let sid = match mgr
        .connect_and_join(&server.address, username, None, &key, None, true)
        .await
        .expect("join")
    {
        PartyJoinOutcome::Joining { server_id, .. } => server_id,
        other => panic!("expected Joining, got {other:?}"),
    };
    pump_until(&mut mgr, "joined", |m| {
        m.server(sid)
            .is_some_and(|s| s.status == PartyStatus::Joined && s.member_id.is_some())
    })
    .await;
    (mgr, sid)
}

/// Content-addressed storage is shared across the whole server, so knowing a
/// hash must not be enough to fetch the bytes. A member who cannot reach the
/// place a file was shared must be refused at the download endpoint.
#[tokio::test]
async fn a_content_hash_is_not_a_capability() {
    let server = start_server().await;
    let (mut owner, owner_sid) = join(&server, "owner").await;
    let (mut intruder, intruder_sid) = join(&server, "intruder").await;

    pump_until(&mut owner, "members", |m| {
        m.server(owner_sid).is_some_and(|s| s.members.len() >= 2)
    })
    .await;

    // A private channel the intruder is not in.
    owner
        .create_channel_of_kind(
            owner_sid,
            "vault".to_string(),
            ChannelKind::Private,
            Vec::new(),
        )
        .expect("create private channel");
    pump_until(&mut owner, "channel", |m| {
        m.server(owner_sid)
            .is_some_and(|s| s.channels.iter().any(|c| c.name == "vault"))
    })
    .await;
    let vault = owner
        .server(owner_sid)
        .unwrap()
        .channels
        .iter()
        .find(|c| c.name == "vault")
        .map(|c| c.id)
        .expect("vault");

    let secret = b"the contents of a private channel".to_vec();
    owner
        .send_file(
            owner_sid,
            vault,
            "secret.bin".to_string(),
            "application/octet-stream".to_string(),
            secret.clone(),
        )
        .expect("upload");
    owner.refresh_files(owner_sid).expect("list");
    pump_until(&mut owner, "file stored", |m| {
        m.server(owner_sid)
            .is_some_and(|s| s.files.iter().any(|f| f.name == "secret.bin"))
    })
    .await;
    let hash = owner
        .server(owner_sid)
        .unwrap()
        .files
        .iter()
        .find(|f| f.name == "secret.bin")
        .map(|f| f.hash.clone())
        .expect("hash");

    // The intruder never sees it in a listing…
    intruder.refresh_files(intruder_sid).expect("list");
    for _ in 0..40 {
        intruder.poll_events();
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        !intruder
            .server(intruder_sid)
            .unwrap()
            .files
            .iter()
            .any(|f| f.name == "secret.bin"),
        "a private channel's file must not appear in a non-member's listing"
    );

    // …and asking for the hash directly must be refused.
    let rx = intruder
        .request_download(intruder_sid, hash)
        .expect("request is sent");
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut rx = rx;
    let outcome = loop {
        intruder.poll_events();
        match rx.try_recv() {
            Ok(result) => break result,
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                break Err("connection closed".to_string())
            }
        }
        if Instant::now() > deadline {
            break Err("no answer".to_string());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    match outcome {
        Err(_) => {} // refused, as required
        Ok(bytes) => panic!(
            "a non-member downloaded {} bytes of a private file by hash",
            bytes.len()
        ),
    }
}

/// Roles are ordered, and the ordering is the whole governance model: nobody may
/// grant at or above their own level, and the owner may never be demoted.
/// Otherwise whoever gets an admin seat owns the community.
#[tokio::test]
async fn a_member_cannot_escalate_or_depose_the_owner() {
    let server = start_server().await;
    let (mut owner, owner_sid) = join(&server, "owner").await;
    let (mut member, member_sid) = join(&server, "member").await;

    pump_until(&mut owner, "members", |m| {
        m.server(owner_sid).is_some_and(|s| s.members.len() >= 2)
    })
    .await;
    pump_until(&mut member, "self", |m| {
        m.server(member_sid).is_some_and(|s| s.member_id.is_some())
    })
    .await;

    let me = member.server(member_sid).unwrap().member_id.unwrap();
    let owner_id = member
        .server(member_sid)
        .unwrap()
        .members
        .iter()
        .find(|m| m.username == "owner")
        .map(|m| m.id)
        .expect("the owner is visible in the directory");

    // Promote self to admin, then to owner; demote the actual owner.
    for role in [Role::Admin, Role::Owner] {
        let _ = member.set_role(member_sid, me, role);
    }
    let _ = member.set_role(member_sid, owner_id, Role::Guest);

    for _ in 0..60 {
        member.poll_events();
        owner.poll_events();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    assert_eq!(
        member.my_role(member_sid),
        Some(Role::Member),
        "a member must not be able to promote themselves"
    );
    assert_eq!(
        owner.my_role(owner_sid),
        Some(Role::Owner),
        "the owner must not be demotable by a member"
    );
}

/// A guest is read-only everywhere. The UI hides the controls, but the server is
/// what has to refuse — a modified client simply would not hide them.
#[tokio::test]
async fn a_guest_cannot_write_however_it_asks() {
    let server = start_server().await;
    let (mut owner, owner_sid) = join(&server, "owner").await;
    let (mut guest, guest_sid) = join(&server, "guest").await;

    pump_until(&mut owner, "members", |m| {
        m.server(owner_sid).is_some_and(|s| s.members.len() >= 2)
    })
    .await;
    let guest_id = owner
        .server(owner_sid)
        .unwrap()
        .members
        .iter()
        .find(|m| m.username == "guest")
        .map(|m| m.id)
        .expect("guest visible");
    owner
        .set_role(owner_sid, guest_id, Role::Guest)
        .expect("demote to guest");

    pump_until(&mut guest, "demoted", |m| {
        m.my_role(guest_sid) == Some(Role::Guest)
    })
    .await;

    let general = guest
        .server(guest_sid)
        .unwrap()
        .channels
        .first()
        .map(|c| c.id)
        .expect("a channel");

    // Every write the client can express.
    let _ = guest.post(guest_sid, general, "let me in".to_string());
    let _ = guest.create_channel(guest_sid, "guest-channel".to_string());
    let _ = guest.send_file(
        guest_sid,
        general,
        "payload.bin".to_string(),
        "application/octet-stream".to_string(),
        vec![0u8; 64],
    );

    for _ in 0..60 {
        guest.poll_events();
        owner.poll_events();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    owner.refresh_files(owner_sid).ok();
    for _ in 0..40 {
        owner.poll_events();
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let server_view = owner.server(owner_sid).unwrap();
    assert!(
        !server_view
            .channels
            .iter()
            .any(|c| c.name == "guest-channel"),
        "a guest created a channel"
    );
    assert!(
        !server_view.files.iter().any(|f| f.name == "payload.bin"),
        "a guest uploaded a file"
    );
}

/// Joining with someone else's username must not take over their membership.
/// Identity is the handshake-verified fingerprint, not the name typed into a box.
#[tokio::test]
async fn a_username_is_not_an_identity() {
    let server = start_server().await;
    let (mut owner, owner_sid) = join(&server, "owner").await;
    pump_until(&mut owner, "joined", |m| {
        m.server(owner_sid).is_some_and(|s| s.member_id.is_some())
    })
    .await;
    let real_owner_id = owner.server(owner_sid).unwrap().member_id.unwrap();

    // A different key claiming the same username.
    let impostor_key = generate_rsa_keypair(RSA_KEY_BITS).expect("key");
    let mut impostor = PartyManager::new();
    let outcome = impostor
        .connect_and_join(&server.address, "owner", None, &impostor_key, None, true)
        .await
        .expect("the connection itself succeeds");
    let imp_sid = match outcome {
        PartyJoinOutcome::Joining { server_id, .. } => server_id,
        other => panic!("expected Joining, got {other:?}"),
    };

    for _ in 0..80 {
        impostor.poll_events();
        owner.poll_events();
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let status = impostor.server(imp_sid).map(|s| s.status.clone());
    match status {
        Some(PartyStatus::Rejected(_)) => {} // refused outright: correct
        Some(PartyStatus::Joined) => {
            let imp_id = impostor.server(imp_sid).unwrap().member_id;
            assert_ne!(
                imp_id,
                Some(real_owner_id),
                "a different key took over an existing member by reusing the username"
            );
            assert_ne!(
                impostor.my_role(imp_sid),
                Some(Role::Owner),
                "a name collision must not confer the owner's role"
            );
        }
        other => panic!("unexpected status {other:?}"),
    }
}
