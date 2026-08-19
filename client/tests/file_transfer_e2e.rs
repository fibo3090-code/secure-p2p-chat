//! End-to-end file transfer between two real `ChatManager`s over a loopback
//! socket: `send_file` → v3 session → chunked wire frames → spool → the
//! acceptance gate → the receiver's download directory.
//!
//! Everything below the manager is the production path — the real handshake, the
//! real `FileMeta`/`FileChunk`/`FileEnd` frames, the real receiver. The pieces
//! this covers are the ones whose failure modes are silent:
//!
//! - **The acceptance gate.** `auto_accept_files` is off by default. Nothing may
//!   reach the download directory or the chat history until `accept_incoming_file`
//!   is called, and declining must both delete the spool *and* tell the sender to
//!   stop — a decline the sender never hears about still pulls the whole file
//!   across the wire.
//! - **Bytes arriving intact**, since a chunking bug corrupts a file rather than
//!   failing it.
//! - **Cancellation from either end**, which has to leave no stuck progress row
//!   and no orphaned temp file.

use messenger_core::core::generate_rsa_keypair;
use messenger_core::types::{Config, MessageContent, TransferStatus};
use messenger_core::RSA_KEY_BITS;
use p2pem_classic::app::ChatManager;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use uuid::Uuid;

/// How long any single "pump until" step may take before the test fails rather
/// than hangs.
///
/// Deliberately generous — it is a hang detector, not a performance assertion.
/// A debug-build RSA handshake is slow, and under a parallel runner several run
/// at once; a tight bound here turns load into a flake.
const STEP_TIMEOUT: Duration = Duration::from_secs(90);

/// Reserve an ephemeral loopback port, then release it for the host to bind.
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    listener.local_addr().expect("local addr").port()
}

/// A manager with its own download/temp directories, so two peers in one test
/// never write over each other.
fn manager_in(dir: &Path, auto_accept: bool) -> ChatManager {
    let download = dir.join("downloads");
    let temp = dir.join("tmp");
    std::fs::create_dir_all(&download).expect("download dir");
    std::fs::create_dir_all(&temp).expect("temp dir");
    let config = Config {
        download_dir: download,
        temp_dir: temp,
        auto_accept_files: auto_accept,
        ..Config::default()
    };
    ChatManager::new(config)
}

/// Drive both managers' event loops until `done` reports true, or fail.
///
/// Both sides have to be pumped: a transfer only advances when the *receiver*
/// drains its session events, and the sender's own progress is mirrored out of
/// the streaming task by the same poll.
async fn pump_until<F>(a: &mut ChatManager, b: &mut ChatManager, label: &str, mut done: F)
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
        if Instant::now() > deadline {
            panic!("[{label}] timed out");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

struct Pair {
    host: ChatManager,
    client: ChatManager,
    /// The host-side conversation for this peer. The host creates a *new* chat
    /// keyed by the client's own chat id, so this is not the placeholder id
    /// `start_host` returned.
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

    // TOFU on both ends. The prompt is a queue read by peeking, and
    // `confirm_fingerprint` is what removes the entry and persists the trust.
    pump_until(&mut host, &mut client, "tofu prompts", |h, c| {
        h.pending_fingerprint().is_some() && c.pending_fingerprint().is_some()
    })
    .await;

    // The prompt carries the SESSION id, which on the host side is the
    // listener placeholder's — not the conversation the peer will live in.
    let host_prompt = host.pending_fingerprint().expect("host prompt").clone();
    let client_prompt = client.pending_fingerprint().expect("client prompt").clone();
    host.confirm_fingerprint(host_prompt.session_id, true)
        .expect("host accepts");
    client
        .confirm_fingerprint(client_prompt.session_id, true)
        .expect("client accepts");

    // The host creates a new chat per incoming connection, keyed by the
    // *client's* chat id, so find it by the fingerprint that was just stored on
    // it rather than assuming an id.
    let mut host_chat = None;
    pump_until(&mut host, &mut client, "sessions ready", |h, c| {
        host_chat = h
            .chats
            .iter()
            .find(|(_, chat)| chat.peer_fingerprint.is_some() && !chat.is_host_placeholder)
            .map(|(id, _)| *id);
        host_chat.is_some() && c.chats.contains_key(&client_chat)
    })
    .await;
    let host_chat = host_chat.expect("a host-side conversation for the peer");

    Pair {
        host,
        client,
        host_chat,
        client_chat,
        _dirs: (host_dir, client_dir),
    }
}

/// Write a file of `len` bytes with a non-repeating pattern, so a chunking bug
/// that duplicates or drops a chunk changes the contents rather than the length.
fn write_test_file(dir: &Path, name: &str, len: usize) -> (PathBuf, Vec<u8>) {
    let bytes: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
    let path = dir.join(name);
    std::fs::write(&path, &bytes).expect("write test file");
    (path, bytes)
}

fn transfers_for(mgr: &ChatManager, chat: Uuid) -> Vec<messenger_core::types::FileTransferState> {
    mgr.active_transfers_snapshot()
        .into_iter()
        .filter(|t| t.chat_id == chat)
        .collect()
}

/// Entries in `dir`, split into the receiver's in-progress spools (`tmp_<uuid>_`)
/// and finished files.
///
/// The spool deliberately lives *in the download directory*: finalizing is a
/// `rename`, and a rename is only atomic within one filesystem. So "nothing has
/// been saved" means no finished file — not an empty directory.
fn dir_entries(dir: &Path) -> (Vec<String>, Vec<String>) {
    let mut spools = Vec::new();
    let mut finished = Vec::new();
    for entry in std::fs::read_dir(dir).expect("read dir").flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("tmp_") {
            spools.push(name);
        } else {
            finished.push(name);
        }
    }
    (spools, finished)
}

