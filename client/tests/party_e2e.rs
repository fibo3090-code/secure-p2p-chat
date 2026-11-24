//! End-to-end Communities: a real `messenger-server` on a loopback socket,
//! joined by the real client-side `PartyManager` (what the desktop bridge and
//! the TUI drive). The failures that matter only appear when handshake, trust
//! step, join gate, hub, dispatch, SQLite and blob store run together:
//!
//! - **No credential moves before the trust step**, and a pin that no longer
//!   matches refuses before the `Join` frame.
//! - **History is durable and paged**, and merges rather than replaces.
//! - **A private channel is never visible to a non-member.**
//! - **File access is decided at the download endpoint** (blobs are global).
//! - **A refused send is taken back off the screen.**

mod common;

use common::*;
use messenger_core::party::{ChannelKind, MessagePayload, Role};
use p2pem_classic::app::party_manager::{PartyJoinOutcome, PartyManager, PartyStatus};
use std::time::{Duration, Instant};
use uuid::Uuid;

async fn download(
    mgr: &mut PartyManager,
    rx: tokio::sync::oneshot::Receiver<Result<Vec<u8>, String>>,
) -> Result<Vec<u8>, String> {
    download_result(mgr, rx, STEP_TIMEOUT)
        .await
        .expect("timed out waiting for a download")
}

/// The core loop: join, see the default channel, create one, post to it, and
/// read the history back from the server's durable store.
#[tokio::test]
async fn a_member_joins_creates_a_channel_posts_and_reads_history_back() {
    let server = start_server(None).await;
    let (mut mgr, sid) = join(&server, "alice", None).await;

    wait_channel(&mut mgr, sid, "general").await;
    assert_eq!(
        mgr.my_role(sid),
        Some(Role::Member),
        "being first to join no longer makes anyone the owner"
    );

    mgr.create_channel(sid, "planning".to_string())
        .expect("create channel");
    let planning = wait_channel(&mut mgr, sid, "planning").await;
    mgr.post(sid, planning, "first post".to_string())
        .expect("post");
    mgr.post(sid, planning, "second post".to_string())
        .expect("post");
    pump_until(&mut mgr, "posts acknowledged", |m| {
        texts_in(m, sid, planning).len() == 2
    })
    .await;

    // Read it back from the server, not the optimistic local copy.
    let mut fresh = PartyManager::new();
    let fingerprint = mgr.server(sid).unwrap().server_fingerprint.clone();
    let sid2 = match fresh
        .connect_and_join(
            &server.address,
            "bob",
            None,
            &client_key(),
            Some(&fingerprint),
        )
        .await
        .expect("join with a matching pin")
    {
        PartyJoinOutcome::Joining { server_id, .. } => server_id,
        other => panic!("a matching pin must join straight through, got {other:?}"),
    };

    pump_until(&mut fresh, "bob sees history", |m| {
        channel_named(m, sid2, "planning").is_some_and(|c| texts_in(m, sid2, c).len() >= 2)
    })
    .await;
    let planning2 = channel_named(&fresh, sid2, "planning").unwrap();
    assert_eq!(
        texts_in(&fresh, sid2, planning2),
        vec!["first post".to_string(), "second post".to_string()],
        "history must come back in order"
    );
}

