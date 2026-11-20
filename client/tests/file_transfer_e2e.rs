//! End-to-end file transfer between two real `ChatManager`s over loopback:
//! `send_file` → session → `FileMeta`/`FileChunk`/`FileEnd` → spool → the
//! acceptance gate → the download directory. What it guards fails silently:
//!
//! - **The acceptance gate** (`auto_accept_files` defaults off): nothing reaches
//!   the download dir or history before `accept_incoming_file`, and a decline
//!   deletes the spool *and* tells the sender to stop.
//! - **Bytes arriving intact** (a chunking bug corrupts rather than fails).
//! - **Cancellation from either end** leaves no stuck row or orphaned spool.

mod common;

use common::*;
use messenger_core::core::generate_rsa_keypair;
use messenger_core::types::{FileTransferState, MessageContent, TransferStatus};
use messenger_core::RSA_KEY_BITS;
use p2pem_classic::app::ChatManager;
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

struct Pair {
    host: ChatManager,
    client: ChatManager,
    /// The host-side conversation: the host creates a *new* chat keyed by the
    /// client's chat id, not the placeholder `start_host` returned.
    host_chat: Uuid,
    client_chat: Uuid,
    _dirs: (tempfile::TempDir, tempfile::TempDir),
}

/// Host + client, connected, both fingerprints confirmed, ready to transfer.
async fn connected_pair(host_auto_accept: bool, client_auto_accept: bool) -> Pair {
    let host_dir = tempfile::tempdir().expect("host dir");
    let client_dir = tempfile::tempdir().expect("client dir");
    let mut host = manager_in(host_dir.path(), host_auto_accept);
    let mut client = manager_in(client_dir.path(), client_auto_accept);

    let port = free_port();
    let host_key = generate_rsa_keypair(RSA_KEY_BITS).expect("host key");
    let client_key = generate_rsa_keypair(RSA_KEY_BITS).expect("client key");

    host.start_host(port, host_key).await.expect("start host");
    // Let the listener bind before dialling it.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let client_chat = client
        .connect_to_host("127.0.0.1", port, None, client_key)
        .await
        .expect("connect to host");

    // TOFU on both ends: the prompt queue is peeked, and `confirm_fingerprint`
    // removes the entry and persists the trust.
    pump_pair(&mut host, &mut client, "tofu prompts", |h, c| {
        h.pending_fingerprint().is_some() && c.pending_fingerprint().is_some()
    })
    .await;
    // The prompt carries the SESSION id (on the host, the listener's).
    let host_prompt = host.pending_fingerprint().expect("host prompt").clone();
    let client_prompt = client.pending_fingerprint().expect("client prompt").clone();
    host.confirm_fingerprint(host_prompt.session_id, true)
        .expect("host accepts");
    client
        .confirm_fingerprint(client_prompt.session_id, true)
        .expect("client accepts");

    // Find the host's new chat by the fingerprint just stored on it.
    let mut host_chat = None;
    pump_pair(&mut host, &mut client, "sessions ready", |h, c| {
        host_chat = h
            .chats
            .iter()
            .find(|(_, chat)| chat.peer_fingerprint.is_some() && !chat.is_host_placeholder)
            .map(|(id, _)| *id);
        host_chat.is_some() && c.chats.contains_key(&client_chat)
    })
    .await;

    Pair {
        host,
        client,
        host_chat: host_chat.expect("a host-side conversation for the peer"),
        client_chat,
        _dirs: (host_dir, client_dir),
    }
}

/// A file of `len` bytes in its own temp dir, with a non-repeating pattern so a
/// duplicated or dropped chunk changes the contents, not just the length.
fn test_file(name: &str, len: usize) -> (tempfile::TempDir, PathBuf, Vec<u8>) {
    let dir = tempfile::tempdir().expect("src dir");
    let bytes: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
    let path = dir.path().join(name);
    std::fs::write(&path, &bytes).expect("write test file");
    (dir, path, bytes)
}

fn awaiting(mgr: &ChatManager) -> Option<FileTransferState> {
    mgr.active_transfers_snapshot()
        .into_iter()
        .find(|t| t.status == TransferStatus::AwaitingAcceptance)
}

/// Every transfer has finished one way or another.
fn settled(mgr: &ChatManager) -> bool {
    mgr.active_transfers_snapshot().iter().all(|t| {
        matches!(
            t.status,
            TransferStatus::Cancelled | TransferStatus::Failed(_) | TransferStatus::Completed
        )
    })
}

fn in_progress(mgr: &ChatManager) -> bool {
    mgr.active_transfers_snapshot()
        .iter()
        .any(|t| t.status == TransferStatus::InProgress)
}

