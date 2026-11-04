use super::*;
use messenger_core::party::{PartyResponse, MAX_HISTORY_BATCH, MAX_PARTY_PAYLOAD_BYTES};

/// Join as the community's Owner the way the operator does: with the one-time
/// owner claim token in place of the password. Being first no longer makes
/// anyone the owner (SRV-04).
fn join_owner(state: &mut PartyState, name: &str) -> Uuid {
    let token = state
        .owner_claim_token()
        .expect("the community has no owner yet")
        .to_string();
    let id = state.join(name, Some(&token), None).unwrap();
    assert_eq!(state.role_of(id), Role::Owner);
    id
}

#[test]
fn open_server_join_succeeds_and_lists_member() {
    let mut state = PartyState::new("Open", None);
    let id = state.join("alice", None, None).expect("join");
    assert!(state.is_member(id));
    let members = state.members();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].username, "alice");
    assert!(members[0].online);
}

/// The comparison must accept exactly the right password and nothing else —
/// including the cases the `Option` equality it replaced got right, so
/// switching to a constant-time compare changed only the timing.
#[test]
fn password_matches_covers_every_combination() {
    assert!(
        password_matches(None, None),
        "open server, no password given"
    );
    assert!(
        !password_matches(None, Some("anything")),
        "an open server must not accept a password as if it were configured"
    );
    assert!(
        !password_matches(Some("s3cret"), None),
        "a protected server must not accept a missing password"
    );
    assert!(password_matches(Some("s3cret"), Some("s3cret")));
    assert!(!password_matches(Some("s3cret"), Some("s3crey")));
    assert!(
        !password_matches(Some("s3cret"), Some("s3cre")),
        "a correct prefix is not a match"
    );
    assert!(
        !password_matches(Some("s3cret"), Some("s3cretx")),
        "a correct prefix plus extra is not a match"
    );
    assert!(password_matches(Some(""), Some("")));
}

#[test]
fn password_protected_join_enforced() {
    let mut state = PartyState::new("Locked", Some("s3cret".to_string()));
    assert_eq!(
        state.join("alice", None, None),
        Err(JoinError::WrongPassword)
    );
    assert_eq!(
        state.join("alice", Some("nope"), None),
        Err(JoinError::WrongPassword)
    );
    assert!(state.join("alice", Some("s3cret"), None).is_ok());
}

#[test]
fn duplicate_and_empty_usernames_rejected() {
    let mut state = PartyState::new("Open", None);
    state.join("alice", None, None).unwrap();
    assert_eq!(
        state.join("Alice", None, None),
        Err(JoinError::UsernameTaken),
        "username uniqueness is case-insensitive"
    );
    assert_eq!(state.join("   ", None, None), Err(JoinError::EmptyUsername));
}

#[test]
fn oversized_username_rejected_at_the_boundary() {
    let mut state = PartyState::new("Open", None);
    // Exactly at the cap is accepted (counted in Unicode scalar values, so the
    // trailing surrounding whitespace is trimmed first).
    let at_cap = "a".repeat(MAX_USERNAME_CHARS);
    assert!(
        state.join(&at_cap, None, None).is_ok(),
        "{MAX_USERNAME_CHARS} chars is allowed"
    );

    // One past the cap is rejected before the member is ever stored.
    let too_long = "b".repeat(MAX_USERNAME_CHARS + 1);
    assert_eq!(
        state.join(&too_long, None, None),
        Err(JoinError::UsernameTooLong)
    );
    assert_eq!(
        state.members().len(),
        1,
        "the oversized username is not registered"
    );
}

#[test]
fn oversized_channel_name_rejected() {
    let mut state = PartyState::new("Open", None);
    let at_cap = "c".repeat(MAX_CHANNEL_NAME_CHARS);
    assert!(state.create_channel(&at_cap).is_ok());

    let too_long = "d".repeat(MAX_CHANNEL_NAME_CHARS + 1);
    let err = state.create_channel(&too_long).unwrap_err();
    assert!(err.contains("too long"), "unexpected error: {err}");
}

#[test]
fn uploaded_file_names_are_sanitized_against_path_traversal() {
    let mut state = PartyState::new("Open", None);
    let alice = state.join("alice", None, None).unwrap();
    let chan = state.default_channel();

    // A member-chosen name must never be able to escape a client's download
    // directory when later joined onto it.
    let env = state
        .post_file(
            alice,
            chan,
            "..\\..\\Startup\\evil.exe".to_string(),
            "application/octet-stream".to_string(),
            b"payload".to_vec(),
        )
        .unwrap();
    match &env.payload {
        MessagePayload::File(f) => {
            assert!(
                !f.name.contains("..") && !f.name.contains('\\') && !f.name.contains('/'),
                "stored name must be traversal-safe, got {:?}",
                f.name
            );
        }
        other => panic!("expected a File payload, got {other:?}"),
    }
}

#[test]
fn oversized_message_text_rejected_in_channels_and_dms() {
    let mut state = PartyState::new("Open", None);
    let alice = state.join("alice", None, None).unwrap();
    let bob = state.join("bob", None, None).unwrap();
    let chan = state.default_channel();

    let too_long = "x".repeat(MAX_MESSAGE_TEXT_BYTES + 1);
    assert!(
        state.post_message(alice, chan, too_long.clone()).is_err(),
        "an oversized channel message must be rejected"
    );
    assert!(
        state.post_dm(alice, bob, too_long).is_err(),
        "an oversized DM must be rejected"
    );
    // Nothing oversized was persisted to the channel.
    assert!(state.history_since(alice, chan, 0).is_empty());

    // A message exactly at the cap is still accepted.
    let at_cap = "y".repeat(MAX_MESSAGE_TEXT_BYTES);
    assert!(state.post_message(alice, chan, at_cap).is_ok());
}

#[test]
fn reconnecting_with_same_fingerprint_reuses_the_member() {
    let mut state = PartyState::new("Open", None);
    let id1 = state
        .join("alice", None, Some("FP-alice".to_string()))
        .unwrap();

    // Simulate a dropped connection.
    state.set_online(id1, false);

    // The same identity reconnects under the same username: it must reuse the
    // existing membership (same id, back online), not be locked out by its own
    // offline ghost entry as "username taken".
    let id2 = state
        .join("alice", None, Some("FP-alice".to_string()))
        .expect("returning member must be allowed to rejoin");
    assert_eq!(id1, id2, "a returning identity keeps its member id");
    assert_eq!(state.members().len(), 1, "no duplicate member is created");
    assert!(
        state.members().iter().find(|m| m.id == id1).unwrap().online,
        "the reused member is marked online again"
    );
}

#[test]
fn a_different_identity_cannot_take_an_existing_username() {
    let mut state = PartyState::new("Open", None);
    state
        .join("alice", None, Some("FP-alice".to_string()))
        .unwrap();
    // A different fingerprint claiming the same username is still rejected
    // (prevents impersonation of an established member).
    assert_eq!(
        state.join("alice", None, Some("FP-mallory".to_string())),
        Err(JoinError::UsernameTaken)
    );
}

#[test]
fn posting_assigns_monotonic_per_channel_seq() {
    let mut state = PartyState::new("Open", None);
    let alice = state.join("alice", None, None).unwrap();
    let chan = state.default_channel();

    let m1 = state
        .post_message(alice, chan, "first".to_string())
        .unwrap();
    let m2 = state
        .post_message(alice, chan, "second".to_string())
        .unwrap();
    assert_eq!(m1.seq, 1);
    assert_eq!(m2.seq, 2);
    assert_eq!(m2.sender, alice);
    assert_eq!(m2.payload, MessagePayload::Text("second".to_string()));
}

#[test]
fn posting_rejects_non_members_and_unknown_channels() {
    let mut state = PartyState::new("Open", None);
    let stranger = Uuid::new_v4();
    let chan = state.default_channel();
    assert!(state.post_message(stranger, chan, "x".to_string()).is_err());

    let alice = state.join("alice", None, None).unwrap();
    assert!(state
        .post_message(alice, Uuid::new_v4(), "x".to_string())
        .is_err());
}

#[test]
fn history_since_supports_offline_catch_up() {
    let mut state = PartyState::new("Open", None);
    let alice = state.join("alice", None, None).unwrap();
    let chan = state.default_channel();
    for i in 0..5 {
        state.post_message(alice, chan, format!("msg {i}")).unwrap();
    }

    // Whole channel.
    assert_eq!(state.history_since(alice, chan, 0).len(), 5);
    // A member who last saw seq=3 gets only 4 and 5.
    let missed = state.history_since(alice, chan, 3);
    assert_eq!(missed.len(), 2);
    assert_eq!(missed[0].seq, 4);
    assert_eq!(missed[1].seq, 5);
    // Caught up: nothing new.
    assert!(state.history_since(alice, chan, 5).is_empty());
    // Unknown channel.
    assert!(state.history_since(alice, Uuid::new_v4(), 0).is_empty());
}

#[test]
fn presence_can_be_toggled() {
    let mut state = PartyState::new("Open", None);
    let alice = state.join("alice", None, None).unwrap();
    state.set_online(alice, false);
    assert!(!state.members()[0].online);
    state.set_online(alice, true);
    assert!(state.members()[0].online);
}

#[test]
fn durable_state_persists_and_reloads() {
    let dir = tempfile::tempdir().unwrap();
    let channel;
    {
        let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
        channel = state.default_channel();
        let alice = state
            .join("alice", None, Some("FINGERPRINT".to_string()))
            .unwrap();
        state
            .post_message(alice, channel, "persisted msg".to_string())
            .unwrap();
    } // dropped; everything was written through to the database on each mutation

    let reloaded = PartyState::load("Srv", None, dir.path()).unwrap();
    // Member directory survived; presence resets to offline after reload.
    let members = reloaded.members();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].username, "alice");
    assert!(!members[0].online, "presence must reset to offline on load");
    // Channel identity + history survived.
    assert_eq!(reloaded.default_channel(), channel);
    let member = reloaded.members()[0].id;
    let history = reloaded.history_since(member, channel, 0);
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0].payload,
        MessagePayload::Text("persisted msg".to_string())
    );
}

#[test]
fn load_without_existing_snapshot_starts_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let state = PartyState::load("Srv", None, dir.path()).unwrap();
    assert!(state.members().is_empty());
    assert_eq!(state.channels().len(), 1); // default `general`
}

#[test]
fn direct_messages_share_one_order_independent_thread() {
    let mut state = PartyState::new("Open", None);
    let alice = state.join("alice", None, None).unwrap();
    let bob = state.join("bob", None, None).unwrap();
    let thread = messenger_core::party::dm_thread_id(alice, bob);

    let m1 = state.post_dm(alice, bob, "hi bob".to_string()).unwrap();
    assert_eq!(m1.channel, thread);
    assert_eq!(m1.sender, alice);
    assert_eq!(m1.seq, 1);
    let m2 = state.post_dm(bob, alice, "hey alice".to_string()).unwrap();
    assert_eq!(m2.channel, thread);
    assert_eq!(m2.seq, 2);

    assert_eq!(state.dm_history(thread, 0).len(), 2);
    assert_eq!(state.dm_history(thread, 1).len(), 1);
    assert!(state.dm_history(Uuid::new_v4(), 0).is_empty());
}

#[test]
fn create_channel_adds_unique_named_channels() {
    let mut state = PartyState::new("Open", None);
    assert_eq!(state.channels().len(), 1); // general
    let info = state.create_channel("random").unwrap();
    assert_eq!(info.name, "random");
    assert_eq!(state.channels().len(), 2);
    // Duplicate (case-insensitive) and empty names are rejected.
    assert!(state.create_channel("Random").is_err());
    assert!(state.create_channel("   ").is_err());
    assert_eq!(state.channels().len(), 2);
}

#[test]
fn dm_rejects_non_members() {
    let mut state = PartyState::new("Open", None);
    let alice = state.join("alice", None, None).unwrap();
    let stranger = Uuid::new_v4();
    assert!(state.post_dm(alice, stranger, "x".to_string()).is_err());
    assert!(state.post_dm(stranger, alice, "x".to_string()).is_err());
}

#[test]
fn dm_history_persists_across_reload() {
    let dir = tempfile::tempdir().unwrap();
    let thread;
    {
        let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
        let alice = s.join("alice", None, None).unwrap();
        let bob = s.join("bob", None, None).unwrap();
        thread = messenger_core::party::dm_thread_id(alice, bob);
        s.post_dm(alice, bob, "secret".to_string()).unwrap();
    }
    let s = PartyState::load("Srv", None, dir.path()).unwrap();
    assert_eq!(s.dm_history(thread, 0).len(), 1);
}

/// A blob whose bytes cannot be written must fail the upload outright.
///
/// The failure this guards is silent and permanent: the write error used to
/// be logged and swallowed, so the blob was recorded, the file message was
/// posted and broadcast to the whole channel, and every download of it
/// answered "unknown file" from then on — with the sender told it worked.
#[test]
fn an_unwritable_blob_store_refuses_the_upload_instead_of_posting_a_dead_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
    let alice = state.join("alice", None, None).unwrap();
    let channel = state.default_channel();

    // Replace the blob directory with a regular file, so writing any blob
    // path inside it fails at the filesystem level.
    let blob_dir = dir.path().join(BLOB_DIR);
    std::fs::remove_dir_all(&blob_dir).unwrap();
    std::fs::write(&blob_dir, b"not a directory").unwrap();

    let err = state
        .post_file(
            alice,
            channel,
            "report.pdf".to_string(),
            "application/pdf".to_string(),
            b"payload".to_vec(),
        )
        .expect_err("an upload whose bytes cannot be stored must fail");
    assert!(
        err.contains("could not store"),
        "expected a storage error, got: {err}"
    );

    // Nothing was recorded: no message in the channel, and no blob to serve.
    assert!(state.history_since(alice, channel, 0).is_empty());
    assert!(state.blob_bytes(&blob_hash(b"payload")).is_none());
}