/// A pin that no longer matches must refuse *before* the `Join` frame, which
/// carries the username and password.
#[tokio::test]
async fn a_changed_server_identity_is_refused_before_any_credential_is_sent() {
    let server = start_server(Some("hunter2hunter2".to_string())).await;
    let mut mgr = PartyManager::new();

    let wrong_pin = "ff".repeat(32);
    let err = mgr
        .connect_and_join(
            &server.address,
            "alice",
            Some("hunter2hunter2".to_string()),
            &client_key(),
            Some(&wrong_pin),
        )
        .await
        .expect_err("a mismatched pin must refuse");

    let msg = err.to_string();
    assert!(
        msg.contains("not the one you verified"),
        "unexpected error: {msg}"
    );
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
/// rather than leaving the client in "Connecting…" forever.
#[tokio::test]
async fn a_wrong_server_password_is_refused_and_reported() {
    let server = start_server(Some("correct-horse-battery".to_string())).await;
    let mut mgr = PartyManager::new();
    let password = Some("wrong-password-entirely".to_string());
    let sid = connect_pinned(&mut mgr, &server, "alice", password, &client_key()).await;

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

/// A private channel must not appear in a non-member's channel list, nor its
/// history be readable by guessing the id. The hub sends every connection the
/// same frame, so the *listing* is what has to be per member.
#[tokio::test]
async fn a_private_channel_is_invisible_to_a_non_member() {
    let server = start_server(None).await;
    let (mut owner, owner_sid) = join_owner(&server, "owner").await;
    let (mut bob, bob_sid) = join(&server, "bob", None).await;

    wait_members(&mut owner, owner_sid, 2).await;
    owner
        .create_channel_of_kind(
            owner_sid,
            "secret".to_string(),
            ChannelKind::Private,
            Vec::new(),
        )
        .expect("create private channel");
    let secret = wait_channel(&mut owner, owner_sid, "secret").await;
    owner
        .post(owner_sid, secret, "for my eyes only".to_string())
        .expect("post");
    pump_until(&mut owner, "post lands", |m| {
        !texts_in(m, owner_sid, secret).is_empty()
    })
    .await;

    idle(&mut [&mut bob], 40, 25).await;
    assert!(
        channel_named(&bob, bob_sid, "secret").is_none(),
        "a private channel must never be broadcast to a non-member"
    );

    bob.fetch_history(bob_sid, secret).ok();
    idle(&mut [&mut bob], 40, 25).await;
    assert!(
        texts_in(&bob, bob_sid, secret).is_empty(),
        "a non-member must not be able to read a private channel by guessing its id"
    );
}

/// Upload, see it listed, download it back byte-exact; a later member who can
/// see it may fetch it; deleting the reference stops the download.
#[tokio::test]
async fn a_file_is_shared_listed_and_downloadable_only_by_those_who_may_see_it() {
    let server = start_server(None).await;
    let (mut owner, sid) = join_owner(&server, "owner").await;
    let general = wait_channel(&mut owner, sid, "general").await;

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
    pump_until(&mut owner, "file listed", |m| has_file(m, sid, "notes.bin")).await;
    let entry = owner
        .server(sid)
        .unwrap()
        .files
        .iter()
        .find(|f| f.name == "notes.bin")
        .cloned()
        .expect("the uploaded file");
    assert_eq!(entry.size as usize, bytes.len());

    // The client re-hashes what arrives, so a mismatch fails the download.
    let rx = owner
        .request_download(sid, entry.hash.clone())
        .expect("request download");
    let downloaded = download(&mut owner, rx).await.expect("download succeeds");
    assert_eq!(downloaded, bytes, "the downloaded bytes must be the file");

    // Access follows the file reference, not who uploaded it.
    let (mut bob, bob_sid) = join(&server, "bob", None).await;
    bob.refresh_files(bob_sid).expect("list files");
    pump_until(&mut bob, "bob sees the file", |m| {
        has_file(m, bob_sid, "notes.bin")
    })
    .await;
    let rx = bob
        .request_download(bob_sid, entry.hash.clone())
        .expect("request download");
    assert_eq!(
        download(&mut bob, rx).await.expect("download succeeds"),
        bytes
    );

    // Deleting the reference stops the download; the message stays put (seqs
    // are what clients merge history on).
    owner
        .delete_file(sid, entry.hash.clone(), general)
        .expect("delete the share");
    // The listing is pulled, not pushed, so ask again while waiting.
    let deadline = Instant::now() + STEP_TIMEOUT;
    loop {
        owner.poll_events();
        if owner.server(sid).is_some() && !has_file(&owner, sid, "notes.bin") {
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
    let after = download(&mut bob, rx).await;
    assert!(
        after.is_err(),
        "a released file must stop downloading, got {} bytes",
        after.map(|b| b.len()).unwrap_or(0)
    );
}

/// A post the server refuses is taken back off the screen: it was appended
/// optimistically, and leaving it would claim a delivery that never happened.
#[tokio::test]
async fn a_refused_post_is_removed_rather_than_left_looking_delivered() {
    let server = start_server(None).await;
    let (mut owner, owner_sid) = join_owner(&server, "owner").await;
    let (mut bob, bob_sid) = join(&server, "bob", None).await;
    wait_members(&mut owner, owner_sid, 2).await;

    // An announce channel is readable by all and writable by admins only.
    owner
        .create_channel_of_kind(
            owner_sid,
            "news".to_string(),
            ChannelKind::Announce,
            Vec::new(),
        )
        .expect("create announce channel");
    let news = wait_channel(&mut bob, bob_sid, "news").await;

    bob.post(bob_sid, news, "let me in".to_string())
        .expect("the send itself is queued");
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

/// Roles are enforced server-side; hiding controls in the UI is politeness.
#[tokio::test]
async fn a_member_cannot_grant_themselves_a_role() {
    let server = start_server(None).await;
    let (mut owner, owner_sid) = join_owner(&server, "owner").await;
    let (mut bob, bob_sid) = join(&server, "bob", None).await;
    wait_members(&mut owner, owner_sid, 2).await;

    let bob_id = bob.server(bob_sid).unwrap().member_id.unwrap();
    assert_eq!(bob.my_role(bob_sid), Some(Role::Member));

    bob.set_role(bob_sid, bob_id, Role::Admin)
        .expect("the request is sent");
    idle(&mut [&mut bob], 40, 25).await;
    assert_eq!(
        bob.my_role(bob_sid),
        Some(Role::Member),
        "the server must refuse a self-promotion"
    );
}

/// The accept loop logs and skips failed connections (hang-ups, garbage) rather
/// than ending the listener.
#[tokio::test]
async fn the_server_keeps_serving_after_failed_connections() {
    let server = start_server(None).await;

    // Under `ratelimit::MAX_CONNECTIONS` (10 per 30 s per address), leaving
    // room for the join's own two connections: this tests the accept loop, not
    // the rate limiter.
    for _ in 0..3 {
        if let Ok(stream) = tokio::net::TcpStream::connect(&server.address).await {
            drop(stream);
        }
    }
    for _ in 0..3 {
        if let Ok(mut stream) = tokio::net::TcpStream::connect(&server.address).await {
            use tokio::io::AsyncWriteExt;
            let _ = stream.write_all(b"garbage, not a v3 handshake").await;
            drop(stream);
        }
    }

    // A real member can still join and use it.
    let (mut mgr, sid) = join(&server, "alice", None).await;
    let general = wait_channel(&mut mgr, sid, "general").await;
    mgr.post(sid, general, "still here".to_string())
        .expect("post");
    pump_until(&mut mgr, "post lands", |m| {
        texts_in(m, sid, general).contains(&"still here".to_string())
    })
    .await;
}

/// Regression: the client's connection task built a fresh, non-cancel-safe
/// `recv_framed` each select, so a request winning the race mid-response lost
/// bytes and dropped the connection. A chunked download interleaved with posts
/// is exactly that race.
#[tokio::test]
async fn sending_while_a_large_download_streams_in_keeps_the_connection() {
    let server = start_server(None).await;
    let (mut mgr, sid) = join(&server, "alice", None).await;
    let general = wait_channel(&mut mgr, sid, "general").await;

    // Past the inline limit, so it moves in chunks both ways.
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
    let hash = messenger_core::party::blob_hash(&data);

    let mut rx = mgr.request_download(sid, hash).expect("download");
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
    assert_eq!(
        downloaded.expect("download completes"),
        data,
        "the file came back intact"
    );
    pump_until(&mut mgr, "every post confirmed", |m| {
        let s = m.server(sid).unwrap();
        s.status == PartyStatus::Joined
            && s.messages[&general]
                .iter()
                .filter(|e| matches!(e.payload, MessagePayload::Text(_)))
                .all(|e| e.seq != 0)
    })
    .await;
    assert_eq!(mgr.server(sid).unwrap().status, PartyStatus::Joined);
}

/// SRV-01: a channel of maximum-size messages made the first history page too
/// big to send, so the community became unjoinable. Pages are now budgeted by
/// bytes, and the client keeps asking when a page was trimmed for size.
#[tokio::test]
async fn a_channel_of_maximum_size_messages_is_joinable_and_pages_to_the_end() {
    let server = start_server(None).await;
    let (mut alice, sid) = join(&server, "alice", None).await;
    let general = wait_channel(&mut alice, sid, "general").await;
    let alice_id = alice.server(sid).unwrap().member_id.unwrap();

    // Well past what one count-capped page could have carried in a frame.
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
        channel_named(m, bob_sid, "general").is_some_and(|c| texts_in(m, bob_sid, c).len() >= POSTS)
    })
    .await;
    let texts = texts_in(&bob, bob_sid, general);
    assert_eq!(texts.len(), POSTS);
    for (i, t) in texts.iter().enumerate() {
        assert!(t.starts_with(&format!("{i:04}")), "in order");
    }
    assert_eq!(bob.server(bob_sid).unwrap().status, PartyStatus::Joined);
}

/// SRV-04: the operator becomes Owner by presenting the one-time claim token in
/// the password field; the token is then spent, and the first joiner is not
/// promoted.
#[tokio::test]
async fn the_owner_is_whoever_presents_the_claim_token_not_whoever_is_first() {
    let password = Uuid::new_v4().to_string();
    let server = start_server(Some(password.clone())).await;
    let token = server
        .claim_token
        .clone()
        .expect("a fresh server has no owner");

    let (mut early, early_sid) = join(&server, "early-bird", Some(password.clone())).await;
    pump_until(&mut early, "directory", |m| m.my_role(early_sid).is_some()).await;
    assert_eq!(early.my_role(early_sid), Some(Role::Member));

    let (mut op, op_sid) = join(&server, "operator", Some(token)).await;
    pump_until(&mut op, "owner role", |m| {
        m.my_role(op_sid) == Some(Role::Owner)
    })
    .await;
    assert_eq!(server.state.lock().await.owner_claim_token(), None, "spent");
}