/// Entries in `dir`, split into in-progress spools (`tmp_<uuid>_`) and finished
/// files. The spool lives *in* the download dir (finalizing is a rename, atomic
/// only within one filesystem), so "nothing saved" means no finished file.
fn dir_entries(dir: &Path) -> (Vec<String>, Vec<String>) {
    std::fs::read_dir(dir)
        .expect("read dir")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .partition(|name| name.starts_with("tmp_"))
}

/// The latest received file message with a saved path, in any conversation.
fn received_file(mgr: &ChatManager) -> Option<(String, PathBuf)> {
    mgr.chats.values().find_map(|chat| {
        chat.messages.iter().rev().find_map(|m| match &m.content {
            MessageContent::File { filename, path, .. } if !m.from_me => {
                Some((filename.clone(), path.clone()?))
            }
            _ => None,
        })
    })
}

/// The whole path with the gate on: offer held, nothing on disk, then accepted
/// and the exact bytes land in the download directory.
#[tokio::test]
async fn a_file_crosses_the_wire_and_lands_after_the_user_accepts() {
    let mut p = connected_pair(false, false).await;
    // Several chunks' worth: FILE_CHUNK_SIZE is 64 KiB.
    let (_src, path, expected) = test_file("report.pdf", 200 * 1024);
    p.host
        .send_file(p.host_chat, path)
        .await
        .expect("send_file");

    pump_pair(&mut p.host, &mut p.client, "offer held", |_, c| {
        awaiting(c).is_some()
    })
    .await;
    let offer = awaiting(&p.client).expect("an offer awaiting acceptance");
    assert_eq!(offer.filename, "report.pdf");

    // Chunks may spool, but nothing may be *saved* or appear in the history.
    let downloads = p.client.config.download_dir.clone();
    let (_spools, finished) = dir_entries(&downloads);
    assert!(
        finished.is_empty(),
        "nothing may be saved before the user accepts, found {finished:?}"
    );
    assert!(received_file(&p.client).is_none());

    p.client
        .accept_incoming_file(offer.id)
        .expect("accept the offer");
    pump_pair(&mut p.host, &mut p.client, "file finalized", |_, c| {
        received_file(c).is_some()
    })
    .await;

    let (name, saved) = received_file(&p.client).expect("a received file message with a path");
    assert_eq!(name, "report.pdf");
    assert!(
        saved.starts_with(&downloads),
        "the file must land in the configured download dir, got {}",
        saved.display()
    );
    assert_eq!(
        std::fs::read(&saved).expect("read saved file"),
        expected,
        "the received bytes must match what was sent"
    );
}

/// With the gate off, the file lands without anyone being asked.
#[tokio::test]
async fn auto_accept_saves_without_prompting() {
    let mut p = connected_pair(false, true).await;
    let (_src, path, expected) = test_file("notes.txt", 8 * 1024);
    p.host
        .send_file(p.host_chat, path)
        .await
        .expect("send_file");

    pump_pair(&mut p.host, &mut p.client, "auto-accepted", |_, c| {
        received_file(c).is_some()
    })
    .await;
    let (_, saved) = received_file(&p.client).expect("a saved file");
    assert_eq!(std::fs::read(&saved).expect("read saved"), expected);
}

/// Declining deletes the spool, saves nothing, and emits `FileCancel` — a
/// decline the sender never hears about still pulls the whole file across.
#[tokio::test]
async fn declining_an_offer_saves_nothing_and_stops_the_sender() {
    let mut p = connected_pair(false, false).await;
    let (_src, path, _) = test_file("unwanted.bin", 512 * 1024);
    p.host
        .send_file(p.host_chat, path)
        .await
        .expect("send_file");

    pump_pair(&mut p.host, &mut p.client, "offer arrives", |_, c| {
        awaiting(c).is_some()
    })
    .await;
    let offer = awaiting(&p.client).expect("an offer");
    p.client
        .reject_incoming_file(offer.id)
        .expect("decline the offer");

    pump_pair(&mut p.host, &mut p.client, "sender stops", |h, _| {
        settled(h)
    })
    .await;

    let (spools, finished) = dir_entries(&p.client.config.download_dir);
    assert!(
        finished.is_empty(),
        "a declined file must not be saved, found {finished:?}"
    );
    assert!(
        spools.is_empty(),
        "the declined spool must be deleted, found {spools:?}"
    );
    assert!(
        !p.client.chats.values().any(|c| c
            .messages
            .iter()
            .any(|m| matches!(&m.content, MessageContent::File { .. }))),
        "a declined file must not appear in the history"
    );
}