#[test]
fn created_channels_and_their_order_persist_across_reload() {
    let dir = tempfile::tempdir().unwrap();
    let (general, random_id);
    {
        let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
        general = s.default_channel();
        let alice = s.join("alice", None, None).unwrap();
        random_id = s.create_channel("random").unwrap().id;
        s.post_message(alice, random_id, "in random".to_string())
            .unwrap();
    }
    let s = PartyState::load("Srv", None, dir.path()).unwrap();
    let channels = s.channels();
    assert_eq!(channels.len(), 2);
    // Order is preserved: general first, then the created channel.
    assert_eq!(s.default_channel(), general);
    assert_eq!(channels[0].id, general);
    assert_eq!(channels[1].id, random_id);
    // History of the created channel survived.
    let member = s.members()[0].id;
    let history = s.history_since(member, random_id, 0);
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0].payload,
        MessagePayload::Text("in random".to_string())
    );
}

#[test]
fn legacy_json_snapshot_is_imported_once() {
    let dir = tempfile::tempdir().unwrap();

    // Hand-write a legacy snapshot in the old on-disk shape.
    let member_id = Uuid::new_v4();
    let channel_id = Uuid::new_v4();
    let snapshot = serde_json::json!({
        "members": [
            { "id": member_id, "username": "legacy_user", "fingerprint": "FP" }
        ],
        "channels": [
            {
                "id": channel_id,
                "name": "general",
                "kind": "Public",
                "messages": [
                    {
                        "tier": "Administered",
                        "sender": member_id,
                        "channel": channel_id,
                        "seq": 1,
                        "timestamp": 1700000000000u64,
                        "payload": { "Text": "hello from the past" }
                    }
                ]
            }
        ],
        "dm_threads": []
    });
    std::fs::write(
        dir.path().join("party_state.json"),
        serde_json::to_string(&snapshot).unwrap(),
    )
    .unwrap();

    // First load imports the snapshot into SQLite and renames the file.
    let state = PartyState::load("Srv", None, dir.path()).unwrap();
    assert_eq!(state.members().len(), 1);
    assert_eq!(state.members()[0].username, "legacy_user");
    assert_eq!(state.default_channel(), channel_id);
    let member = state.members()[0].id;
    let history = state.history_since(member, channel_id, 0);
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0].payload,
        MessagePayload::Text("hello from the past".to_string())
    );
    assert!(
        !dir.path().join("party_state.json").exists(),
        "legacy snapshot should be renamed after import"
    );
    assert!(dir.path().join("party_state.json.imported").exists());

    // A second load must not re-import (the database is now authoritative).
    drop(state);
    let reloaded = PartyState::load("Srv", None, dir.path()).unwrap();
    assert_eq!(reloaded.members().len(), 1);
    let member = reloaded.members()[0].id;
    assert_eq!(reloaded.history_since(member, channel_id, 0).len(), 1);
}

fn file_payload(env: &Envelope) -> &FileMeta {
    match &env.payload {
        MessagePayload::File(f) => f,
        other => panic!("expected a File payload, got {other:?}"),
    }
}

#[test]
fn post_file_stores_blob_and_posts_a_file_message() {
    let mut state = PartyState::new("Open", None);
    let alice = state.join("alice", None, None).unwrap();
    let chan = state.default_channel();

    let env = state
        .post_file(
            alice,
            chan,
            "hi.txt".into(),
            "text/plain".into(),
            b"hello".to_vec(),
        )
        .expect("post_file");
    assert_eq!(env.seq, 1);
    let meta = file_payload(&env);
    assert_eq!(meta.name, "hi.txt");
    assert_eq!(meta.size, 5);
    assert_eq!(meta.hash, blob_hash(b"hello"));

    // The bytes are retrievable by hash, and the message is in channel history.
    assert_eq!(state.blob_bytes(&meta.hash), Some(b"hello".to_vec()));
    assert_eq!(state.history_since(alice, chan, 0).len(), 1);
}

#[test]
fn identical_uploads_are_deduplicated_by_content() {
    let mut state = PartyState::new("Open", None);
    let alice = state.join("alice", None, None).unwrap();
    let chan = state.default_channel();

    let a = state
        .post_file(
            alice,
            chan,
            "a.bin".into(),
            "application/octet-stream".into(),
            b"same".to_vec(),
        )
        .unwrap();
    let b = state
        .post_file(
            alice,
            chan,
            "b.bin".into(),
            "application/octet-stream".into(),
            b"same".to_vec(),
        )
        .unwrap();

    // Two messages, two names, but one shared content-addressed blob.
    assert_eq!(file_payload(&a).hash, file_payload(&b).hash);
    assert_eq!(state.blobs.len(), 1);
    assert_eq!(state.blobs[&file_payload(&a).hash].refcount, 2);
    assert_eq!(state.history_since(alice, chan, 0).len(), 2);
}

#[test]
fn oversized_and_empty_uploads_are_rejected() {
    let mut state = PartyState::new("Open", None);
    let alice = state.join("alice", None, None).unwrap();
    let chan = state.default_channel();

    assert!(state
        .post_file(alice, chan, "empty".into(), "text/plain".into(), Vec::new())
        .is_err());
    let too_big = vec![0u8; MAX_INLINE_FILE_BYTES + 1];
    assert!(state
        .post_file(
            alice,
            chan,
            "big".into(),
            "application/octet-stream".into(),
            too_big
        )
        .is_err());
    // Nothing was stored.
    assert!(state.blobs.is_empty());
    assert!(state.history_since(alice, chan, 0).is_empty());
}

#[test]
fn post_file_rejects_non_members_and_unknown_channels() {
    let mut state = PartyState::new("Open", None);
    let stranger = Uuid::new_v4();
    let chan = state.default_channel();
    assert!(state
        .post_file(
            stranger,
            chan,
            "x".into(),
            "text/plain".into(),
            b"x".to_vec()
        )
        .is_err());

    let alice = state.join("alice", None, None).unwrap();
    assert!(state
        .post_file(
            alice,
            Uuid::new_v4(),
            "x".into(),
            "text/plain".into(),
            b"x".to_vec()
        )
        .is_err());
}

#[test]
fn file_dm_is_stored_in_the_thread() {
    let mut state = PartyState::new("Open", None);
    let alice = state.join("alice", None, None).unwrap();
    let bob = state.join("bob", None, None).unwrap();
    let thread = messenger_core::party::dm_thread_id(alice, bob);

    let env = state
        .post_file_dm(
            alice,
            bob,
            "secret.txt".into(),
            "text/plain".into(),
            b"psst".to_vec(),
        )
        .expect("post_file_dm");
    assert_eq!(env.channel, thread);
    assert_eq!(file_payload(&env).name, "secret.txt");
    assert_eq!(state.dm_history(thread, 0).len(), 1);
    assert_eq!(
        state.blob_bytes(&file_payload(&env).hash),
        Some(b"psst".to_vec())
    );
}

#[test]
fn a_new_blob_past_the_storage_ceiling_is_rejected() {
    let mut state = PartyState::new("Open", None);
    let alice = state.join("alice", None, None).unwrap();
    let chan = state.default_channel();
    state.set_max_blob_bytes(8);

    // First upload fits under the 8-byte ceiling.
    state
        .post_file(
            alice,
            chan,
            "a.bin".into(),
            "application/octet-stream".into(),
            vec![1u8; 6],
        )
        .expect("a blob under the ceiling is accepted");

    // A distinct second blob would push the total to 12 bytes: rejected,
    // stored nowhere, and no message is posted for it.
    let err = state
        .post_file(
            alice,
            chan,
            "b.bin".into(),
            "application/octet-stream".into(),
            vec![2u8; 6],
        )
        .expect_err("a blob past the ceiling must be rejected");
    assert!(err.contains("storage"), "error should say why: {err}");
    assert_eq!(state.blobs.len(), 1, "the rejected blob must not be stored");
    assert_eq!(
        state.history_since(alice, chan, 0).len(),
        1,
        "no message is posted for a rejected upload"
    );

    // Re-uploading already-stored content adds no bytes, so deduplicated
    // uploads still succeed at the ceiling.
    state
        .post_file(
            alice,
            chan,
            "a-again.bin".into(),
            "application/octet-stream".into(),
            vec![1u8; 6],
        )
        .expect("re-upload of stored content adds no bytes and stays allowed");
}

#[test]
fn download_is_denied_for_a_dm_file_a_member_cannot_see() {
    let mut state = PartyState::new("Open", None);
    let alice = state.join("alice", None, None).unwrap();
    let bob = state.join("bob", None, None).unwrap();
    let mallory = state.join("mallory", None, None).unwrap();

    // Alice sends Bob a private file over a DM.
    let env = state
        .post_file_dm(
            alice,
            bob,
            "secret.pdf".into(),
            "application/pdf".into(),
            b"top secret".to_vec(),
        )
        .unwrap();
    let hash = file_payload(&env).hash.clone();

    // Both participants can download it...
    assert_eq!(
        state.blob_bytes_for(alice, &hash),
        Some(b"top secret".to_vec())
    );
    assert_eq!(
        state.blob_bytes_for(bob, &hash),
        Some(b"top secret".to_vec())
    );
    // ...but a third member cannot, even if they somehow learn the hash.
    assert_eq!(
        state.blob_bytes_for(mallory, &hash),
        None,
        "a non-participant must not be able to download a private DM file"
    );
}

#[test]
fn download_of_a_channel_file_is_allowed_for_any_member() {
    let mut state = PartyState::new("Open", None);
    let alice = state.join("alice", None, None).unwrap();
    let bob = state.join("bob", None, None).unwrap();
    let chan = state.default_channel();

    let env = state
        .post_file(
            alice,
            chan,
            "pic.png".into(),
            "image/png".into(),
            b"pixels".to_vec(),
        )
        .unwrap();
    let hash = file_payload(&env).hash.clone();

    // A public-channel file is downloadable by any member, not just the poster.
    assert_eq!(state.blob_bytes_for(bob, &hash), Some(b"pixels".to_vec()));
    // An unknown hash is denied.
    assert_eq!(state.blob_bytes_for(bob, "deadbeef"), None);
}

#[test]
fn blobs_and_file_messages_persist_across_reload() {
    let dir = tempfile::tempdir().unwrap();
    let (chan, hash);
    {
        let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
        chan = s.default_channel();
        let alice = s.join("alice", None, None).unwrap();
        let env = s
            .post_file(
                alice,
                chan,
                "doc.txt".into(),
                "text/plain".into(),
                b"durable".to_vec(),
            )
            .unwrap();
        hash = file_payload(&env).hash.clone();
    }
    let s = PartyState::load("Srv", None, dir.path()).unwrap();
    // The blob bytes and the file message both survived.
    assert_eq!(s.blob_bytes(&hash), Some(b"durable".to_vec()));
    let member = s.members()[0].id;
    let history = s.history_since(member, chan, 0);
    assert_eq!(history.len(), 1);
    assert_eq!(file_payload(&history[0]).name, "doc.txt");
    assert_eq!(s.blobs[&hash].refcount, 1);
}

/// A disk-backed server must not hold every blob's bytes in memory: the
/// storage ceiling would double as a resident-memory ceiling.
#[test]
fn disk_backed_blobs_are_not_held_in_memory() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
    let alice = s.join("alice", None, None).unwrap();
    let chan = s.default_channel();
    let env = s
        .post_file(
            alice,
            chan,
            "doc.txt".into(),
            "text/plain".into(),
            b"durable".to_vec(),
        )
        .unwrap();
    let hash = file_payload(&env).hash.clone();

    assert!(
        s.blobs[&hash].data.is_none(),
        "a disk-backed blob must not keep its bytes resident"
    );
    // …and is still served correctly, read back from disk on demand.
    assert_eq!(s.blob_bytes(&hash), Some(b"durable".to_vec()));
}

/// `ChannelKind` was stored, shipped to clients, and enforced nowhere; then
/// it was made to fail closed, which left three of the four kinds unusable.
/// Each kind now has a real rule, and an ordinary member is held to it.
#[test]
fn non_public_channels_are_enforced_rather_than_decorative() {
    let mut state = PartyState::new("Open", None);
    // The first member is the owner, so join a second one to test the
    // ordinary-member path.
    let _owner = join_owner(&mut state, "owner");
    let bob = state.join("bob", None, None).unwrap();
    let public = state.default_channel();

    for (kind, label) in [
        (ChannelKind::Private, "private"),
        (ChannelKind::Locked, "locked"),
        (ChannelKind::Announce, "announce"),
    ] {
        let id = Uuid::new_v4();
        state.channels.push(Channel {
            id,
            name: label.to_string(),
            kind,
            members: Vec::new(),
            history: History::new(),
        });
        assert!(
            state.post_message(bob, id, "hi".to_string()).is_err(),
            "{label}: an ordinary member must not be able to post"
        );
        assert!(
            state
                .post_file(bob, id, "f".into(), "text/plain".into(), b"x".to_vec())
                .is_err(),
            "{label}: an ordinary member must not be able to upload"
        );
    }

    // A private channel they are not in is not readable, and says nothing
    // about whether it exists.
    let private = state
        .channels
        .iter()
        .find(|c| c.kind == ChannelKind::Private)
        .unwrap()
        .id;
    assert!(state.history_since(bob, private, 0).is_empty());
    assert!(
        !state.channels_for(bob).iter().any(|c| c.id == private),
        "a private channel must not even be listed to a non-member"
    );
    // The public channel is unaffected.
    assert!(state.post_message(bob, public, "hi".to_string()).is_ok());
    assert_eq!(state.history_since(bob, public, 0).len(), 1);
}

/// The kinds are restrictions on ordinary members, not on the people who
/// administer the server — otherwise nobody could post an announcement to an
/// announcement channel.
#[test]
fn admins_can_post_where_ordinary_members_cannot() {
    let mut state = PartyState::new("Open", None);
    let owner = join_owner(&mut state, "owner");
    assert_eq!(state.role_of(owner), Role::Owner);

    let announce = state
        .create_channel_of_kind(owner, "news", ChannelKind::Announce, vec![])
        .unwrap();
    assert!(state
        .post_message(owner, announce.id, "release day".to_string())
        .is_ok());

    let bob = state.join("bob", None, None).unwrap();
    assert_eq!(state.role_of(bob), Role::Member);
    assert!(state
        .post_message(bob, announce.id, "me too".to_string())
        .is_err());
    // But bob can still read it — that is the point of an announce channel.
    assert_eq!(state.history_since(bob, announce.id, 0).len(), 1);
}