fn received_file_message(mgr: &ChatManager, chat: Uuid) -> Option<(String, PathBuf)> {
    mgr.chats
        .get(&chat)?
        .messages
        .iter()
        .rev()
        .find_map(|m| match &m.content {
            MessageContent::File { filename, path, .. } if !m.from_me => {
                Some((filename.clone(), path.clone()?))
            }
            _ => None,
        })
}

/// The whole path, with the gate on: offer held, nothing on disk, then accepted
/// and the exact bytes land in the download directory.
#[tokio::test]
async fn a_file_crosses_the_wire_and_lands_after_the_user_accepts() {
    let mut p = connected_pair(false, false).await;
    let src_dir = tempfile::tempdir().expect("src dir");
    // Several chunks' worth: FILE_CHUNK_SIZE is 64 KiB.
    let (path, expected) = write_test_file(src_dir.path(), "report.pdf", 200 * 1024);

    p.host
        .send_file(p.host_chat, path)
        .await
        .expect("send_file");

    // The receiver holds the offer: `auto_accept_files` is off.
    pump_until(&mut p.host, &mut p.client, "offer held", |_, c| {
        transfers_for(c, c.chats.keys().copied().next().unwrap_or_default())
            .iter()
            .any(|t| t.status == TransferStatus::AwaitingAcceptance)
            || c.active_transfers_snapshot()
                .iter()
                .any(|t| t.status == TransferStatus::AwaitingAcceptance)
    })
    .await;

    let offer = p
        .client
        .active_transfers_snapshot()
        .into_iter()
        .find(|t| t.status == TransferStatus::AwaitingAcceptance)
        .expect("an offer awaiting acceptance");
    assert_eq!(offer.filename, "report.pdf");

    // Chunks may be spooling, but nothing may be *saved*: no finished file in
    // the download directory, and nothing in the conversation.
    let downloads = p.client.config.download_dir.clone();
    let (_spools, finished) = dir_entries(&downloads);
    assert!(
        finished.is_empty(),
        "nothing may be saved before the user accepts, found {finished:?}"
    );
    assert!(received_file_message(&p.client, p.client_chat).is_none());

    p.client
        .accept_incoming_file(offer.id)
        .expect("accept the offer");

    pump_until(&mut p.host, &mut p.client, "file finalized", |_, c| {
        c.chats.values().any(|chat| {
            chat.messages
                .iter()
                .any(|m| !m.from_me && matches!(&m.content, MessageContent::File { .. }))
        })
    })
    .await;

    let (name, saved) = p
        .client
        .chats
        .values()
        .find_map(|chat| {
            chat.messages.iter().rev().find_map(|m| match &m.content {
                MessageContent::File { filename, path, .. } if !m.from_me => {
                    Some((filename.clone(), path.clone()?))
                }
                _ => None,
            })
        })
        .expect("a received file message with a path");

    assert_eq!(name, "report.pdf");
    assert!(
        saved.starts_with(&downloads),
        "the file must land in the configured download dir, got {}",
        saved.display()
    );
    // Byte-exact: a chunking bug corrupts rather than fails.
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
    let src_dir = tempfile::tempdir().expect("src dir");
    let (path, expected) = write_test_file(src_dir.path(), "notes.txt", 8 * 1024);

    p.host
        .send_file(p.host_chat, path)
        .await
        .expect("send_file");

    pump_until(&mut p.host, &mut p.client, "auto-accepted", |_, c| {
        c.chats.values().any(|chat| {
            chat.messages
                .iter()
                .any(|m| !m.from_me && matches!(&m.content, MessageContent::File { .. }))
        })
    })
    .await;

    let saved = p
        .client
        .chats
        .values()
        .find_map(|chat| {
            chat.messages.iter().rev().find_map(|m| match &m.content {
                MessageContent::File { path, .. } if !m.from_me => path.clone(),
                _ => None,
            })
        })
        .expect("a saved file");
    assert_eq!(std::fs::read(&saved).expect("read saved"), expected);
}

