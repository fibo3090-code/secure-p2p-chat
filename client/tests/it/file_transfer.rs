//! File transfer between two real `ChatManager`s over loopback: `send_file` →
//! session → chunked frames → spool → the acceptance gate → the download
//! directory.
//!
//! What is pinned here is what only shows up when the pieces run together:
//! bytes arriving intact (a chunking bug corrupts a file rather than failing
//! it), the gate holding a file back until the user says yes, and neither side
//! being left with a stuck progress row or an orphaned spool.
//!
//! Wire-level details with deterministic answers — that a decline *emits*
//! `FileCancel`, that a second concurrent send is refused, that a send with no
//! session is an `Err` — are pinned by the `chat_manager` unit tests against a
//! captured session lane, where no race can make them pass by accident.

use crate::common::*;
use messenger_core::types::{FileTransferState, MessageContent, TransferStatus};
use p2pem_classic::app::ChatManager;
use std::path::{Path, PathBuf};
use uuid::Uuid;

struct Pair {
    host: ChatManager,
    client: ChatManager,
    host_chat: Uuid,
    client_chat: Uuid,
    _dirs: (tempfile::TempDir, tempfile::TempDir),
}

/// Host + client, connected, both fingerprints confirmed.
async fn connected_pair(host_auto_accept: bool, client_auto_accept: bool) -> Pair {
    let host_dir = tempfile::tempdir().expect("host dir");
    let client_dir = tempfile::tempdir().expect("client dir");
    let mut host = manager_in(host_dir.path());
    let mut client = manager_in(client_dir.path());
    host.config.auto_accept_files = host_auto_accept;
    client.config.auto_accept_files = client_auto_accept;

    let port = host_on_any_port(&mut host, rsa_key()).await;
    let (host_chat, client_chat) = dial_and_verify(&mut host, &mut client, port, rsa_key()).await;
    Pair {
        host,
        client,
        host_chat,
        client_chat,
        _dirs: (host_dir, client_dir),
    }
}

/// A file with a non-repeating pattern, so a chunk duplicated or dropped
/// changes the contents rather than only the length.
fn write_test_file(dir: &Path, name: &str, len: usize) -> (PathBuf, Vec<u8>) {
    let bytes: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
    let path = dir.join(name);
    std::fs::write(&path, &bytes).expect("write test file");
    (path, bytes)
}

fn awaiting(mgr: &ChatManager) -> Vec<FileTransferState> {
    mgr.active_transfers_snapshot()
        .into_iter()
        .filter(|t| t.status == TransferStatus::AwaitingAcceptance)
        .collect()
}

fn settled(mgr: &ChatManager) -> bool {
    mgr.active_transfers_snapshot().iter().all(|t| {
        matches!(
            t.status,
            TransferStatus::Cancelled | TransferStatus::Failed(_) | TransferStatus::Completed
        )
    })
}