/// A private channel is readable and postable by the members on its list.
#[test]
fn private_channel_membership_grants_access() {
    let mut state = PartyState::new("Open", None);
    let owner = join_owner(&mut state, "owner");
    let bob = state.join("bob", None, None).unwrap();
    let carol = state.join("carol", None, None).unwrap();

    let private = state
        .create_channel_of_kind(owner, "secret", ChannelKind::Private, vec![bob])
        .unwrap();
    // The creator is always added, so they cannot lock themselves out.
    assert!(private.members.contains(&owner));
    assert!(private.members.contains(&bob));

    assert!(state
        .post_message(bob, private.id, "in the club".to_string())
        .is_ok());
    assert_eq!(state.history_since(bob, private.id, 0).len(), 1);

    // Carol is not on the list: she cannot post, read, or even see it.
    assert!(state
        .post_message(carol, private.id, "let me in".to_string())
        .is_err());
    assert!(state.history_since(carol, private.id, 0).is_empty());
    assert!(!state.channels_for(carol).iter().any(|c| c.id == private.id));
}

/// Only admins may create the restricted kinds: each of them is a way to
/// control what other members can see or say.
#[test]
fn ordinary_members_may_only_create_public_channels() {
    let mut state = PartyState::new("Open", None);
    let _owner = join_owner(&mut state, "owner");
    let bob = state.join("bob", None, None).unwrap();

    assert!(state
        .create_channel_of_kind(bob, "chat", ChannelKind::Public, vec![])
        .is_ok());
    for kind in [
        ChannelKind::Private,
        ChannelKind::Locked,
        ChannelKind::Announce,
    ] {
        assert!(
            state
                .create_channel_of_kind(bob, "nope", kind, vec![])
                .is_err(),
            "{kind:?} is administrative"
        );
    }
}

/// A file posted in a channel the member cannot read must not be reachable
/// by content hash either — the download endpoint is where access is
/// enforced, because blob storage is shared across the whole server.
#[test]
fn blobs_in_unreadable_channels_are_not_downloadable() {
    let mut state = PartyState::new("Open", None);
    // The first member owns the server, and an admin can read every channel,
    // so the member under test has to be an ordinary one.
    let _owner = join_owner(&mut state, "owner");
    let bob = state.join("bob", None, None).unwrap();
    let chan = state.default_channel();
    let env = state
        .post_file(
            bob,
            chan,
            "secret.txt".into(),
            "text/plain".into(),
            b"classified".to_vec(),
        )
        .unwrap();
    let hash = file_payload(&env).hash.clone();
    assert!(state.blob_bytes_for(bob, &hash).is_some());

    // Flip the channel to Private with bob off the list: he can no longer
    // reach the file, even though he knows its hash and uploaded it.
    {
        let c = state.channel_mut(chan).unwrap();
        c.kind = ChannelKind::Private;
        c.members = Vec::new();
    }
    assert!(
        state.blob_bytes_for(bob, &hash).is_none(),
        "a file in an unreadable channel must not be downloadable by hash"
    );
}

/// SRV-04: the first identity through the door used to become the permanent,
/// undemotable Owner — so whoever reached the address before the operator did
/// owned the community. Being first now grants nothing; presenting the
/// one-time owner claim token does, once.
#[test]
fn being_first_grants_nothing_and_the_claim_token_grants_ownership_once() {
    // Generated, not written down: a credential literal in a test is still a
    // credential literal to a scanner.
    let pw = Uuid::new_v4().to_string();
    let mut state = PartyState::new("Srv", Some(pw.clone()));
    let token = state.owner_claim_token().expect("no owner yet").to_string();

    let squatter = state.join("squatter", Some(pw.as_str()), None).unwrap();
    assert_eq!(state.role_of(squatter), Role::Member, "first is not owner");
    assert!(!state.has_owner());

    // A wrong token is just a wrong password.
    assert_eq!(
        state.join(
            "guess",
            Some(Uuid::new_v4().simple().to_string().as_str()),
            None
        ),
        Err(JoinError::WrongPassword)
    );

    // The token stands in for the password, with surrounding whitespace from
    // a paste tolerated.
    let owner = state
        .join("owner", Some(&format!(" {token}\n")), None)
        .unwrap();
    let bob = state.join("bob", Some(pw.as_str()), None).unwrap();
    assert_eq!(state.role_of(owner), Role::Owner);
    assert_eq!(state.role_of(bob), Role::Member);
    assert_eq!(state.owner_claim_token(), None, "used up");
    assert_eq!(
        state.join("again", Some(&token), None),
        Err(JoinError::WrongPassword),
        "a spent token is not a password"
    );
    let log = state.audit_log(owner, 10).unwrap();
    assert!(log.iter().any(|e| e.action == "role.claim"));
    assert_eq!(
        state.role_of(Uuid::new_v4()),
        Role::Guest,
        "unknown ids fail closed"
    );
}

/// An admin must not be able to mint a peer admin or unseat the operator —
/// either would let them take the community from the person who runs it.
#[test]
fn role_changes_cannot_escalate_past_the_actor() {
    let mut state = PartyState::new("Srv", None);
    let owner = join_owner(&mut state, "owner");
    let bob = state.join("bob", None, None).unwrap();
    let carol = state.join("carol", None, None).unwrap();

    // The owner may appoint an admin.
    state.set_role(owner, bob, Role::Admin).unwrap();
    assert_eq!(state.role_of(bob), Role::Admin);

    // That admin may demote an ordinary member to guest…
    state.set_role(bob, carol, Role::Guest).unwrap();
    assert_eq!(state.role_of(carol), Role::Guest);
    // …but not appoint another admin, not take the owner's seat, and not
    // touch the owner.
    assert!(state.set_role(bob, carol, Role::Admin).is_err());
    assert!(state.set_role(bob, carol, Role::Owner).is_err());
    assert!(state.set_role(bob, owner, Role::Guest).is_err());
    assert_eq!(state.role_of(owner), Role::Owner);

    // An ordinary member may not change roles at all, including their own.
    assert!(state.set_role(carol, carol, Role::Admin).is_err());
    assert!(state.set_role(owner, owner, Role::Admin).is_err());
}

/// A guest is read-only everywhere, before any per-channel rule applies.
#[test]
fn a_guest_cannot_write_anywhere() {
    let mut state = PartyState::new("Srv", None);
    let owner = join_owner(&mut state, "owner");
    let bob = state.join("bob", None, None).unwrap();
    state.set_role(owner, bob, Role::Guest).unwrap();
    let chan = state.default_channel();

    assert!(state.post_message(bob, chan, "hello".to_string()).is_err());
    assert!(state
        .post_file(bob, chan, "f".into(), "text/plain".into(), b"x".to_vec())
        .is_err());
    assert!(state
        .create_channel_of_kind(bob, "mine", ChannelKind::Public, vec![])
        .is_err());
    // Reading is still allowed.
    state
        .post_message(owner, chan, "welcome".to_string())
        .unwrap();
    assert_eq!(state.history_since(bob, chan, 0).len(), 1);
}

/// Reference counting only ever counted up: uploads incremented it and
/// nothing decremented it, so deleting was impossible and storage only grew.
#[test]
fn deleting_the_last_reference_reclaims_the_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
    let owner = join_owner(&mut state, "owner");
    let a = state.default_channel();
    let b = state
        .create_channel_of_kind(owner, "second", ChannelKind::Public, vec![])
        .unwrap()
        .id;

    // The same content shared twice is one blob with two references.
    let env = state
        .post_file(
            owner,
            a,
            "d.txt".into(),
            "text/plain".into(),
            b"same".to_vec(),
        )
        .unwrap();
    let hash = file_payload(&env).hash.clone();
    state
        .post_file(
            owner,
            b,
            "d.txt".into(),
            "text/plain".into(),
            b"same".to_vec(),
        )
        .unwrap();
    assert_eq!(state.blobs[&hash].refcount, 2);
    assert_eq!(state.quota_for(owner).used, 4, "dedup: counted once");

    // Dropping one reference keeps the bytes for the other.
    state.delete_file(owner, &hash, a).unwrap();
    assert_eq!(state.blobs[&hash].refcount, 1);
    assert!(state.blob_bytes(&hash).is_some());

    // Dropping the last one reclaims them, on disk as well as in memory.
    state.delete_file(owner, &hash, b).unwrap();
    assert!(!state.blobs.contains_key(&hash));
    assert!(!dir.path().join(BLOB_DIR).join(&hash).exists());
    assert_eq!(state.quota_for(owner).used, 0);
}

/// Deleting is the uploader's right or an admin's, not everyone's.
#[test]
fn only_the_uploader_or_an_admin_may_delete_a_file() {
    let mut state = PartyState::new("Srv", None);
    let owner = join_owner(&mut state, "owner");
    let bob = state.join("bob", None, None).unwrap();
    let carol = state.join("carol", None, None).unwrap();
    let chan = state.default_channel();

    let env = state
        .post_file(
            bob,
            chan,
            "b.txt".into(),
            "text/plain".into(),
            b"bobs".to_vec(),
        )
        .unwrap();
    let hash = file_payload(&env).hash.clone();

    assert!(
        state.delete_file(carol, &hash, chan).is_err(),
        "a bystander"
    );
    assert!(state.delete_file(owner, &hash, chan).is_ok(), "an admin");

    // And the uploader can delete their own.
    let env = state
        .post_file(
            bob,
            chan,
            "c.txt".into(),
            "text/plain".into(),
            b"more".to_vec(),
        )
        .unwrap();
    let hash = file_payload(&env).hash.clone();
    assert!(state.delete_file(bob, &hash, chan).is_ok());
}

/// A deleted file stops being downloadable immediately, while the message
/// that referenced it stays in history — removing the envelope would
/// renumber the channel and desynchronise every client that had it.
#[test]
fn deleting_a_file_revokes_access_but_keeps_history_intact() {
    let mut state = PartyState::new("Srv", None);
    let owner = join_owner(&mut state, "owner");
    let chan = state.default_channel();
    state
        .post_message(owner, chan, "before".to_string())
        .unwrap();
    let env = state
        .post_file(
            owner,
            chan,
            "x.txt".into(),
            "text/plain".into(),
            b"bytes".to_vec(),
        )
        .unwrap();
    state
        .post_message(owner, chan, "after".to_string())
        .unwrap();
    let hash = file_payload(&env).hash.clone();

    state.delete_file(owner, &hash, chan).unwrap();
    assert!(state.blob_bytes_for(owner, &hash).is_none());

    let history = state.history_since(owner, chan, 0);
    assert_eq!(history.len(), 3, "history keeps its shape");
    assert_eq!(
        history.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![1, 2, 3],
        "and its sequence numbers"
    );
}

/// The server-wide ceiling alone lets the first member to reach it deny the
/// feature to everyone else, so storage is budgeted per member as well.
#[test]
fn a_member_cannot_exceed_their_own_storage_allowance() {
    let mut state = PartyState::new("Srv", None);
    let owner = join_owner(&mut state, "owner");
    let bob = state.join("bob", None, None).unwrap();
    state.set_max_member_blob_bytes(10);
    let chan = state.default_channel();

    assert!(state
        .post_file(
            bob,
            chan,
            "a".into(),
            "text/plain".into(),
            b"12345678".to_vec()
        )
        .is_ok());
    // Distinct content, so it is a new blob rather than a deduplicated
    // re-share, and it would take him past the ceiling.
    let err = state
        .post_file(
            bob,
            chan,
            "b".into(),
            "text/plain".into(),
            b"abcdefgh".to_vec(),
        )
        .expect_err("this would take bob past his allowance");
    assert!(err.contains("allowance"), "got: {err}");
    assert_eq!(state.quota_for(bob).used, 8);
    assert_eq!(state.quota_for(bob).limit, Some(10));

    // Admins are exempt: they are who clears space when it fills.
    assert!(state
        .post_file(
            owner,
            chan,
            "c".into(),
            "text/plain".into(),
            b"abcdefghij!".to_vec()
        )
        .is_ok());
    assert_eq!(state.quota_for(owner).limit, None);

    // Freeing a reference frees the allowance again.
    let hash = blob_hash(b"12345678");
    state.delete_file(bob, &hash, chan).unwrap();
    assert_eq!(state.quota_for(bob).used, 0);
    assert!(state
        .post_file(
            bob,
            chan,
            "d".into(),
            "text/plain".into(),
            b"abcdefgh".to_vec()
        )
        .is_ok());
}

/// The Drive listing shows a member their own files and the ones shared
/// where they can see them — and nothing else.
#[test]
fn the_file_listing_respects_who_may_see_what() {
    let mut state = PartyState::new("Srv", None);
    let owner = join_owner(&mut state, "owner");
    let bob = state.join("bob", None, None).unwrap();
    let public = state.default_channel();

    state
        .post_file(
            bob,
            public,
            "shared.txt".into(),
            "text/plain".into(),
            b"pub".to_vec(),
        )
        .unwrap();
    // A DM between owner and bob.
    state
        .post_file_dm(
            owner,
            bob,
            "dm.txt".into(),
            "text/plain".into(),
            b"dm".to_vec(),
        )
        .unwrap();
    // A private channel bob is not in.
    let secret = state
        .create_channel_of_kind(owner, "secret", ChannelKind::Private, vec![])
        .unwrap()
        .id;
    state
        .post_file(
            owner,
            secret,
            "hidden.txt".into(),
            "text/plain".into(),
            b"sec".to_vec(),
        )
        .unwrap();

    let listing = state.files_for(bob);
    let names: Vec<&str> = listing.iter().map(|f| f.name.as_str()).collect();
    assert!(names.contains(&"shared.txt"));
    assert!(names.contains(&"dm.txt"), "his own DM thread");
    assert!(
        !names.contains(&"hidden.txt"),
        "a private channel he is not in"
    );

    // He may delete only what he uploaded.
    let shared = listing.iter().find(|f| f.name == "shared.txt").unwrap();
    assert!(shared.can_delete);
    assert_eq!(shared.uploader_name, "bob");
    let dm = listing.iter().find(|f| f.name == "dm.txt").unwrap();
    assert!(!dm.can_delete, "the owner uploaded it");
    assert!(dm.is_dm);
}