/// The sender cancels mid-flight; neither side may be left with a transfer
/// that still reads as in progress.
#[tokio::test]
async fn the_sender_can_cancel_and_neither_side_is_left_hanging() {
    let mut p = connected_pair(false, true).await;
    // Large enough that the cancel lands before the stream finishes.
    let (_src, path, _) = test_file("big.iso", 4 * 1024 * 1024);
    p.host
        .send_file(p.host_chat, path)
        .await
        .expect("send_file");

    pump_pair(&mut p.host, &mut p.client, "transfer started", |h, _| {
        !h.active_transfers_snapshot().is_empty()
    })
    .await;
    let outgoing = p
        .host
        .active_transfers_snapshot()
        .into_iter()
        .next()
        .expect("an outgoing transfer");
    p.host.cancel_transfer(outgoing.id);

    pump_pair(&mut p.host, &mut p.client, "both settle", |h, c| {
        settled(h) && settled(c)
    })
    .await;
    assert!(
        !in_progress(&p.host),
        "the sender must not be left with a running transfer"
    );
    assert!(
        !in_progress(&p.client),
        "the receiver must not be left with a stuck progress row"
    );
}

/// One outgoing transfer per conversation: `FileChunk` carries no transfer id,
/// so two concurrent sends would interleave and corrupt both files.
#[tokio::test]
async fn a_second_concurrent_send_on_one_conversation_is_refused() {
    let mut p = connected_pair(false, true).await;
    let (_src1, first, _) = test_file("one.bin", 2 * 1024 * 1024);
    let (_src2, second, _) = test_file("two.bin", 2 * 1024 * 1024);

    p.host
        .send_file(p.host_chat, first)
        .await
        .expect("the first send is allowed");
    let err = p
        .host
        .send_file(p.host_chat, second)
        .await
        .expect_err("a second concurrent send must be refused");
    let msg = err.to_string();
    assert!(
        msg.contains("one.bin") && msg.to_lowercase().contains("still sending"),
        "the refusal should name the transfer in the way and what to do: {msg}"
    );
}

/// A send with no session must return `Err`, never `Ok` plus a toast: a
/// front-end reads success as "sent".
#[tokio::test]
async fn sending_a_file_with_no_session_is_an_error() {
    let dir = tempfile::tempdir().expect("dir");
    let mut mgr = manager_in(dir.path(), false);
    let (_src, path, _) = test_file("orphan.txt", 16);
    let err = mgr
        .send_file(Uuid::new_v4(), path)
        .await
        .expect_err("no session means no send");
    assert!(!err.to_string().is_empty());
}

/// A second offer while the first awaits a decision must not touch the first.
/// Chunks route to the transfer *streaming*; a held offer used to count as
/// streaming after its `FileEnd`, so the second file's chunks overflowed the
/// first one's spool and deleted it.
#[tokio::test]
async fn a_second_offer_while_the_first_awaits_acceptance_leaves_both_intact() {
    let mut p = connected_pair(false, false).await;
    let (_src1, first, first_bytes) = test_file("first.bin", 200 * 1024);
    let (_src2, second, second_bytes) = test_file("second.bin", 150 * 1024);
    let held = |c: &ChatManager| {
        c.active_transfers_snapshot()
            .into_iter()
            .filter(|t| t.status == TransferStatus::AwaitingAcceptance && t.received == t.size)
            .count()
    };

    p.host
        .send_file(p.host_chat, first)
        .await
        .expect("send first");
    // The sender finishes; the receiver holds the complete file, undecided.
    let host_chat = p.host_chat;
    pump_pair(&mut p.host, &mut p.client, "first held", |h, c| {
        h.active_transfers_snapshot()
            .iter()
            .any(|t| t.chat_id == host_chat && t.status == TransferStatus::Completed)
            && held(c) >= 1
    })
    .await;

    p.host
        .send_file(p.host_chat, second)
        .await
        .expect("send second");
    pump_pair(&mut p.host, &mut p.client, "second held too", |_, c| {
        held(c) == 2
    })
    .await;

    for offer in p.client.active_transfers_snapshot() {
        p.client.accept_incoming_file(offer.id).expect("accept");
    }
    let client_chat = p.client_chat;
    pump_pair(&mut p.host, &mut p.client, "both saved", |_, c| {
        c.chats[&client_chat]
            .messages
            .iter()
            .filter(|m| !m.from_me && matches!(m.content, MessageContent::File { .. }))
            .count()
            == 2
    })
    .await;
    for (name, expected) in [("first.bin", &first_bytes), ("second.bin", &second_bytes)] {
        let saved = p.client.chats[&p.client_chat]
            .messages
            .iter()
            .find_map(|m| match &m.content {
                MessageContent::File { filename, path, .. } if filename == name => path.clone(),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{name} was not saved"));
        assert_eq!(
            &std::fs::read(saved).expect("read saved"),
            expected,
            "{name} corrupted"
        );
    }
}