/// Entries in `dir`, split into in-progress spools (`tmp_<uuid>_`) and finished
/// files. The spool lives *in* the download directory — finalizing is a rename,
/// and a rename is only atomic within one filesystem — so "nothing saved"
/// means no finished file, not an empty directory.
fn dir_entries(dir: &Path) -> (Vec<String>, Vec<String>) {
    let (mut spools, mut finished) = (Vec::new(), Vec::new());
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

/// Received files in `chat`, as `(name, saved path)`.
fn received_files(mgr: &ChatManager, chat: Uuid) -> Vec<(String, PathBuf)> {
    mgr.chats[&chat]
        .messages
        .iter()
        .filter_map(|m| match &m.content {
            MessageContent::File { filename, path, .. } if !m.from_me => {
                Some((filename.clone(), path.clone()?))
            }
            _ => None,
        })
        .collect()
}

/// Both directions over one session. Host → client goes through the gate
/// (`auto_accept_files` off, the default): held, nothing saved, then accepted
/// and byte-exact. Client → host is auto-accepted: it lands with nobody asked.
#[tokio::test]
async fn a_file_lands_byte_exact_through_the_gate_and_without_it() {
    let mut p = connected_pair(true, false).await;
    let src = tempfile::tempdir().expect("src dir");
    // Several chunks' worth: FILE_CHUNK_SIZE is 64 KiB.
    let (gated, gated_bytes) = write_test_file(src.path(), "report.pdf", 200 * 1024);
    let (open, open_bytes) = write_test_file(src.path(), "notes.txt", 8 * 1024);

    p.host.send_file(p.host_chat, gated).await.expect("send");
    pump_pair(&mut p.host, &mut p.client, "offer held", |_, c| {
        awaiting(c).iter().any(|t| t.received == t.size)
    })
    .await;
    let offer = awaiting(&p.client).remove(0);
    assert_eq!(offer.filename, "report.pdf");

    // Every byte has arrived, and still nothing may be saved.
    let downloads = p.client.config.download_dir.clone();
    let (_, finished) = dir_entries(&downloads);
    assert!(finished.is_empty(), "saved before acceptance: {finished:?}");
    assert!(received_files(&p.client, p.client_chat).is_empty());

    p.client.accept_incoming_file(offer.id).expect("accept");
    p.client
        .send_file(p.client_chat, open)
        .await
        .expect("send back");
    pump_pair(&mut p.host, &mut p.client, "both saved", |h, c| {
        !received_files(c, p.client_chat).is_empty() && !received_files(h, p.host_chat).is_empty()
    })
    .await;

    let (name, saved) = received_files(&p.client, p.client_chat).remove(0);
    assert_eq!(name, "report.pdf");
    assert!(
        saved.starts_with(&downloads),
        "landed outside the download dir: {}",
        saved.display()
    );
    assert_eq!(std::fs::read(&saved).expect("read"), gated_bytes);

    let (name, saved) = received_files(&p.host, p.host_chat).remove(0);
    assert_eq!(name, "notes.txt");
    assert_eq!(std::fs::read(&saved).expect("read"), open_bytes);
    assert!(
        awaiting(&p.host).is_empty(),
        "an auto-accepted file must never ask"
    );
}

/// Declining deletes the spool, saves nothing, records nothing, and leaves the
/// sender with no transfer that still reads as running.
#[tokio::test]
async fn declining_an_offer_saves_nothing_and_leaves_no_spool() {
    let mut p = connected_pair(false, false).await;
    let src = tempfile::tempdir().expect("src dir");
    let (path, _) = write_test_file(src.path(), "unwanted.bin", 512 * 1024);

    p.host.send_file(p.host_chat, path).await.expect("send");
    pump_pair(&mut p.host, &mut p.client, "offer arrives", |_, c| {
        !awaiting(c).is_empty()
    })
    .await;
    let offer = awaiting(&p.client).remove(0);
    p.client.reject_incoming_file(offer.id).expect("decline");

    pump_pair(&mut p.host, &mut p.client, "both settle", |h, c| {
        settled(h) && settled(c)
    })
    .await;
    let (spools, finished) = dir_entries(&p.client.config.download_dir);
    assert!(
        finished.is_empty(),
        "a declined file was saved: {finished:?}"
    );
    assert!(spools.is_empty(), "the declined spool survived: {spools:?}");
    assert!(received_files(&p.client, p.client_chat).is_empty());
}

/// The sender cancels mid-flight: the sender's row reads Cancelled, the
/// receiver's is not left in progress, and the half-sent file is not saved —
/// with auto-accept on, which is the case where a missed cancel would finalize
/// a truncated file as if it were whole.
#[tokio::test]
async fn the_sender_can_cancel_and_neither_side_is_left_hanging() {
    let mut p = connected_pair(false, true).await;
    let src = tempfile::tempdir().expect("src dir");
    // Far more than can cross before the cancel lands: the cancel is issued
    // on the first poll after the send starts.
    let (path, _) = write_test_file(src.path(), "big.iso", 32 * 1024 * 1024);

    p.host.send_file(p.host_chat, path).await.expect("send");
    pump_pair(&mut p.host, &mut p.client, "transfer started", |h, _| {
        !h.active_transfers_snapshot().is_empty()
    })
    .await;
    let outgoing = p.host.active_transfers_snapshot().remove(0);
    p.host.cancel_transfer(outgoing.id);

    pump_pair(&mut p.host, &mut p.client, "both settle", |h, c| {
        settled(h) && settled(c)
    })
    .await;
    let sender = p.host.active_transfers_snapshot();
    assert!(
        sender.iter().all(|t| t.status == TransferStatus::Cancelled),
        "the sender's transfer must read Cancelled, got {sender:?}"
    );
    let receiver = p.client.active_transfers_snapshot();
    assert!(
        receiver
            .iter()
            .all(|t| t.status != TransferStatus::Completed),
        "a cancelled file must not complete on the receiver, got {receiver:?}"
    );
    let (spools, finished) = dir_entries(&p.client.config.download_dir);
    assert!(
        finished.is_empty(),
        "a cancelled file was saved: {finished:?}"
    );
    assert!(
        spools.is_empty(),
        "the cancelled spool survived: {spools:?}"
    );
}

/// A second file sent while the first awaits the user's decision must not
/// touch the first.
///
/// `FileChunk` carries no transfer id, so chunks go to the transfer currently
/// streaming. A held offer used to count as streaming even after its
/// `FileEnd`: the second offer was refused and its chunks were written into
/// the *first* file's spool — overflowing it, failing it, deleting it.
#[tokio::test]
async fn a_second_offer_while_the_first_awaits_acceptance_leaves_both_intact() {
    let mut p = connected_pair(false, false).await;
    let src = tempfile::tempdir().expect("src dir");
    let (first, first_bytes) = write_test_file(src.path(), "first.bin", 200 * 1024);
    let (second, second_bytes) = write_test_file(src.path(), "second.bin", 150 * 1024);

    p.host
        .send_file(p.host_chat, first)
        .await
        .expect("send first");
    // The sender finishes; the receiver holds the whole file, undecided.
    pump_pair(&mut p.host, &mut p.client, "first held", |h, c| {
        h.active_transfers_snapshot()
            .iter()
            .any(|t| t.status == TransferStatus::Completed)
            && awaiting(c).iter().any(|t| t.received == t.size)
    })
    .await;

    p.host
        .send_file(p.host_chat, second)
        .await
        .expect("send second");
    pump_pair(&mut p.host, &mut p.client, "second held too", |_, c| {
        awaiting(c).iter().filter(|t| t.received == t.size).count() == 2
    })
    .await;

    for offer in awaiting(&p.client) {
        p.client.accept_incoming_file(offer.id).expect("accept");
    }
    pump_pair(&mut p.host, &mut p.client, "both saved", |_, c| {
        received_files(c, p.client_chat).len() == 2
    })
    .await;
    for (name, saved) in received_files(&p.client, p.client_chat) {
        let expected = match name.as_str() {
            "first.bin" => &first_bytes,
            "second.bin" => &second_bytes,
            other => panic!("unexpected file {other}"),
        };
        assert_eq!(
            &std::fs::read(saved).expect("read"),
            expected,
            "{name} corrupted"
        );
    }
}