/// Governance actions are recorded, and the log is admin-only: it says who
/// did what to whom, which is not an ordinary member's to enumerate.
#[test]
fn governance_actions_are_audited_and_the_log_is_admin_only() {
    let mut state = PartyState::new("Srv", None);
    let owner = join_owner(&mut state, "owner");
    let bob = state.join("bob", None, None).unwrap();

    state.set_role(owner, bob, Role::Admin).unwrap();
    state
        .create_channel_of_kind(owner, "news", ChannelKind::Announce, vec![])
        .unwrap();

    let log = state.audit_log(owner, 50).unwrap();
    assert!(log.len() >= 2);
    // Newest first.
    assert_eq!(log[0].action, "channel.create");
    assert_eq!(log[1].action, "role.set");
    assert_eq!(log[1].actor_name, "owner");
    assert!(log[1].detail.contains("bob"));

    // Bob is an admin now, so he may read it too.
    assert!(state.audit_log(bob, 50).is_ok());
    state.set_role(owner, bob, Role::Member).unwrap();
    assert!(state.audit_log(bob, 50).is_err());
}

/// A community that predates roles reloads with nobody able to administer it.
/// It used to hand ownership to whichever member had the lowest id — a random
/// member, permanently (SRV-20). Now it waits for the operator: a claim token
/// is issued, and nobody is promoted until it is used.
#[test]
fn an_upgraded_server_with_no_admin_waits_for_the_operator_to_claim_it() {
    let dir = tempfile::tempdir().unwrap();
    let (alice, bob);
    {
        let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
        alice = s.join("alice", None, Some("fp-alice".into())).unwrap();
        bob = s.join("bob", None, None).unwrap();
        assert!(!s.members.values().any(|m| m.role.is_admin()));
    }

    let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
    assert!(!s.has_owner(), "nobody is promoted by accident");
    let token = s
        .owner_claim_token()
        .expect("a claim is issued")
        .to_string();
    // The operator is an existing member: claiming promotes them in place.
    assert_eq!(
        s.join("alice", Some(&token), Some("fp-alice".into())),
        Ok(alice)
    );
    assert_eq!(s.role_of(alice), Role::Owner);
    assert_eq!(s.role_of(bob), Role::Member);

    // It persists, and a restart issues no new token.
    let again = PartyState::load("Srv", None, dir.path()).unwrap();
    assert_eq!(again.role_of(alice), Role::Owner);
    assert_eq!(again.owner_claim_token(), None);
}

/// `--owner <fingerprint>`: the configured identity becomes the Owner when it
/// joins, no token is issued, and nobody else is promoted by joining first.
#[test]
fn a_configured_owner_fingerprint_owns_the_community() {
    let mut state = PartyState::new("Srv", None);
    state.set_owner_fingerprint(Some("  AB12CD  ".into()));
    assert_eq!(state.owner_claim_token(), None, "no claim is needed");
    let first = state.join("first", None, Some("ffff".into())).unwrap();
    assert_eq!(state.role_of(first), Role::Member);
    let op = state.join("op", None, Some("ab12cd".into())).unwrap();
    assert_eq!(state.role_of(op), Role::Owner, "matched canonically");
}

/// The operator's recovery path: configuring their fingerprint on a community
/// whose ownership went to someone else moves the seat to them and leaves the
/// previous owner an admin, which the new owner may then demote.
#[test]
fn configuring_an_owner_recovers_a_squatted_community() {
    let dir = tempfile::tempdir().unwrap();
    let (squatter, op);
    {
        let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
        squatter = join_owner(&mut s, "squatter");
        op = s.join("op", None, Some("0a0a".into())).unwrap();
        assert_eq!(s.role_of(op), Role::Member);
    }
    let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
    s.set_owner_fingerprint(Some("0A0A".into()));
    assert_eq!(s.role_of(op), Role::Owner);
    assert_eq!(s.role_of(squatter), Role::Admin, "exactly one owner");
    assert!(s.set_role(op, squatter, Role::Member).is_ok());
    let log = s.audit_log(op, 10).unwrap();
    assert!(log.iter().any(|e| e.action == "role.configured"));
    assert!(log.iter().any(|e| e.action == "role.owner_moved"));
}

/// The same content shared twice in one place is two shares, and both have
/// to survive a restart — otherwise the blob's reference count outlives the
/// references, and the last visible one cannot release it.
#[test]
fn repeated_shares_of_one_file_survive_a_reload_individually() {
    let dir = tempfile::tempdir().unwrap();
    let (owner, channel, hash);
    {
        let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
        owner = join_owner(&mut s, "owner");
        channel = s.default_channel();
        let env = s
            .post_file(
                owner,
                channel,
                "twice.txt".into(),
                "text/plain".into(),
                b"same bytes".to_vec(),
            )
            .unwrap();
        hash = file_payload(&env).hash.clone();
        s.post_file(
            owner,
            channel,
            "twice.txt".into(),
            "text/plain".into(),
            b"same bytes".to_vec(),
        )
        .unwrap();
        assert_eq!(s.blobs[&hash].refcount, 2);
        assert_eq!(s.file_refs.len(), 2);
    }

    let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
    assert_eq!(s.file_refs.len(), 2, "both shares came back");
    assert_eq!(s.blobs[&hash].refcount, 2);

    // Releasing them one at a time reclaims the bytes exactly once.
    s.delete_file(owner, &hash, channel).unwrap();
    assert!(
        s.blob_bytes(&hash).is_some(),
        "one reference still holds it"
    );
    s.delete_file(owner, &hash, channel).unwrap();
    assert!(!s.blobs.contains_key(&hash));
    assert!(!dir.path().join(BLOB_DIR).join(&hash).exists());
}

/// Roles, channel kinds, private membership, file provenance and the audit
/// log all have to survive a restart, or the server forgets who runs it.
#[test]
fn governance_state_survives_a_reload() {
    let dir = tempfile::tempdir().unwrap();
    let (owner, bob, private_id, hash);
    {
        let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
        owner = join_owner(&mut s, "owner");
        bob = s.join("bob", None, None).unwrap();
        s.set_role(owner, bob, Role::Admin).unwrap();
        private_id = s
            .create_channel_of_kind(owner, "secret", ChannelKind::Private, vec![bob])
            .unwrap()
            .id;
        let env = s
            .post_file(
                owner,
                private_id,
                "p.txt".into(),
                "text/plain".into(),
                b"private bytes".to_vec(),
            )
            .unwrap();
        hash = file_payload(&env).hash.clone();
    }

    let s = PartyState::load("Srv", None, dir.path()).unwrap();
    assert_eq!(s.role_of(owner), Role::Owner);
    assert_eq!(s.role_of(bob), Role::Admin);
    let chan = s
        .channels()
        .into_iter()
        .find(|c| c.id == private_id)
        .unwrap();
    assert_eq!(chan.kind, ChannelKind::Private);
    assert!(chan.members.contains(&bob));
    assert!(chan.members.contains(&owner));
    // Provenance came back with it, so the Drive panel and quotas still work.
    let files = s.files_for(owner);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].uploader, owner);
    assert_eq!(files[0].location_name, "#secret");
    assert_eq!(s.quota_for(owner).used, 13);
    assert!(s.blob_bytes_for(bob, &hash).is_some());
    assert!(!s.audit_log(owner, 50).unwrap().is_empty());
}

/// Deleting a channel has to release the files shared in it, or their bytes
/// are stranded with nothing holding a count and nothing able to reach them.
#[test]
fn deleting_a_channel_reclaims_the_files_shared_in_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
    let owner = join_owner(&mut state, "owner");
    let extra = state
        .create_channel_of_kind(owner, "temp", ChannelKind::Public, vec![])
        .unwrap()
        .id;
    let env = state
        .post_file(
            owner,
            extra,
            "t.txt".into(),
            "text/plain".into(),
            b"temp".to_vec(),
        )
        .unwrap();
    let hash = file_payload(&env).hash.clone();

    state.delete_channel(owner, extra).unwrap();
    assert!(!state.blobs.contains_key(&hash));
    assert!(!dir.path().join(BLOB_DIR).join(&hash).exists());
    assert!(state.files_for(owner).is_empty());
    assert_eq!(state.quota_for(owner).used, 0);

    // The last channel cannot go: `default_channel` needs one to exist.
    let last = state.default_channel();
    assert!(state.delete_channel(owner, last).is_err());
}

/// A file too large to fit in one frame goes up in chunks and comes back
/// down the same way, and the result is indistinguishable from an inline
/// upload once it lands.
#[test]
fn a_chunked_upload_round_trips_and_posts_a_file_message() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
    let owner = join_owner(&mut state, "owner");
    let channel = state.default_channel();

    // Deliberately not a multiple of the chunk size, so the last chunk is
    // short — the case an off-by-one gets wrong.
    let payload: Vec<u8> = (0..(PARTY_CHUNK_BYTES * 2 + 77))
        .map(|i| (i % 251) as u8)
        .collect();

    let upload = state
        .start_upload(
            owner,
            "big.bin".into(),
            "application/octet-stream".into(),
            payload.len() as u64,
            UploadTarget::Channel(channel),
        )
        .unwrap();
    for chunk in payload.chunks(PARTY_CHUNK_BYTES) {
        state.upload_chunk(owner, upload, chunk).unwrap();
    }
    let (env, target) = state.finish_upload(owner, upload).unwrap();
    assert_eq!(target, UploadTarget::Channel(channel));
    let meta = file_payload(&env);
    assert_eq!(meta.size, payload.len() as u64);
    assert_eq!(meta.hash, blob_hash(&payload));

    // It is a normal file message in the channel, and a normal blob.
    assert_eq!(state.history_since(owner, channel, 0).len(), 1);
    assert_eq!(
        state.blob_bytes_for(owner, &meta.hash),
        Some(payload.clone())
    );

    // And it comes back down in order, in chunks, with the total attached.
    let mut got = Vec::new();
    loop {
        let (data, total) = state
            .blob_chunk_for(owner, &meta.hash, got.len() as u64)
            .unwrap();
        assert_eq!(total, payload.len() as u64);
        if data.is_empty() {
            break;
        }
        got.extend_from_slice(&data);
        if got.len() as u64 >= total {
            break;
        }
    }
    assert_eq!(got, payload);
}

/// The declared size is the contract: a client that sends more than it said
/// it would is cut off rather than allowed to grow the server's memory.
#[test]
fn an_upload_that_exceeds_its_declared_size_is_dropped() {
    let mut state = PartyState::new("Srv", None);
    let owner = join_owner(&mut state, "owner");
    let channel = state.default_channel();
    let upload = state
        .start_upload(
            owner,
            "x.bin".into(),
            "application/octet-stream".into(),
            4,
            UploadTarget::Channel(channel),
        )
        .unwrap();
    state.upload_chunk(owner, upload, b"abcd").unwrap();
    assert!(state.upload_chunk(owner, upload, b"more").is_err());
    // The upload is gone, so finishing it fails too.
    assert!(state.finish_upload(owner, upload).is_err());
}

/// Finishing early must not store a truncated file under a hash that claims
/// to be the whole thing.
#[test]
fn an_incomplete_upload_cannot_be_finished() {
    let mut state = PartyState::new("Srv", None);
    let owner = join_owner(&mut state, "owner");
    let channel = state.default_channel();
    let upload = state
        .start_upload(
            owner,
            "x.bin".into(),
            "application/octet-stream".into(),
            10,
            UploadTarget::Channel(channel),
        )
        .unwrap();
    state.upload_chunk(owner, upload, b"abc").unwrap();
    let err = state.finish_upload(owner, upload).unwrap_err();
    assert!(err.contains("incomplete"), "got: {err}");
}

/// Everything checkable is checked before a byte moves — otherwise the
/// server spools a hundred megabytes and only then says no.
#[test]
fn uploads_are_refused_up_front_on_size_quota_and_permission() {
    let mut state = PartyState::new("Srv", None);
    let owner = join_owner(&mut state, "owner");
    let bob = state.join("bob", None, None).unwrap();
    let channel = state.default_channel();
    let octet = || "application/octet-stream".to_string();

    // Past the server's hard ceiling.
    assert!(state
        .start_upload(
            bob,
            "huge".into(),
            octet(),
            MAX_PARTY_FILE_BYTES + 1,
            UploadTarget::Channel(channel),
        )
        .is_err());
    // Empty.
    assert!(state
        .start_upload(bob, "e".into(), octet(), 0, UploadTarget::Channel(channel),)
        .is_err());
    // Past the member's allowance.
    state.set_max_member_blob_bytes(1024);
    let err = state
        .start_upload(
            bob,
            "big".into(),
            octet(),
            4096,
            UploadTarget::Channel(channel),
        )
        .unwrap_err();
    assert!(err.contains("allowance"), "got: {err}");
    state.set_max_member_blob_bytes(MAX_MEMBER_BLOB_BYTES);
    // Too many at once. The cap counts the spools the state is actually
    // holding, so it has to be reached by opening them, not by asserting it.
    let opened: Vec<Uuid> = (0..MAX_CONCURRENT_UPLOADS)
        .map(|i| {
            state
                .start_upload(
                    bob,
                    format!("n{i}"),
                    octet(),
                    16,
                    UploadTarget::Channel(channel),
                )
                .expect("under the cap")
        })
        .collect();
    let err = state
        .start_upload(bob, "n".into(), octet(), 16, UploadTarget::Channel(channel))
        .unwrap_err();
    assert!(err.contains("in progress"), "got: {err}");
    for upload in opened {
        state.cancel_upload(bob, upload);
    }
    // A channel they may not post to.
    let locked = state
        .create_channel_of_kind(owner, "locked", ChannelKind::Locked, vec![])
        .unwrap()
        .id;
    assert!(state
        .start_upload(bob, "l".into(), octet(), 16, UploadTarget::Channel(locked),)
        .is_err());
    // A guest cannot upload at all, DM included.
    state.set_role(owner, bob, Role::Guest).unwrap();
    assert!(state
        .start_upload(bob, "g".into(), octet(), 16, UploadTarget::Dm(owner))
        .is_err());
}

