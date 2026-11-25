//! Communities end to end: a real `messenger-server` on a loopback socket,
//! joined by the real `PartyManager` the desktop bridge and the TUI drive.
//!
//! A join crosses the handshake, the two-step trust decision, the join gate,
//! the hub, dispatch, SQLite and the blob store. The server's own suites pin
//! each rule against `PartyState` directly; what is pinned here is that the
//! *client* reaches those rules over the wire and reacts correctly when they
//! refuse it:
//!
//! - **No credential moves before the trust step**, and a changed pin refuses
//!   before the `Join` frame that carries the password.
//! - **History is durable and paged**, including past one frame's worth.
//! - **A private channel and its files are invisible to a non-member** —
//!   listing, history by id, file listing, and download by content hash.
//! - **Governance is the server's**, whatever a modified client asks for.
//! - **A refused send is taken back off the screen.**
//!
//! Negative assertions go through [`settle`], never "poll for a second and see
//! that nothing happened": that passes just as well when the server has not
//! got round to the request yet.

use crate::common::*;
use messenger_core::party::{blob_hash, ChannelKind, MessagePayload, Role};
use p2pem_classic::app::party_manager::{PartyJoinOutcome, PartyManager, PartyStatus};
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Join, create a channel, post, and read the history back through a second
/// member — from the server's durable store, not the optimistic local copy.
#[tokio::test]
async fn a_member_joins_creates_a_channel_posts_and_reads_history_back() {
    let server = start_server(None).await;
    let (mut alice, sid) = join(&server, "alice", None).await;
    assert_eq!(
        alice.my_role(sid),
        Some(Role::Member),
        "being first to join makes nobody the owner"
    );

    alice
        .create_channel(sid, "planning".to_string())
        .expect("create channel");
    pump_until(&mut alice, "channel created", |m| {
        channel_named(m, sid, "planning").is_some()
    })
    .await;
    let planning = channel_named(&alice, sid, "planning").unwrap();
    alice
        .post(sid, planning, "first post".into())
        .expect("post");
    alice
        .post(sid, planning, "second post".into())
        .expect("post");
    pump_until(&mut alice, "posts confirmed", |m| {
        m.server(sid).unwrap().messages[&planning]
            .iter()
            .filter(|e| e.seq != 0)
            .count()
            == 2
    })
    .await;

    // A pin that matches joins straight through, with no verification stop.
    let fingerprint = alice.server(sid).unwrap().server_fingerprint.clone();
    let mut bob = PartyManager::new();
    let bob_sid = match bob
        .connect_and_join(&server.address, "bob", None, &rsa_key(), Some(&fingerprint))
        .await
        .expect("join with a matching pin")
    {
        PartyJoinOutcome::Joining { server_id, .. } => server_id,
        other => panic!("a matching pin must join straight through, got {other:?}"),
    };
    pump_until(&mut bob, "bob sees history", |m| {
        channel_named(m, bob_sid, "planning").is_some_and(|c| texts_in(m, bob_sid, c).len() >= 2)
    })
    .await;
    let planning = channel_named(&bob, bob_sid, "planning").unwrap();
    assert_eq!(
        texts_in(&bob, bob_sid, planning),
        ["first post", "second post"],
        "history comes back in order"
    );
}

/// A pinned fingerprint that no longer matches must refuse *before* the `Join`
/// frame — that frame carries the username and password, so checking after it
/// means a server that swapped its key has already been handed them.
#[tokio::test]
async fn a_changed_server_identity_is_refused_before_any_credential_is_sent() {
    let server = start_server(Some("hunter2hunter2".to_string())).await;
    let mut mgr = PartyManager::new();
    let err = mgr
        .connect_and_join(
            &server.address,
            "alice",
            Some("hunter2hunter2".to_string()),
            &rsa_key(),
            Some(&"ff".repeat(32)),
        )
        .await
        .expect_err("a mismatched pin must refuse");
    let msg = err.to_string();
    assert!(msg.contains("not the one you verified"), "got: {msg}");
    assert!(
        msg.contains("NOT sent"),
        "the user must be told their credentials are safe: {msg}"
    );
    assert!(mgr.is_empty(), "a refused join must leave no server behind");
    assert!(
        server.state.lock().await.members().is_empty(),
        "nobody may have joined"
    );
}