/// Declining deletes the spool and leaves the download directory untouched.
///
/// It must also emit `FileCancel`: a decline the sender never hears about still
/// pulls the entire file across the wire, so the user's "no" only ever reached
/// their own disk.
#[tokio::test]
async fn declining_an_offer_saves_nothing_and_stops_the_sender() {
    let mut p = connected_pair(false, false).await;
    let src_dir = tempfile::tempdir().expect("src dir");
    let (path, _) = write_test_file(src_dir.path(), "unwanted.bin", 512 * 1024);

    p.host
        .send_file(p.host_chat, path)
        .await
        .expect("send_file");

    pump_until(&mut p.host, &mut p.client, "offer arrives", |_, c| {
        c.active_transfers_snapshot()
            .iter()
            .any(|t| t.status == TransferStatus::AwaitingAcceptance)
    })
    .await;

    let offer = p
        .client
        .active_transfers_snapshot()
        .into_iter()
        .find(|t| t.status == TransferStatus::AwaitingAcceptance)
        .expect("an offer");
    p.client
        .reject_incoming_file(offer.id)
        .expect("decline the offer");

    // The sender is told to stop.
    pump_until(&mut p.host, &mut p.client, "sender stops", |h, _| {
        h.active_transfers_snapshot().iter().all(|t| {
            matches!(
                t.status,
                TransferStatus::Cancelled | TransferStatus::Failed(_) | TransferStatus::Completed
            )
        })
    })
    .await;

    // Nothing saved, and no orphaned spool left behind.
    let downloads = p.client.config.download_dir.clone();
    let (spools, finished) = dir_entries(&downloads);
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

/// The sender cancels mid-flight. Neither side may be left with a transfer that
/// still reads as in progress, and nothing may be saved.
#[tokio::test]
async fn the_sender_can_cancel_and_neither_side_is_left_hanging() {
    let mut p = connected_pair(false, true).await;
    let src_dir = tempfile::tempdir().expect("src dir");
    // Large enough that the cancel lands before the stream finishes.
    let (path, _) = write_test_file(src_dir.path(), "big.iso", 4 * 1024 * 1024);

    p.host
        .send_file(p.host_chat, path)
        .await
        .expect("send_file");

    pump_until(&mut p.host, &mut p.client, "transfer started", |h, _| {
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

    pump_until(&mut p.host, &mut p.client, "both settle", |h, c| {
        let settled = |m: &ChatManager| {
            m.active_transfers_snapshot().iter().all(|t| {
                matches!(
                    t.status,
                    TransferStatus::Cancelled
                        | TransferStatus::Failed(_)
                        | TransferStatus::Completed
                )
            })
        };
        settled(h) && settled(c)
    })
    .await;

    assert!(
        !p.host
            .active_transfers_snapshot()
            .iter()
            .any(|t| t.status == TransferStatus::InProgress),
        "the sender must not be left with a running transfer"
    );
    assert!(
        !p.client
            .active_transfers_snapshot()
            .iter()
            .any(|t| t.status == TransferStatus::InProgress),
        "the receiver must not be left with a stuck progress row"
    );
}

/// One outgoing transfer per conversation. `FileChunk` carries no transfer id,
/// so two concurrent sends interleave into whichever spool the receiver has open
/// and corrupt both files — the guard is what makes the wire format safe.
#[tokio::test]
async fn a_second_concurrent_send_on_one_conversation_is_refused() {
    let mut p = connected_pair(false, true).await;
    let src_dir = tempfile::tempdir().expect("src dir");
    let (first, _) = write_test_file(src_dir.path(), "one.bin", 2 * 1024 * 1024);
    let (second, _) = write_test_file(src_dir.path(), "two.bin", 2 * 1024 * 1024);

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
    let src_dir = tempfile::tempdir().expect("src dir");
    let (path, _) = write_test_file(src_dir.path(), "orphan.txt", 16);

    let err = mgr
        .send_file(Uuid::new_v4(), path)
        .await
        .expect_err("no session means no send");
    assert!(!err.to_string().is_empty());
}