/// One member's upload id must be useless to anyone else, and a disconnect
/// must not leave the spool pinned for the life of the process.
#[test]
fn uploads_are_owned_by_their_uploader_and_cleaned_up() {
    let mut state = PartyState::new("Srv", None);
    let owner = join_owner(&mut state, "owner");
    let bob = state.join("bob", None, None).unwrap();
    let channel = state.default_channel();
    let upload = state
        .start_upload(
            bob,
            "x".into(),
            "application/octet-stream".into(),
            8,
            UploadTarget::Channel(channel),
        )
        .unwrap();

    // Someone else's id gets the same answer as a made-up one.
    assert!(state.upload_chunk(owner, upload, b"abcd").is_err());
    assert!(state.finish_upload(owner, upload).is_err());
    state.cancel_upload(owner, upload);
    assert!(
        state.upload_chunk(bob, upload, b"abcd").is_ok(),
        "still bob's"
    );

    // Bob's connection goes away.
    state.cancel_uploads_for(bob);
    assert!(state.upload_chunk(bob, upload, b"abcd").is_err());
}

/// An upload finished short must not leave its spool behind.
///
/// This was an unbounded memory leak a Guest could drive: declare the
/// maximum, send all but a byte of it, call `FinishUpload`. The server
/// answered "upload is incomplete" and kept every byte, while the
/// connection — which cleared its own list *before* the call — reported
/// nothing in flight, so `MAX_CONCURRENT_UPLOADS` read zero and the
/// disconnect sweep, gated on that same list, never ran. Each round leaked
/// up to `MAX_PARTY_FILE_BYTES` and nothing short of a restart reclaimed it.
#[test]
fn an_upload_finished_short_does_not_leak_its_spool() {
    let mut state = PartyState::new("Srv", None);
    let owner = join_owner(&mut state, "owner");
    let bob = state.join("bob", None, None).unwrap();
    let channel = state.default_channel();
    let _ = owner;

    for _ in 0..(MAX_CONCURRENT_UPLOADS * 3) {
        let upload = state
            .start_upload(
                bob,
                "x".into(),
                "application/octet-stream".into(),
                64,
                UploadTarget::Channel(channel),
            )
            .expect("the cap must not be reached by abandoned uploads");
        state.upload_chunk(bob, upload, &[0u8; 32]).unwrap();

        let err = state.finish_upload(bob, upload).unwrap_err();
        assert!(err.contains("incomplete"), "got: {err}");

        assert_eq!(
            state.staged_upload_bytes(),
            0,
            "the spool survived a short finish"
        );
        assert_eq!(
            state.uploads_in_flight(bob),
            0,
            "the abandoned upload still counts against the cap"
        );
    }
}

/// The concurrency cap is enforced against the spools the server holds, not
/// against a number a caller passes in. Nothing a connection forgets can
/// buy a member more of them.
#[test]
fn the_upload_cap_counts_what_the_server_is_holding() {
    let mut state = PartyState::new("Srv", None);
    let bob = state.join("bob", None, None).unwrap();
    let channel = state.default_channel();

    let mut opened = Vec::new();
    for i in 0..MAX_CONCURRENT_UPLOADS {
        opened.push(
            state
                .start_upload(
                    bob,
                    format!("f{i}"),
                    "application/octet-stream".into(),
                    16,
                    UploadTarget::Channel(channel),
                )
                .expect("under the cap"),
        );
    }
    assert_eq!(state.uploads_in_flight(bob), MAX_CONCURRENT_UPLOADS);
    assert!(state
        .start_upload(
            bob,
            "one-too-many".into(),
            "application/octet-stream".into(),
            16,
            UploadTarget::Channel(channel),
        )
        .is_err());

    // Cancelling one frees exactly one slot.
    state.cancel_upload(bob, opened[0]);
    assert_eq!(state.uploads_in_flight(bob), MAX_CONCURRENT_UPLOADS - 1);
    assert!(state
        .start_upload(
            bob,
            "now-there-is-room".into(),
            "application/octet-stream".into(),
            16,
            UploadTarget::Channel(channel),
        )
        .is_ok());
}

/// A chunked download reads one chunk, not the whole file.
///
/// Serving 64 KiB of a 100 MiB blob used to read all 100 MiB off disk —
/// about 160 GiB over a full download's 1,600 chunks, every byte of it with
/// the state mutex held, so one member fetching a large file repeatedly
/// stalled everyone else's messages.
///
/// The observable contract is unchanged, which is what this pins: same
/// bytes, same reported total, same behaviour past the end.
#[test]
fn a_chunked_read_returns_the_same_bytes_from_disk_as_from_memory() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
    let owner = join_owner(&mut state, "owner");
    let channel = state.default_channel();

    // Two and a bit chunks, so offsets land mid-file and at the tail.
    let payload: Vec<u8> = (0..(PARTY_CHUNK_BYTES * 2 + 1234))
        .map(|i| (i % 251) as u8)
        .collect();
    let env = state
        .post_file(
            owner,
            channel,
            "big.bin".into(),
            "application/octet-stream".into(),
            payload.clone(),
        )
        .unwrap();
    let hash = file_payload(&env).hash.clone();

    let mut reassembled = Vec::new();
    let mut offset = 0u64;
    loop {
        let (chunk, total) = state
            .blob_chunk_for(owner, &hash, offset)
            .expect("the owner may read their own file");
        assert_eq!(total, payload.len() as u64, "reported size drifted");
        if chunk.is_empty() {
            break;
        }
        assert!(
            chunk.len() <= PARTY_CHUNK_BYTES,
            "a chunk must not exceed the chunk size"
        );
        reassembled.extend_from_slice(&chunk);
        offset += chunk.len() as u64;
    }
    assert_eq!(reassembled, payload, "the file did not round-trip");

    // Past the end is an empty chunk and the real total, not an error.
    let (tail, total) = state
        .blob_chunk_for(owner, &hash, payload.len() as u64 + 99)
        .unwrap();
    assert!(tail.is_empty());
    assert_eq!(total, payload.len() as u64);

    // Access is still decided before any byte is read.
    let mallory = state.join("mallory", None, None).unwrap();
    state.set_role(owner, mallory, Role::Guest).unwrap();
    let private = state
        .create_channel_of_kind(owner, "private", ChannelKind::Private, vec![])
        .unwrap()
        .id;
    let env = state
        .post_file(
            owner,
            private,
            "secret.bin".into(),
            "application/octet-stream".into(),
            b"secret".to_vec(),
        )
        .unwrap();
    let secret_hash = file_payload(&env).hash.clone();
    assert!(
        state.blob_chunk_for(mallory, &secret_hash, 0).is_none(),
        "a chunked read must not bypass the access check"
    );
}

/// Peak disk is bounded by the storage ceiling, not by what happens to
/// arrive at once.
///
/// Phase 2 writes the whole payload before phase 3 re-checks the ceiling, so
/// without a reservation the limit bounds only what is *stored*: members
/// each finishing several uploads would stage far past it and only then be
/// refused, having already written every byte. Reserving in phase 1 is what
/// makes the ceiling mean something about the disk.
#[test]
fn staged_uploads_are_counted_against_the_storage_ceiling() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
    let owner = join_owner(&mut state, "owner");
    let bob = state.join("bob", None, None).unwrap();
    let channel = state.default_channel();
    // Room for two of these and no more.
    state.set_max_blob_bytes(200);
    state.set_max_member_blob_bytes(1_000_000);
    let _ = owner;

    let mut staged = Vec::new();
    for i in 0..2 {
        let upload = state
            .start_upload(
                bob,
                format!("f{i}"),
                "application/octet-stream".into(),
                100,
                UploadTarget::Channel(channel),
            )
            .unwrap();
        state.upload_chunk(bob, upload, &[b'x'; 100]).unwrap();
        staged.push(state.begin_upload(bob, upload).expect("within the ceiling"));
    }

    // Nothing is stored yet — every byte of that is a reservation — and the
    // third upload is refused *before* it is written, not after.
    assert_eq!(state.blobs.len(), 0, "nothing has been committed yet");
    let upload = state
        .start_upload(
            bob,
            "third".into(),
            "application/octet-stream".into(),
            100,
            UploadTarget::Channel(channel),
        )
        .unwrap();
    state.upload_chunk(bob, upload, &[b'z'; 100]).unwrap();
    let err = state
        .begin_upload(bob, upload)
        .expect_err("the ceiling is already spoken for");
    assert!(err.contains("storage is full"), "got: {err}");

    // Giving one back frees exactly its reservation. (A refused finish
    // consumes its spool — see `begin_upload` — so this needs a fresh one.)
    let first = staged.remove(0);
    state.discard_upload(first);
    let upload = state
        .start_upload(
            bob,
            "fourth".into(),
            "application/octet-stream".into(),
            100,
            UploadTarget::Channel(channel),
        )
        .unwrap();
    state.upload_chunk(bob, upload, &[b'w'; 100]).unwrap();
    assert!(
        state.begin_upload(bob, upload).is_ok(),
        "discarding a staged upload must release its claim"
    );
}

/// A deduplicated re-upload still succeeds at a full ceiling — on the
/// **chunked** path too, not only the inline one.
///
/// Content the store already holds costs no new storage, so it must not be
/// refused for lack of room. That is easy inline (the bytes are in hand and
/// can be hashed) and was briefly lost for chunked uploads, where the hash
/// used to be unknown until after the ceiling check and the reservation had
/// to assume the worst. Accumulating the hash as chunks arrive removes the
/// guesswork rather than documenting it.
#[test]
fn a_deduplicated_chunked_reupload_succeeds_at_a_full_ceiling() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
    let owner = join_owner(&mut state, "owner");
    let bob = state.join("bob", None, None).unwrap();
    let channel = state.default_channel();
    let _ = owner;
    let payload = [7u8; 200];

    let upload_it = |state: &mut PartyState, name: &str| -> Result<(), String> {
        let upload = state.start_upload(
            bob,
            name.to_string(),
            "application/octet-stream".into(),
            payload.len() as u64,
            UploadTarget::Channel(channel),
        )?;
        for chunk in payload.chunks(64) {
            state.upload_chunk(bob, upload, chunk)?;
        }
        state.finish_upload(bob, upload).map(|_| ())
    };

    upload_it(&mut state, "first.bin").expect("room for the first");

    // The store is now exactly full.
    state.set_max_blob_bytes(payload.len() as u64);

    // Distinct content is refused, as it must be.
    let upload = state
        .start_upload(
            bob,
            "other.bin".into(),
            "application/octet-stream".into(),
            8,
            UploadTarget::Channel(channel),
        )
        .unwrap();
    state.upload_chunk(bob, upload, b"distinct").unwrap();
    let err = state.finish_upload(bob, upload).unwrap_err();
    assert!(err.contains("storage is full"), "got: {err}");

    // The same content again adds no bytes, so it is still allowed.
    upload_it(&mut state, "same-again.bin")
        .expect("a chunked re-upload of stored content adds no bytes");
    assert_eq!(state.blobs.len(), 1, "dedupe stored a second copy");
}

/// A member whose connection dies between phase 1 and phase 3 must not
/// leave a claim on storage that nothing releases.
#[test]
fn a_disconnect_releases_a_staged_upload_s_claim() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
    let bob = state.join("bob", None, None).unwrap();
    let channel = state.default_channel();
    state.set_max_blob_bytes(150);

    let upload = state
        .start_upload(
            bob,
            "f".into(),
            "application/octet-stream".into(),
            100,
            UploadTarget::Channel(channel),
        )
        .unwrap();
    state.upload_chunk(bob, upload, &[b'x'; 100]).unwrap();
    let staged = state.begin_upload(bob, upload).unwrap();

    // The connection goes away with the upload mid-flight.
    state.cancel_uploads_for(bob);
    drop(staged);

    let upload = state
        .start_upload(
            bob,
            "again".into(),
            "application/octet-stream".into(),
            100,
            UploadTarget::Channel(channel),
        )
        .unwrap();
    state.upload_chunk(bob, upload, &[b'y'; 100]).unwrap();
    assert!(
        state.begin_upload(bob, upload).is_ok(),
        "the abandoned upload's claim was never released"
    );
}

/// One member's two devices are two connections, and closing one must not
/// cancel the other's uploads.
///
/// The disconnect sweep used to run per *member*, so a laptop closing took
/// the phone's transfer with it. Sweeping the ids the departing connection
/// actually had open is what makes multi-device work rather than merely
/// appear to.
#[test]
fn closing_one_device_does_not_cancel_another_s_uploads() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
    let bob = state.join("bob", None, None).unwrap();
    let channel = state.default_channel();

    let start = |state: &mut PartyState, name: &str| {
        let upload = state
            .start_upload(
                bob,
                name.to_string(),
                "application/octet-stream".into(),
                8,
                UploadTarget::Channel(channel),
            )
            .unwrap();
        state.upload_chunk(bob, upload, b"1234").unwrap();
        upload
    };

    let laptop = start(&mut state, "from-laptop.bin");
    let phone = start(&mut state, "from-phone.bin");
    assert_eq!(state.uploads_in_flight(bob), 2);

    // The laptop's connection goes away, carrying only its own id.
    state.cancel_connection_uploads(bob, &[laptop]);

    assert_eq!(
        state.uploads_in_flight(bob),
        1,
        "the phone's upload was cancelled"
    );
    assert!(
        state.upload_chunk(bob, laptop, b"5678").is_err(),
        "the laptop's upload should be gone"
    );
    assert!(
        state.upload_chunk(bob, phone, b"5678").is_ok(),
        "the phone's upload must still be usable"
    );
    // And it can still be completed: 4 + 4 is the 8 bytes it declared.
    assert!(state.finish_upload(bob, phone).is_ok());
}

/// A staged reservation belongs to the connection that made it, so a
/// disconnect releases that one and leaves another device's alone.
#[test]
fn a_disconnect_releases_only_its_own_reservation() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
    let bob = state.join("bob", None, None).unwrap();
    let channel = state.default_channel();
    state.set_max_blob_bytes(250);

    let stage_one = |state: &mut PartyState, fill: u8| {
        let upload = state
            .start_upload(
                bob,
                format!("f{fill}.bin"),
                "application/octet-stream".into(),
                100,
                UploadTarget::Channel(channel),
            )
            .unwrap();
        state.upload_chunk(bob, upload, &[fill; 100]).unwrap();
        (upload, state.begin_upload(bob, upload).unwrap())
    };

    let (laptop_id, laptop) = stage_one(&mut state, b'a');
    let (_phone_id, mut phone) = stage_one(&mut state, b'b');
    phone.write_staging().unwrap();

    // 200 of 250 reserved: a third 100-byte upload has no room.
    let upload = state
        .start_upload(
            bob,
            "third.bin".into(),
            "application/octet-stream".into(),
            100,
            UploadTarget::Channel(channel),
        )
        .unwrap();
    state.upload_chunk(bob, upload, &[b'c'; 100]).unwrap();
    assert!(state.begin_upload(bob, upload).is_err());

    // The laptop disconnects mid-stage. Its reservation goes; the phone's
    // does not.
    drop(laptop);
    state.cancel_connection_uploads(bob, &[laptop_id]);

    let upload = state
        .start_upload(
            bob,
            "fourth.bin".into(),
            "application/octet-stream".into(),
            100,
            UploadTarget::Channel(channel),
        )
        .unwrap();
    state.upload_chunk(bob, upload, &[b'd'; 100]).unwrap();
    assert!(
        state.begin_upload(bob, upload).is_ok(),
        "the laptop's reservation was not released"
    );

    // The phone's upload is untouched and still commits.
    assert!(state.commit_upload(phone).is_ok());
}