/// A wrong password is refused, and the refusal is surfaced rather than
/// leaving the client sitting in "Connecting…" forever.
#[tokio::test]
async fn a_wrong_server_password_is_refused_and_reported() {
    let server = start_server(Some("correct-horse-battery".to_string())).await;
    let key = rsa_key();
    let mut mgr = PartyManager::new();
    let sid = match mgr
        .connect_and_join(
            &server.address,
            "alice",
            Some("wrong-password-entirely".to_string()),
            &key,
            Some(&learn_fingerprint(&server.address, &key).await),
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
    let PartyStatus::Rejected(reason) = &mgr.server(sid).unwrap().status else {
        unreachable!()
    };
    assert!(
        reason.to_lowercase().contains("password"),
        "the refusal must say what was wrong, got {reason:?}"
    );
}

/// A private channel is invisible to a non-member on every route the client
/// has: the channel list, history fetched by guessing the id, the file
/// listing, and a download by content hash. Blobs are stored globally for
/// dedup, so knowing a hash must not be a capability.
#[tokio::test]
async fn a_private_channel_and_its_files_are_invisible_to_a_non_member() {
    let server = start_server(None).await;
    let (mut owner, owner_sid) = join_owner(&server, "owner").await;
    let (mut bob, bob_sid) = join(&server, "bob", None).await;

    owner
        .create_channel_of_kind(owner_sid, "vault".into(), ChannelKind::Private, Vec::new())
        .expect("create private channel");
    pump_until(&mut owner, "vault exists", |m| {
        channel_named(m, owner_sid, "vault").is_some()
    })
    .await;
    let vault = channel_named(&owner, owner_sid, "vault").unwrap();
    owner
        .post(owner_sid, vault, "for my eyes only".into())
        .expect("post");
    let secret = b"the contents of a private channel".to_vec();
    owner
        .send_file(
            owner_sid,
            vault,
            "secret.bin".into(),
            "application/octet-stream".into(),
            secret.clone(),
        )
        .expect("upload");
    pump_until(&mut owner, "post and file stored", |m| {
        m.server(owner_sid)
            .and_then(|s| s.messages.get(&vault))
            .is_some_and(|msgs| msgs.len() == 2 && msgs.iter().all(|e| e.seq != 0))
    })
    .await;

    // Everything bob could ask for, then a barrier so the answers are in.
    bob.fetch_history(bob_sid, vault).ok();
    bob.refresh_files(bob_sid).expect("list files");
    settle(&mut bob, bob_sid).await;
    assert!(
        channel_named(&bob, bob_sid, "vault").is_none(),
        "a private channel reached a non-member's channel list"
    );
    assert!(
        texts_in(&bob, bob_sid, vault).is_empty(),
        "a non-member read a private channel by guessing its id"
    );
    assert!(
        !bob.server(bob_sid)
            .unwrap()
            .files
            .iter()
            .any(|f| f.name == "secret.bin"),
        "a private channel's file appeared in a non-member's listing"
    );
    assert!(
        download(&mut bob, bob_sid, blob_hash(&secret))
            .await
            .is_err(),
        "a non-member downloaded a private file by its hash"
    );
}

/// Upload, see it listed, download it byte-exact; a later member of the
/// channel may fetch it too; deleting the share stops the download while the
/// message stays — sequence numbers are what clients merge history on.
#[tokio::test]
async fn a_shared_file_is_listed_downloadable_and_revocable() {
    let server = start_server(None).await;
    let (mut owner, sid) = join_owner(&server, "owner").await;
    let general = channel_named(&owner, sid, "general").unwrap();

    let bytes: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
    owner
        .send_file(
            sid,
            general,
            "notes.bin".into(),
            "application/octet-stream".into(),
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
        .unwrap();
    assert_eq!(entry.size as usize, bytes.len());
    assert_eq!(
        download(&mut owner, sid, entry.hash.clone()).await,
        Ok(bytes.clone())
    );

    // Access follows the file reference, not who uploaded it.
    let (mut bob, bob_sid) = join(&server, "bob", None).await;
    assert_eq!(
        download(&mut bob, bob_sid, entry.hash.clone()).await,
        Ok(bytes)
    );

    owner
        .delete_file(sid, entry.hash.clone(), general)
        .expect("delete the share");
    // The listing is pulled, not pushed, so ask again while waiting.
    let deadline = Instant::now() + STEP_TIMEOUT;
    while owner
        .server(sid)
        .is_some_and(|s| s.files.iter().any(|f| f.name == "notes.bin"))
    {
        assert!(Instant::now() < deadline, "the deleted file stayed listed");
        owner.refresh_files(sid).ok();
        tokio::time::sleep(Duration::from_millis(50)).await;
        owner.poll_events();
    }
    assert!(
        download(&mut bob, bob_sid, entry.hash).await.is_err(),
        "a released file must stop downloading"
    );
    assert!(
        owner.server(sid).unwrap().messages[&general]
            .iter()
            .any(|e| matches!(e.payload, MessagePayload::File(_))),
        "deleting a file must not renumber the channel by removing its message"
    );
}

/// A post the server refuses is taken back off the screen, and the user is
/// told why. Outgoing posts are appended optimistically, so leaving one there
/// says something was delivered that was never stored.
#[tokio::test]
async fn a_refused_post_is_removed_rather_than_left_looking_delivered() {
    let server = start_server(None).await;
    let (owner, owner_sid) = join_owner(&server, "owner").await;
    let (mut bob, bob_sid) = join(&server, "bob", None).await;

    // An announce channel: readable by all, writable by admins only.
    owner
        .create_channel_of_kind(owner_sid, "news".into(), ChannelKind::Announce, Vec::new())
        .expect("create announce channel");
    pump_until(&mut bob, "bob sees news", |m| {
        channel_named(m, bob_sid, "news").is_some()
    })
    .await;
    let news = channel_named(&bob, bob_sid, "news").unwrap();

    bob.post(bob_sid, news, "let me in".into())
        .expect("the send itself is queued");
    assert_eq!(
        texts_in(&bob, bob_sid, news),
        ["let me in"],
        "the post is shown optimistically"
    );
    pump_until(&mut bob, "post retracted", |m| {
        texts_in(m, bob_sid, news).is_empty() && m.server(bob_sid).unwrap().last_error.is_some()
    })
    .await;
}

/// Governance is enforced by the server, whatever the client asks for. The UI
/// hides these controls; a modified client simply would not.
///
/// - a member cannot promote themselves, or demote the owner;
/// - a guest cannot post, create a channel, or upload;
/// - a different key cannot take over a member by reusing their username.
#[tokio::test]
async fn governance_is_enforced_by_the_server_not_the_client() {
    let server = start_server(None).await;
    let (mut owner, owner_sid) = join_owner(&server, "owner").await;
    let (mut member, member_sid) = join(&server, "member", None).await;
    let (mut guest, guest_sid) = join(&server, "guest", None).await;

    pump_until(&mut owner, "directory", |m| {
        m.server(owner_sid).is_some_and(|s| s.members.len() == 3)
    })
    .await;
    let guest_id = member_named(&owner, owner_sid, "guest").unwrap();
    owner
        .set_role(owner_sid, guest_id, Role::Guest)
        .expect("demote to guest");
    pump_until(&mut guest, "demoted", |m| {
        m.my_role(guest_sid) == Some(Role::Guest)
    })
    .await;

    // The member reaches for the community.
    let me = member.server(member_sid).unwrap().member_id.unwrap();
    let owner_id = member_named(&member, member_sid, "owner").unwrap();
    for role in [Role::Admin, Role::Owner] {
        member.set_role(member_sid, me, role).expect("sent");
    }
    member
        .set_role(member_sid, owner_id, Role::Guest)
        .expect("sent");

    // The guest tries every write the client can express.
    let general = channel_named(&guest, guest_sid, "general").unwrap();
    guest
        .post(guest_sid, general, "let me in".into())
        .expect("sent");
    guest
        .create_channel(guest_sid, "guest-channel".into())
        .expect("sent");
    guest
        .send_file(
            guest_sid,
            general,
            "payload.bin".into(),
            "application/octet-stream".into(),
            vec![0u8; 64],
        )
        .expect("sent");

    settle(&mut member, member_sid).await;
    settle(&mut guest, guest_sid).await;

    {
        let state = server.state.lock().await;
        assert_eq!(
            state.role_of(me),
            Role::Member,
            "a member promoted themselves"
        );
        assert_eq!(
            state.role_of(owner_id),
            Role::Owner,
            "a member deposed the owner"
        );
        assert_eq!(state.role_of(guest_id), Role::Guest);
        assert!(
            !state.channels().iter().any(|c| c.name == "guest-channel"),
            "a guest created a channel"
        );
        assert!(
            state
                .files_for(owner_id)
                .iter()
                .all(|f| f.name != "payload.bin"),
            "a guest uploaded a file"
        );
    }
    assert_eq!(member.my_role(member_sid), Some(Role::Member));
    assert!(
        texts_in(&guest, guest_sid, general).is_empty(),
        "the guest's refused post is still on its screen"
    );

    // A different key claiming the owner's username is refused outright.
    let key = rsa_key();
    let mut impostor = PartyManager::new();
    let imp_sid = match impostor
        .connect_and_join(
            &server.address,
            "owner",
            None,
            &key,
            Some(&learn_fingerprint(&server.address, &key).await),
        )
        .await
        .expect("the connection itself succeeds")
    {
        PartyJoinOutcome::Joining { server_id, .. } => server_id,
        other => panic!("expected Joining, got {other:?}"),
    };
    pump_until(&mut impostor, "impostor refused", |m| {
        m.server(imp_sid)
            .is_some_and(|s| matches!(s.status, PartyStatus::Rejected(_)))
    })
    .await;
    assert_eq!(server.state.lock().await.members().len(), 3);
}

/// The accept loop survives connections that hang up without a handshake —
/// what a port scanner or a health check produces all day — and keeps serving.
#[tokio::test]
async fn the_server_keeps_serving_after_failed_connections() {
    let server = start_server(None).await;
    // Under the per-address limit (10 per 30 s), leaving room for the join's
    // own two connections: going over would test the rate limiter instead.
    for garbage in [&b""[..], b"", b"", b"not a handshake", b"x", b"\0\0\0\xff"] {
        if let Ok(mut stream) = tokio::net::TcpStream::connect(&server.address).await {
            use tokio::io::AsyncWriteExt;
            let _ = stream.write_all(garbage).await;
        }
    }
    let (mut alice, sid) = join(&server, "alice", None).await;
    let general = channel_named(&alice, sid, "general").unwrap();
    alice.post(sid, general, "still here".into()).expect("post");
    pump_until(&mut alice, "post confirmed", |m| {
        m.server(sid).unwrap().messages[&general]
            .iter()
            .any(|e| e.seq != 0)
    })
    .await;
}

/// Regression: the client's connection task selected between "send the next
/// request" and "read the next response", building a fresh read each time.
/// `recv_framed` is not cancel-safe, so a request winning the race while a
/// response was half-read lost bytes and dropped the connection. Download
/// chunks streaming in while the user keeps posting is exactly that.
#[tokio::test]
async fn sending_while_a_large_download_streams_in_keeps_the_connection() {
    let server = start_server(None).await;
    let (mut mgr, sid) = join(&server, "alice", None).await;
    let general = channel_named(&mgr, sid, "general").unwrap();

    // Past the inline limit, so it is uploaded and downloaded in chunks.
    let data: Vec<u8> = (0..6 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    mgr.send_file(
        sid,
        general,
        "big.bin".into(),
        "application/octet-stream".into(),
        data.clone(),
    )
    .expect("upload");
    pump_until(&mut mgr, "upload stored", |m| {
        m.server(sid).unwrap().messages[&general]
            .iter()
            .any(|e| matches!(e.payload, MessagePayload::File(_)) && e.seq != 0)
    })
    .await;

    let mut rx = mgr
        .request_download(sid, blob_hash(&data))
        .expect("download");
    let deadline = Instant::now() + STEP_TIMEOUT;
    let mut sent = 0;
    let downloaded = loop {
        mgr.poll_events();
        if sent < 400 {
            for _ in 0..4 {
                mgr.post(sid, general, format!("while downloading {sent}"))
                    .expect("post");
                sent += 1;
            }
        }
        match rx.try_recv() {
            Ok(result) => break result,
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                break Err("the connection dropped mid-download".to_string())
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the download"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    assert_eq!(downloaded.expect("download completes"), data);
    pump_until(&mut mgr, "every post confirmed", |m| {
        let s = m.server(sid).unwrap();
        s.status == PartyStatus::Joined
            && s.messages[&general]
                .iter()
                .filter(|e| matches!(e.payload, MessagePayload::Text(_)))
                .all(|e| e.seq != 0)
    })
    .await;
}

/// SRV-01: a channel full of maximum-size messages made the community
/// unjoinable — the first history page was over the frame limit, and a reply
/// that cannot be sent ends the connection. Pages are budgeted by bytes now,
/// and the client keeps asking when a page was trimmed for size.
#[tokio::test]
async fn a_channel_of_maximum_size_messages_is_joinable_and_pages_to_the_end() {
    let server = start_server(None).await;
    let (alice, sid) = join(&server, "alice", None).await;
    let general = channel_named(&alice, sid, "general").unwrap();
    let alice_id = alice.server(sid).unwrap().member_id.unwrap();

    // Well past what one count-capped page could carry in a frame.
    const POSTS: usize = 150;
    {
        let mut st = server.state.lock().await;
        for i in 0..POSTS {
            let mut text = format!("{i:04}");
            text.push_str(&"m".repeat(messenger_core::MAX_TEXT_MESSAGE_BYTES - text.len()));
            st.post_message(alice_id, general, text).expect("post");
        }
    }

    let (mut bob, bob_sid) = join(&server, "bob", None).await;
    pump_until(&mut bob, "every page arrives", |m| {
        texts_in(m, bob_sid, general).len() >= POSTS
    })
    .await;
    let texts = texts_in(&bob, bob_sid, general);
    assert_eq!(texts.len(), POSTS);
    for (i, t) in texts.iter().enumerate() {
        assert!(t.starts_with(&format!("{i:04}")), "out of order at {i}");
    }
    assert_eq!(bob.server(bob_sid).unwrap().status, PartyStatus::Joined);
}

/// SRV-04: on a password-protected server the operator joins with the one-time
/// claim token in the password field and becomes the Owner; the token is then
/// spent, and whoever joined first is not promoted.
#[tokio::test]
async fn the_owner_is_whoever_presents_the_claim_token_not_whoever_is_first() {
    let password = Uuid::new_v4().to_string();
    let server = start_server(Some(password.clone())).await;
    let token = server
        .claim_token
        .clone()
        .expect("a fresh server has no owner");

    let (early, early_sid) = join(&server, "early-bird", Some(password)).await;
    assert_eq!(early.my_role(early_sid), Some(Role::Member));

    let (mut op, op_sid) = join(&server, "operator", Some(token)).await;
    pump_until(&mut op, "owner role", |m| {
        m.my_role(op_sid) == Some(Role::Owner)
    })
    .await;
    assert_eq!(server.state.lock().await.owner_claim_token(), None, "spent");
}