/// Two members uploading identical content must not be able to delete each
/// other's bytes.
///
/// Staging under the *content hash* would let one member's failed commit
/// unlink the file the other's commit is about to record, leaving a file
/// message that answers "unknown file" for good. Staging under a per-upload
/// id makes that impossible rather than unlikely.
#[test]
fn identical_concurrent_uploads_do_not_delete_each_other() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
    // The first joiner is the Owner and cannot be demoted, so the two
    // uploaders are ordinary members joining after them.
    let owner = join_owner(&mut state, "owner");
    let alice = state.join("alice", None, None).unwrap();
    let bob = state.join("bob", None, None).unwrap();
    let channel = state.default_channel();
    let payload = b"a forwarded image".to_vec();

    let stage_for = |state: &mut PartyState, who: Uuid| {
        let upload = state
            .start_upload(
                who,
                "same.bin".into(),
                "application/octet-stream".into(),
                payload.len() as u64,
                UploadTarget::Channel(channel),
            )
            .unwrap();
        state.upload_chunk(who, upload, &payload).unwrap();
        let mut staged = state.begin_upload(who, upload).unwrap();
        staged.write_staging().unwrap();
        staged
    };

    // Both stage the same bytes, both written to disk, neither committed.
    let alice_staged = stage_for(&mut state, alice);
    let bob_staged = stage_for(&mut state, bob);

    // Alice's commit fails on the far side of the write — she was demoted
    // while her bytes were going down.
    state.set_role(owner, alice, Role::Guest).unwrap();
    assert!(
        state.commit_upload(alice_staged).is_err(),
        "a guest may not post"
    );

    // Bob's commit must still produce a file that downloads.
    let (env, _) = state.commit_upload(bob_staged).expect("bob may still post");
    let hash = file_payload(&env).hash.clone();
    assert_eq!(
        state.blob_bytes_for(bob, &hash),
        Some(payload),
        "the surviving upload's bytes were deleted by the failed one"
    );
}

/// The staging directory does not accumulate files.
#[test]
fn a_discarded_upload_leaves_no_file_behind() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = dir.path().join("blobs");
    let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
    let bob = state.join("bob", None, None).unwrap();
    let channel = state.default_channel();

    for _ in 0..3 {
        let upload = state
            .start_upload(
                bob,
                "f".into(),
                "application/octet-stream".into(),
                8,
                UploadTarget::Channel(channel),
            )
            .unwrap();
        state.upload_chunk(bob, upload, b"12345678").unwrap();
        let mut staged = state.begin_upload(bob, upload).unwrap();
        staged.write_staging().unwrap();
        state.discard_upload(staged);
    }

    let leftovers: Vec<_> = std::fs::read_dir(&blobs)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("staging_"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "staging files left behind: {leftovers:?}"
    );
}

/// A crash leaves staging files nothing else will ever remove, so the next
/// start sweeps them.
#[test]
fn startup_sweeps_staging_files_a_crash_left_behind() {
    let dir = tempfile::tempdir().unwrap();
    {
        let _state = PartyState::load("Srv", None, dir.path()).unwrap();
    }
    let blobs = dir.path().join("blobs");
    std::fs::write(blobs.join("staging_orphan-one"), b"partial").unwrap();
    std::fs::write(blobs.join("staging_orphan-two"), b"partial").unwrap();
    // A real blob, which must survive: its name is a content hash and no
    // hash starts with the staging prefix.
    let keep = blobs.join("abc123");
    std::fs::write(&keep, b"real").unwrap();

    let _state = PartyState::load("Srv", None, dir.path()).unwrap();

    assert!(keep.exists(), "the sweep removed a real blob");
    let leftovers: Vec<_> = std::fs::read_dir(&blobs)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("staging_"))
        .collect();
    assert!(leftovers.is_empty(), "sweep missed: {leftovers:?}");
}

/// Permission is re-checked on the far side of the write, not only before it.
///
/// `begin_upload` checks; for a 100 MiB upload that check is minutes old by
/// the time the bytes land, and the lock was released for all of it. Anyone
/// demoted, or any channel deleted, in that window used to get its message
/// appended anyway. `check_may_post_file` is one function called from both
/// ends precisely so the two cannot drift — which is how the DM recipient
/// came to be checked only on the way in.
#[test]
fn permission_lost_during_the_write_refuses_the_commit() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
    let owner = join_owner(&mut state, "owner");
    let bob = state.join("bob", None, None).unwrap();
    let carol = state.join("carol", None, None).unwrap();
    let channel = state.default_channel();

    // (a) The uploader is demoted to a read-only role mid-DM-upload.
    let upload = state
        .start_upload(
            bob,
            "for-carol.bin".into(),
            "application/octet-stream".into(),
            4,
            UploadTarget::Dm(carol),
        )
        .unwrap();
    state.upload_chunk(bob, upload, b"abcd").unwrap();
    let mut staged = state.begin_upload(bob, upload).unwrap();
    staged.write_staging().unwrap();
    state.set_role(owner, bob, Role::Guest).unwrap();

    let err = state
        .commit_upload(staged)
        .expect_err("a guest may not send a DM");
    assert!(err.contains("read-only"), "got: {err}");
    assert!(
        state
            .dm_history(messenger_core::party::dm_thread_id(bob, carol), 0)
            .is_empty(),
        "the message was appended despite the refusal"
    );

    // (b) The channel is deleted mid-upload.
    state.set_role(owner, bob, Role::Member).unwrap();
    let doomed = state
        .create_channel_of_kind(owner, "doomed", ChannelKind::Public, vec![])
        .unwrap()
        .id;
    let upload = state
        .start_upload(
            bob,
            "f.bin".into(),
            "application/octet-stream".into(),
            4,
            UploadTarget::Channel(doomed),
        )
        .unwrap();
    state.upload_chunk(bob, upload, b"efgh").unwrap();
    let mut staged = state.begin_upload(bob, upload).unwrap();
    staged.write_staging().unwrap();
    state.delete_channel(owner, doomed).unwrap();

    assert!(
        state.commit_upload(staged).is_err(),
        "a deleted channel still accepted a file"
    );

    // And nothing of either attempt is left on disk.
    let leftovers: Vec<_> = std::fs::read_dir(dir.path().join("blobs"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("staging_"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "staging files left behind: {leftovers:?}"
    );

    // A permitted upload still works, so this is not just refusing everything.
    let upload = state
        .start_upload(
            bob,
            "ok.bin".into(),
            "application/octet-stream".into(),
            4,
            UploadTarget::Channel(channel),
        )
        .unwrap();
    state.upload_chunk(bob, upload, b"ijkl").unwrap();
    let mut staged = state.begin_upload(bob, upload).unwrap();
    staged.write_staging().unwrap();
    assert!(state.commit_upload(staged).is_ok());
}

/// Someone else's failed `FinishUpload` must not discard your spool. The
/// "no such upload" answer is deliberately the same for a stranger's id and
/// a made-up one, and it must stay just as inert.
#[test]
fn a_strangers_finish_does_not_drop_your_upload() {
    let mut state = PartyState::new("Srv", None);
    let owner = join_owner(&mut state, "owner");
    let bob = state.join("bob", None, None).unwrap();
    let channel = state.default_channel();

    let upload = state
        .start_upload(
            bob,
            "x".into(),
            "application/octet-stream".into(),
            8,
            UploadTarget::Channel(channel),
        )
        .unwrap();
    state.upload_chunk(bob, upload, b"abcd").unwrap();

    assert!(state.finish_upload(owner, upload).is_err());
    assert_eq!(state.uploads_in_flight(bob), 1, "bob's upload was dropped");
    assert!(state.upload_chunk(bob, upload, b"efgh").is_ok());
    assert!(state.finish_upload(bob, upload).is_ok());
}

/// The rights are separate on purpose: seeing that a file exists is not the
/// same as being able to fetch it.
#[test]
fn view_and_download_are_separate_rights() {
    let mut state = PartyState::new("Srv", None);
    let owner = join_owner(&mut state, "owner");
    let bob = state.join("bob", None, None).unwrap();
    let chan = state.default_channel();
    let env = state
        .post_file(
            owner,
            chan,
            "d.txt".into(),
            "text/plain".into(),
            b"bytes".to_vec(),
        )
        .unwrap();
    let hash = file_payload(&env).hash.clone();

    // The default: bob can see it and fetch it.
    assert_eq!(state.files_for(bob).len(), 1);
    assert!(state.blob_bytes_for(bob, &hash).is_some());

    // Downgrade to view-only, for everyone.
    state
        .set_file_permissions(
            owner,
            &hash,
            chan,
            None,
            FilePermissions {
                view: true,
                download: false,
                delete: false,
                share: false,
            },
        )
        .unwrap();
    assert_eq!(state.files_for(bob).len(), 1, "he can still see it");
    assert!(
        state.blob_bytes_for(bob, &hash).is_none(),
        "but not fetch it"
    );

    // Take view away too and it disappears from his listing entirely.
    state
        .set_file_permissions(owner, &hash, chan, None, FilePermissions::none())
        .unwrap();
    assert!(state.files_for(bob).is_empty());
    // The uploader is unaffected by the grant they handed out.
    assert!(state.blob_bytes_for(owner, &hash).is_some());
}

/// A per-member grant overrides the default for that member and nobody else.
#[test]
fn a_member_grant_overrides_the_default() {
    let mut state = PartyState::new("Srv", None);
    let owner = join_owner(&mut state, "owner");
    let bob = state.join("bob", None, None).unwrap();
    let carol = state.join("carol", None, None).unwrap();
    let chan = state.default_channel();
    let env = state
        .post_file(
            owner,
            chan,
            "d.txt".into(),
            "text/plain".into(),
            b"bytes".to_vec(),
        )
        .unwrap();
    let hash = file_payload(&env).hash.clone();

    state
        .set_file_permissions(owner, &hash, chan, None, FilePermissions::none())
        .unwrap();
    state
        .set_file_permissions(
            owner,
            &hash,
            chan,
            Some(bob),
            FilePermissions {
                view: true,
                download: true,
                delete: false,
                share: true,
            },
        )
        .unwrap();

    assert!(
        state.blob_bytes_for(bob, &hash).is_some(),
        "bob was granted"
    );
    assert!(
        state.blob_bytes_for(carol, &hash).is_none(),
        "carol still has the default, which is nothing"
    );
}

/// You can only hand out rights you hold — otherwise a grant is a way to
/// mint authority rather than pass it on.
#[test]
fn a_grant_cannot_exceed_what_the_granter_holds() {
    let mut state = PartyState::new("Srv", None);
    let owner = join_owner(&mut state, "owner");
    let bob = state.join("bob", None, None).unwrap();
    let carol = state.join("carol", None, None).unwrap();
    let chan = state.default_channel();

    // Bob shares a file, so he is its uploader and holds everything.
    let env = state
        .post_file(
            bob,
            chan,
            "b.txt".into(),
            "text/plain".into(),
            b"bobs".to_vec(),
        )
        .unwrap();
    let hash = file_payload(&env).hash.clone();
    assert!(state
        .set_file_permissions(bob, &hash, chan, Some(carol), FilePermissions::all())
        .is_ok());

    // Carol holds everything now, but she is not the uploader and not an
    // admin, so she may not rewrite who else may do what.
    let err = state
        .set_file_permissions(carol, &hash, chan, Some(owner), FilePermissions::all())
        .unwrap_err();
    assert!(err.contains("only the member who shared"), "got: {err}");
}

/// Re-sharing spends a reference on content the server already holds, and
/// the new share starts at the default rather than inheriting the sharer's
/// authority over it.
#[test]
fn sharing_a_file_elsewhere_costs_a_reference_not_a_copy() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
    let owner = join_owner(&mut state, "owner");
    let bob = state.join("bob", None, None).unwrap();
    let first = state.default_channel();
    let second = state
        .create_channel_of_kind(owner, "second", ChannelKind::Public, vec![])
        .unwrap()
        .id;

    let env = state
        .post_file(
            owner,
            first,
            "d.txt".into(),
            "text/plain".into(),
            b"shared".to_vec(),
        )
        .unwrap();
    let hash = file_payload(&env).hash.clone();

    // Bob has only the default grant, which does not include share.
    let err = state
        .share_file(bob, &hash, first, UploadTarget::Channel(second))
        .unwrap_err();
    assert!(err.contains("permission to share"), "got: {err}");

    // Granted, he can — and it is a reference, not a second upload.
    state
        .set_file_permissions(
            owner,
            &hash,
            first,
            Some(bob),
            FilePermissions {
                view: true,
                download: true,
                delete: false,
                share: true,
            },
        )
        .unwrap();
    let shared = state
        .share_file(bob, &hash, first, UploadTarget::Channel(second))
        .unwrap();
    assert_eq!(file_payload(&shared).hash, hash);
    assert_eq!(state.blobs[&hash].refcount, 2, "one blob, two references");
    assert_eq!(
        state.blob_bytes(&hash),
        Some(b"shared".to_vec()),
        "the bytes were never moved again"
    );

    // The new share starts at the default: bob passed on the file, not his
    // own right to spread it further.
    let there = state
        .file_refs
        .iter()
        .find(|r| r.location == second)
        .unwrap();
    assert_eq!(there.default_perms, FilePermissions::default());
    assert_eq!(there.uploader, bob, "he is who put it there");

    // Sharing it into the same place twice is refused rather than
    // silently inflating the reference count.
    assert!(state
        .share_file(bob, &hash, first, UploadTarget::Channel(second))
        .is_err());

    // Deleting one reference leaves the other holding the bytes.
    state.delete_file(bob, &hash, second).unwrap();
    assert_eq!(state.blobs[&hash].refcount, 1);
    assert!(state.blob_bytes(&hash).is_some());
}

/// A guest is read-only everywhere, and that outranks any grant — otherwise
/// this would be the one place a guest could write.
#[test]
fn a_grant_cannot_give_a_guest_write_rights() {
    let mut state = PartyState::new("Srv", None);
    let owner = join_owner(&mut state, "owner");
    let bob = state.join("bob", None, None).unwrap();
    let chan = state.default_channel();
    let env = state
        .post_file(
            owner,
            chan,
            "d.txt".into(),
            "text/plain".into(),
            b"bytes".to_vec(),
        )
        .unwrap();
    let hash = file_payload(&env).hash.clone();

    state
        .set_file_permissions(owner, &hash, chan, Some(bob), FilePermissions::all())
        .unwrap();
    state.set_role(owner, bob, Role::Guest).unwrap();

    // He kept view and download, and lost the two that are writes.
    let listing = state.files_for(bob);
    assert_eq!(listing.len(), 1);
    assert!(listing[0].perms.view && listing[0].perms.download);
    assert!(!listing[0].perms.delete && !listing[0].perms.share);
    assert!(state.delete_file(bob, &hash, chan).is_err());
}

/// Permissions have to survive a restart, or a grant quietly evaporates.
#[test]
fn file_permissions_survive_a_reload() {
    let dir = tempfile::tempdir().unwrap();
    let (owner, bob, chan, hash);
    {
        let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
        owner = join_owner(&mut s, "owner");
        bob = s.join("bob", None, None).unwrap();
        chan = s.default_channel();
        let env = s
            .post_file(
                owner,
                chan,
                "d.txt".into(),
                "text/plain".into(),
                b"bytes".to_vec(),
            )
            .unwrap();
        hash = file_payload(&env).hash.clone();
        s.set_file_permissions(owner, &hash, chan, None, FilePermissions::none())
            .unwrap();
        s.set_file_permissions(
            owner,
            &hash,
            chan,
            Some(bob),
            FilePermissions {
                view: true,
                download: true,
                delete: true,
                share: false,
            },
        )
        .unwrap();
    }

    let s = PartyState::load("Srv", None, dir.path()).unwrap();
    let r = s.file_refs.iter().find(|r| r.hash == hash).unwrap();
    assert_eq!(r.default_perms, FilePermissions::none());
    assert!(r.grants.get(&bob).copied().unwrap().delete);
    assert!(s.blob_bytes_for(bob, &hash).is_some(), "his grant survived");
}

/// Incoherent grants are normalised rather than stored as written:
/// downloading what you cannot see, or sharing what you cannot download, is
/// not a thing to represent.
#[test]
fn grants_are_normalised() {
    let odd = FilePermissions {
        view: false,
        download: true,
        delete: false,
        share: true,
    }
    .normalized();
    assert!(odd.view, "downloading implies seeing");
    assert!(odd.download, "sharing implies downloading");

    assert!(FilePermissions::all().covers(FilePermissions::default()));
    assert!(!FilePermissions::default().covers(FilePermissions::all()));
    assert!(FilePermissions::none().covers(FilePermissions::none()));
}

/// A whole channel in one frame stopped fitting once history grew past
/// `MAX_PACKET_SIZE`, and the send failure dropped the connection. History
/// is paged instead.
#[test]
fn history_is_paged_so_a_reply_always_fits_in_one_frame() {
    let mut state = PartyState::new("Open", None);
    let alice = state.join("alice", None, None).unwrap();
    let chan = state.default_channel();
    for i in 0..(MAX_HISTORY_BATCH + 25) {
        state.post_message(alice, chan, format!("m{i}")).unwrap();
    }

    let first = state.history_since(alice, chan, 0);
    assert_eq!(first.len(), MAX_HISTORY_BATCH, "the batch is capped");
    assert_eq!(first[0].seq, 1);

    // Paging with the last seq seen returns the remainder.
    let last_seq = first.last().unwrap().seq;
    let second = state.history_since(alice, chan, last_seq);
    assert_eq!(second.len(), 25);
    assert_eq!(second[0].seq, last_seq + 1);

    // And the page after the end is empty, which is how a client stops.
    let done = state.history_since(alice, chan, second.last().unwrap().seq);
    assert!(done.is_empty());
}

#[test]
fn dm_history_is_paged_too() {
    let mut state = PartyState::new("Open", None);
    let alice = state.join("alice", None, None).unwrap();
    let bob = state.join("bob", None, None).unwrap();
    for i in 0..(MAX_HISTORY_BATCH + 5) {
        state.post_dm(alice, bob, format!("m{i}")).unwrap();
    }
    let thread = messenger_core::party::dm_thread_id(alice, bob);
    assert_eq!(state.dm_history(thread, 0).len(), MAX_HISTORY_BATCH);
    assert_eq!(state.dm_history(thread, MAX_HISTORY_BATCH as u64).len(), 5);
}

/// Channel creation is open to every member with no admin role, so the cap
/// is the only thing bounding server state.
#[test]
fn channel_creation_is_capped() {
    let mut state = PartyState::new("Open", None);
    // `general` already exists.
    for i in 1..MAX_CHANNELS {
        state
            .create_channel(&format!("chan{i}"))
            .unwrap_or_else(|e| panic!("channel {i} should be allowed: {e}"));
    }
    let err = state
        .create_channel("one-too-many")
        .expect_err("past the cap");
    assert!(err.contains("maximum"), "error should say why: {err}");
    assert_eq!(state.channels.len(), MAX_CHANNELS);
}

/// The server-wide [`MAX_CHANNELS`] cap does not tell a hundred members
/// making one channel each from one member making a hundred. Without a
/// per-member limit, any single joined member can walk the server to its
/// ceiling in a burst — broadcasting a refreshed directory to every
/// connection each time and denying everyone else the feature until an admin
/// cleans up.
#[test]
fn a_member_cannot_burst_create_channels() {
    let mut state = PartyState::new("Open", None);
    let _owner = join_owner(&mut state, "owner");
    let bob = state.join("bob", None, None).unwrap();
    let now = Instant::now();

    for i in 0..MAX_CHANNELS_PER_MEMBER {
        state
            .create_channel_of_kind_at(bob, &format!("bob{i}"), ChannelKind::Public, vec![], now)
            .unwrap_or_else(|e| panic!("channel {i} is within the allowance: {e}"));
    }
    let err = state
        .create_channel_of_kind_at(bob, "one-more", ChannelKind::Public, vec![], now)
        .expect_err("past the per-member allowance");
    assert!(
        err.contains("wait a moment"),
        "the error should say it is temporary: {err}"
    );
    assert_eq!(
        state.channels.len(),
        1 + MAX_CHANNELS_PER_MEMBER,
        "general plus the allowed burst, and nothing more"
    );
}

/// The limit is per member, not per server: one member burning their
/// allowance must not stop everyone else from making a channel.
#[test]
fn the_channel_rate_limit_is_per_member() {
    let mut state = PartyState::new("Open", None);
    let _owner = join_owner(&mut state, "owner");
    let bob = state.join("bob", None, None).unwrap();
    let carol = state.join("carol", None, None).unwrap();
    let now = Instant::now();

    for i in 0..MAX_CHANNELS_PER_MEMBER {
        state
            .create_channel_of_kind_at(bob, &format!("bob{i}"), ChannelKind::Public, vec![], now)
            .unwrap();
    }
    assert!(state
        .create_channel_of_kind_at(bob, "bob-extra", ChannelKind::Public, vec![], now)
        .is_err());
    assert!(
        state
            .create_channel_of_kind_at(carol, "carol1", ChannelKind::Public, vec![], now)
            .is_ok(),
        "an unrelated member must not be caught in someone else's limit"
    );
}

/// It is a rate limit, not a quota — the allowance comes back.
#[test]
fn the_channel_rate_limit_window_slides() {
    let mut state = PartyState::new("Open", None);
    let bob = state.join("bob", None, None).unwrap();
    let now = Instant::now();

    for i in 0..MAX_CHANNELS_PER_MEMBER {
        state
            .create_channel_of_kind_at(bob, &format!("bob{i}"), ChannelKind::Public, vec![], now)
            .unwrap();
    }
    assert!(state
        .create_channel_of_kind_at(bob, "blocked", ChannelKind::Public, vec![], now)
        .is_err());

    let later = now + CHANNEL_CREATE_WINDOW + Duration::from_secs(1);
    assert!(state
        .create_channel_of_kind_at(bob, "later", ChannelKind::Public, vec![], later)
        .is_ok());
}

/// A refused creation must not cost the member part of their allowance —
/// otherwise mistyping a duplicate name five times locks you out of making
/// the channel you were trying to make.
#[test]
fn a_rejected_channel_name_does_not_consume_the_allowance() {
    let mut state = PartyState::new("Open", None);
    let bob = state.join("bob", None, None).unwrap();
    let now = Instant::now();

    for _ in 0..20 {
        assert!(state
            .create_channel_of_kind_at(bob, "general", ChannelKind::Public, vec![], now)
            .is_err());
    }
    for i in 0..MAX_CHANNELS_PER_MEMBER {
        state
            .create_channel_of_kind_at(bob, &format!("bob{i}"), ChannelKind::Public, vec![], now)
            .unwrap_or_else(|e| panic!("attempt {i} should still be allowed: {e}"));
    }
}

/// A non-member must not be able to read history by guessing a channel id.
#[test]
fn history_requires_membership() {
    let mut state = PartyState::new("Open", None);
    let alice = state.join("alice", None, None).unwrap();
    let chan = state.default_channel();
    state.post_message(alice, chan, "hi".to_string()).unwrap();

    let stranger = Uuid::new_v4();
    assert!(state.history_since(stranger, chan, 0).is_empty());
}

/// Every write used to be its own rollback-journal commit with
/// `synchronous=FULL` — several fsyncs per post, all under the state lock.
#[test]
fn the_database_runs_in_wal_mode_with_normal_sync() {
    let dir = tempfile::tempdir().unwrap();
    let s = PartyState::load("Srv", None, dir.path()).unwrap();
    let conn = s.db.as_ref().unwrap();
    let mode: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode.to_lowercase(), "wal");
    // 1 == NORMAL.
    let sync: i64 = conn
        .query_row("PRAGMA synchronous", [], |r| r.get(0))
        .unwrap();
    assert_eq!(sync, 1);
}

/// A mutation that writes several rows commits them together, and a helper
/// that opens its own transaction still works when called inside one.
#[test]
fn multi_row_mutations_commit_as_one_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let (alice, bob);
    {
        let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
        alice = s.join("alice", None, None).unwrap();
        bob = s.join("bob", None, None).unwrap();
        // A DM writes its thread row and its message; here it runs inside an
        // outer transaction, so the inner one must not try to nest.
        s.with_tx(|s| {
            s.post_dm(alice, bob, "inside".to_string()).unwrap();
            assert!(!s.db.as_ref().unwrap().is_autocommit());
        });
        assert!(s.db.as_ref().unwrap().is_autocommit(), "committed");
        s.post_dm(alice, bob, "outside".to_string()).unwrap();
    }
    let s = PartyState::load("Srv", None, dir.path()).unwrap();
    let thread = messenger_core::party::dm_thread_id(alice, bob);
    assert_eq!(s.dm_history(thread, 0).len(), 2);
}

/// SRV-01: a page was capped at 200 envelopes and nothing else, and 200
/// envelopes of 64 KiB text are 12.8 MiB. About 130 maximum-size posts made
/// every page covering them unsendable, every joining client was disconnected
/// on its first history fetch, and the community stayed unjoinable across
/// restarts. A channel full of maximum-size messages must still page in frames
/// that fit, and paging must reach the end.
#[test]
fn a_channel_of_maximum_size_messages_still_pages_in_frames_that_fit() {
    let mut state = PartyState::new("Open", None);
    let alice = state.join("alice", None, None).unwrap();
    let chan = state.default_channel();
    let total = MAX_HISTORY_BATCH + 50;
    for i in 0..total {
        let mut text = format!("{i:04}");
        text.push_str(&"x".repeat(MAX_MESSAGE_TEXT_BYTES - text.len()));
        state.post_message(alice, chan, text).unwrap();
    }

    let mut since = 0u64;
    let mut seen = Vec::new();
    let mut pages = 0;
    loop {
        let page = state.history_since(alice, chan, since);
        if page.is_empty() {
            break;
        }
        pages += 1;
        let frame = PartyResponse::History(page.clone()).to_bytes();
        assert!(
            frame.len() <= MAX_PARTY_PAYLOAD_BYTES,
            "page {pages} is {} bytes, over the frame limit",
            frame.len()
        );
        let more = messenger_core::party::history_page_may_continue(&page);
        since = page.last().unwrap().seq;
        seen.extend(page.into_iter().map(|e| e.seq));
        if !more {
            break;
        }
        assert!(pages < 100, "paging must terminate");
    }
    assert!(
        pages > 2,
        "the byte budget, not the count, split this channel"
    );
    assert_eq!(seen, (1..=total as u64).collect::<Vec<_>>());
}

/// The DM thread twin of the test above.
#[test]
fn dm_history_of_maximum_size_messages_fits_in_a_frame() {
    let mut state = PartyState::new("Open", None);
    let alice = state.join("alice", None, None).unwrap();
    let bob = state.join("bob", None, None).unwrap();
    for _ in 0..MAX_HISTORY_BATCH {
        state
            .post_dm(alice, bob, "y".repeat(MAX_MESSAGE_TEXT_BYTES))
            .unwrap();
    }
    let thread = messenger_core::party::dm_thread_id(alice, bob);
    let page = state.dm_history(thread, 0);
    assert!(page.len() < MAX_HISTORY_BATCH);
    assert!(PartyResponse::History(page).to_bytes().len() <= MAX_PARTY_PAYLOAD_BYTES);
}

/// One upload whose MIME type sat just under 8 MiB produced a broadcast
/// *larger* than the request that created it, disconnecting every online
/// member, and poisoned every history page that held it. The name and type are
/// bounded on arrival, on both upload paths.
#[test]
fn file_names_and_mime_types_are_bounded_on_upload() {
    let mut state = PartyState::new("Open", None);
    let alice = state.join("alice", None, None).unwrap();
    let chan = state.default_channel();
    let huge_mime = format!("text/{}", "a".repeat(1024 * 1024));
    let huge_name = "n".repeat(1024 * 1024);

    let env = state
        .post_file(
            alice,
            chan,
            huge_name.clone(),
            huge_mime.clone(),
            b"x".to_vec(),
        )
        .unwrap();
    let meta = file_payload(&env);
    assert_eq!(meta.mime, FALLBACK_MIME);
    assert!(meta.name.len() <= messenger_core::util::MAX_FILENAME_BYTES);
    assert!(encoded_len(&env) < 1024, "the message stays small");

    // The chunked path takes the same route.
    let upload = state
        .start_upload(alice, huge_name, huge_mime, 1, UploadTarget::Channel(chan))
        .unwrap();
    state.upload_chunk(alice, upload, b"y").unwrap();
    let (env, _) = state.finish_upload(alice, upload).unwrap();
    let meta = file_payload(&env);
    assert_eq!(meta.mime, FALLBACK_MIME);
    assert!(meta.name.len() <= messenger_core::util::MAX_FILENAME_BYTES);

    // And so is the listing built from them.
    for f in state.files_for(alice) {
        assert!(f.mime.len() <= MAX_MIME_BYTES);
    }
}

#[test]
fn mime_types_are_normalised_to_type_and_subtype() {
    assert_eq!(normalize_mime("image/png"), "image/png");
    assert_eq!(normalize_mime(" Text/Plain; charset=utf-8 "), "text/plain");
    assert_eq!(
        normalize_mime("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
    );
    for bad in [
        "",
        "png",
        "/png",
        "image/",
        "image/p ng",
        "a/b/c",
        "image/\u{202e}gnp",
    ] {
        assert_eq!(normalize_mime(bad), FALLBACK_MIME, "{bad:?}");
    }
    assert_eq!(
        normalize_mime(&format!("a/{}", "b".repeat(200))),
        FALLBACK_MIME
    );
}

/// Re-sharing is free, so the number of file references has no natural bound,
/// and `Files` is one frame. The listing keeps the newest that fit.
#[test]
fn the_file_listing_is_capped_to_fit_one_frame() {
    let mut state = PartyState::new("Open", None);
    let alice = state.join("alice", None, None).unwrap();
    let chan = state.default_channel();
    for i in 0..(MAX_FILE_LISTING + 5) {
        state
            .post_file(
                alice,
                chan,
                format!("f{i}.txt"),
                "text/plain".into(),
                format!("{i}").into_bytes(),
            )
            .unwrap();
    }
    let listing = state.files_for(alice);
    assert_eq!(listing.len(), MAX_FILE_LISTING);
    assert!(PartyResponse::Files(listing.clone()).to_bytes().len() <= MAX_PARTY_PAYLOAD_BYTES);

    // The rest is reachable page by page, with nothing repeated or skipped.
    let (first, next) = state.files_page(alice, 0);
    assert_eq!(first, listing);
    assert_eq!(next, Some(MAX_FILE_LISTING));
    let (rest, next) = state.files_page(alice, MAX_FILE_LISTING);
    assert_eq!(rest.len(), 5);
    assert_eq!(next, None);
    let mut names: Vec<String> = first.iter().chain(&rest).map(|f| f.name.clone()).collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), MAX_FILE_LISTING + 5);
    assert_eq!(state.files_page(alice, 10_000), (Vec::new(), None));

    let mut items: Vec<String> = (0..10).map(|i| "z".repeat(100 + i)).collect();
    truncate_listing(&mut items, 100, 3 * 120);
    assert_eq!(items.len(), 3, "the byte budget trims too");
}

/// The directory is one frame, so membership has a ceiling. A returning
/// member is always let back in.
#[test]
fn membership_is_capped_but_returning_members_still_join() {
    let mut state = PartyState::new("Open", None);
    state.set_max_members(2);
    let a = state.join("a", None, Some("fp-a".into())).unwrap();
    state.join("b", None, Some("fp-b".into())).unwrap();
    assert_eq!(
        state.join("c", None, Some("fp-c".into())),
        Err(JoinError::ServerFull)
    );
    assert_eq!(state.join("a", None, Some("fp-a".into())), Ok(a));

    // The ceiling is a cap, not a knob: it cannot be raised past the frame.
    state.set_max_members(usize::MAX);
    assert_eq!(state.max_members, MAX_MEMBERS);
    let directory: Vec<MemberInfo> = (0..MAX_MEMBERS)
        .map(|_| MemberInfo {
            id: Uuid::new_v4(),
            username: "\u{1F600}".repeat(MAX_USERNAME_CHARS),
            online: true,
            role: Role::Member,
        })
        .collect();
    assert!(PartyResponse::Members(directory).to_bytes().len() <= MAX_PARTY_PAYLOAD_BYTES);
}

/// Every channel's private membership travels in the one `Channels` frame.
#[test]
fn a_private_channel_member_list_is_capped_and_deduplicated() {
    let mut state = PartyState::new("Open", None);
    let owner = join_owner(&mut state, "owner");
    let others: Vec<Uuid> = (0..MAX_PRIVATE_CHANNEL_MEMBERS)
        .map(|i| state.join(&format!("m{i}"), None, None).unwrap())
        .collect();
    // `owner` plus every other member is one too many.
    assert!(state
        .create_channel_of_kind(owner, "big", ChannelKind::Private, others.clone())
        .is_err());
    let dupes = vec![others[0], others[0], others[1], owner];
    let info = state
        .create_channel_of_kind(owner, "small", ChannelKind::Private, dupes)
        .unwrap();
    assert_eq!(info.members.len(), 3, "duplicates are dropped");
    assert!(state
        .set_channel_access(owner, info.id, ChannelKind::Private, others)
        .is_err());

    // The worst case the caps allow still fits in a frame.
    let channels: Vec<ChannelInfo> = (0..MAX_CHANNELS)
        .map(|_| ChannelInfo {
            id: Uuid::new_v4(),
            name: "\u{1F600}".repeat(MAX_CHANNEL_NAME_CHARS),
            kind: ChannelKind::Private,
            members: (0..MAX_PRIVATE_CHANNEL_MEMBERS)
                .map(|_| Uuid::new_v4())
                .collect(),
        })
        .collect();
    assert!(PartyResponse::Channels(channels).to_bytes().len() <= MAX_PARTY_PAYLOAD_BYTES);
}

/// A database written before names and types were bounded is repaired on
/// load, so a poisoned community can be joined again without hand-editing it.
#[test]
fn oversized_file_metadata_from_an_older_server_is_repaired_on_load() {
    let env = Envelope {
        tier: TrustTier::Administered,
        sender: Uuid::new_v4(),
        channel: Uuid::new_v4(),
        seq: 1,
        timestamp: 0,
        payload: MessagePayload::File(FileMeta {
            hash: "00".repeat(32),
            name: "n".repeat(100_000),
            size: 1,
            mime: format!("text/{}", "a".repeat(100_000)),
        }),
    };
    let repaired = repair_envelope(env);
    assert!(encoded_len(&repaired) < 1024);
}

/// Collect a thread's whole history by paging, as a client does.
fn page_all(mut fetch: impl FnMut(u64) -> Vec<Envelope>) -> Vec<u64> {
    let mut since = 0;
    let mut seqs = Vec::new();
    loop {
        let page = fetch(since);
        let Some(last) = page.last() else { break };
        since = last.seq;
        seqs.extend(page.iter().map(|e| e.seq));
    }
    seqs
}

/// SRV-06: every message ever posted used to stay in memory for the life of
/// the process and be deserialized at startup, so RSS and restart time grew
/// with the community's whole history. A disk-backed server keeps only each
/// thread's newest envelopes resident and pages older ones from SQLite.
#[test]
fn only_the_newest_history_stays_in_memory_and_older_pages_come_from_disk() {
    let dir = tempfile::tempdir().unwrap();
    let total = RESIDENT_HISTORY + 60;
    let (alice, bob, chan);
    {
        let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
        alice = s.join("alice", None, None).unwrap();
        bob = s.join("bob", None, None).unwrap();
        chan = s.default_channel();
        for i in 0..total {
            s.post_message(alice, chan, format!("c{i}")).unwrap();
            s.post_dm(alice, bob, format!("d{i}")).unwrap();
        }
        assert_eq!(
            s.channel(chan).unwrap().history.resident_len(),
            RESIDENT_HISTORY
        );
        let thread = messenger_core::party::dm_thread_id(alice, bob);
        assert_eq!(
            s.dm_threads[&thread].history.resident_len(),
            RESIDENT_HISTORY
        );

        // Paging from the start reaches back past what memory holds.
        let all: Vec<u64> = (1..=total as u64).collect();
        assert_eq!(page_all(|since| s.history_since(alice, chan, since)), all);
        assert_eq!(page_all(|since| s.dm_history(thread, since)), all);
    }

    // A restart loads only the tail, and numbering carries on from the
    // highest stored seq rather than from how many envelopes are resident.
    let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
    assert_eq!(
        s.channel(chan).unwrap().history.resident_len(),
        RESIDENT_HISTORY
    );
    let next = s.post_message(alice, chan, "after restart".into()).unwrap();
    assert_eq!(next.seq, total as u64 + 1);
    assert_eq!(
        page_all(|since| s.history_since(alice, chan, since)),
        (1..=total as u64 + 1).collect::<Vec<_>>()
    );
    // A page entirely inside the resident tail is served from memory.
    let recent = s.history_since(alice, chan, total as u64 - 3);
    assert_eq!(recent.len(), 4);
}

/// The resident cap is a cache bound, not a retention rule: a small cap still
/// serves every message, and access control still applies to disk pages.
#[test]
fn a_tiny_resident_cap_still_serves_everything_to_those_who_may_read_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
    s.set_resident_history(3);
    let alice = s.join("alice", None, None).unwrap();
    let chan = s.default_channel();
    for i in 0..20 {
        s.post_message(alice, chan, format!("m{i}")).unwrap();
    }
    assert_eq!(s.channel(chan).unwrap().history.resident_len(), 3);
    let texts: Vec<String> = s
        .history_since(alice, chan, 0)
        .into_iter()
        .map(|e| match e.payload {
            MessagePayload::Text(t) => t,
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(texts, (0..20).map(|i| format!("m{i}")).collect::<Vec<_>>());
    // A stranger still gets nothing, from memory or disk.
    assert!(s.history_since(Uuid::new_v4(), chan, 0).is_empty());
}

/// One damaged message row used to fail the whole load, so the server would
/// not start at all. It is skipped instead.
#[test]
fn an_undecodable_stored_message_is_skipped_rather_than_fatal() {
    let dir = tempfile::tempdir().unwrap();
    let (alice, chan);
    {
        let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
        alice = s.join("alice", None, None).unwrap();
        chan = s.default_channel();
        s.post_message(alice, chan, "one".into()).unwrap();
        s.post_message(alice, chan, "two".into()).unwrap();
        s.db.as_ref()
            .unwrap()
            .execute(
                "UPDATE messages SET envelope = 'not json' WHERE seq = 1",
                [],
            )
            .unwrap();
    }
    let mut s = PartyState::load("Srv", None, dir.path()).expect("the server still starts");
    let seqs: Vec<u64> = s
        .history_since(alice, chan, 0)
        .iter()
        .map(|e| e.seq)
        .collect();
    assert_eq!(seqs, vec![2]);
    assert_eq!(s.post_message(alice, chan, "three".into()).unwrap().seq, 3);
}

/// SRV-06: without a retention period everything was kept forever. With one,
/// old messages and file shares go — but each thread keeps its newest message,
/// so numbering continues across a restart instead of reusing seqs clients
/// already hold.
#[test]
fn retention_prunes_old_history_and_files_but_keeps_numbering() {
    let dir = tempfile::tempdir().unwrap();
    let channel;
    let alice;
    {
        let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
        alice = s.join("alice", None, None).unwrap();
        channel = s.default_channel();
        for text in ["one", "two", "three"] {
            s.post_message(alice, channel, text.to_string()).unwrap();
        }
        s.post_file(
            alice,
            channel,
            "notes.txt".to_string(),
            "text/plain".to_string(),
            b"old notes".to_vec(),
        )
        .unwrap();
        assert_eq!(s.files_for(alice).len(), 1);

        // Nothing happens without a retention period.
        let later = messenger_core::util::current_timestamp_millis() + 3 * 86_400_000;
        assert_eq!(s.prune_expired(later), PruneReport::default());

        s.set_retention(Some(std::time::Duration::from_secs(86_400)));
        let report = s.prune_expired(later);
        // Three texts and the file message are old; the newest (the file
        // message) is kept so the next seq is still known.
        assert_eq!(report.messages, 3);
        assert_eq!(report.files, 1);
        assert!(s.files_for(alice).is_empty());
        let left: Vec<u64> = s
            .history_since(alice, channel, 0)
            .iter()
            .map(|e| e.seq)
            .collect();
        assert_eq!(left, vec![4]);
    }
    let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
    let left: Vec<u64> = s
        .history_since(alice, channel, 0)
        .iter()
        .map(|e| e.seq)
        .collect();
    assert_eq!(left, vec![4], "pruned rows stay pruned across a restart");
    let next = s.post_message(alice, channel, "five".to_string()).unwrap();
    assert_eq!(next.seq, 5, "numbering continues, never reusing a seq");
}

/// Retention also applies to a server without a database (in-memory history).
#[test]
fn retention_prunes_in_memory_history_too() {
    let mut s = PartyState::new("Srv", None);
    let alice = s.join("alice", None, None).unwrap();
    let channel = s.default_channel();
    for text in ["a", "b"] {
        s.post_message(alice, channel, text.to_string()).unwrap();
    }
    s.set_retention(Some(std::time::Duration::from_secs(60)));
    let report = s.prune_expired(messenger_core::util::current_timestamp_millis() + 120_000);
    assert_eq!(report.messages, 1);
    assert_eq!(s.history_since(alice, channel, 0).len(), 1);
}
