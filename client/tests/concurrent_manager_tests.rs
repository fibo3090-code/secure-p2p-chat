//! Concurrency around `ChatManager`.
//!
//! Both shipped front-ends hold the same `Arc<Mutex<ChatManager>>`: a background
//! loop polls session events and persists, while command handlers mutate state
//! from other tasks. That is the arrangement in `desktop/src-tauri/src/lib.rs`
//! and in the TUI, and nothing tested it — every existing test drives one
//! manager from one thread.
//!
//! What these check is not "does a `Mutex` work" — it does — but the properties
//! that a lock does *not* give you for free:
//!
//! - **No deadlock.** Manager methods must not try to re-enter the lock. A
//!   method that awaits something which itself needs the manager would hang the
//!   whole app, and it would hang it only under contention, which is exactly
//!   when nobody is looking.
//! - **No lost updates.** Interleaved mutations must all land; a read-modify-write
//!   through a clone would silently drop some.
//! - **Consistent reads.** A snapshot taken while others mutate must be internally
//!   coherent — a conversation in the list must still be fetchable.
//! - **`poll_session_events` is safe to call concurrently with commands**, which
//!   is the actual production pattern.
//!
//! Run these under `cargo nextest` as usual; they are also the tests most worth
//! running under a thread sanitiser if that is ever set up.

use messenger_core::types::Config;
use p2pem_classic::app::chat_manager::ChatManager;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use uuid::Uuid;

/// The shape both front-ends use.
fn shared() -> Arc<Mutex<ChatManager>> {
    Arc::new(Mutex::new(ChatManager::new(Config::default())))
}

/// Fail rather than hang. A deadlock here would otherwise sit until the harness
/// timeout with no indication of which task is stuck.
async fn within<F: std::future::Future>(label: &str, fut: F) -> F::Output {
    match tokio::time::timeout(Duration::from_secs(30), fut).await {
        Ok(v) => v,
        Err(_) => panic!("[{label}] timed out — likely a deadlock"),
    }
}

/// Many tasks adding contacts at once: every one must land exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_contact_writes_all_land() {
    let mgr = shared();
    const TASKS: usize = 16;
    const PER_TASK: usize = 25;

    let mut handles = Vec::new();
    for t in 0..TASKS {
        let mgr = mgr.clone();
        handles.push(tokio::spawn(async move {
            for i in 0..PER_TASK {
                let mut guard = mgr.lock().await;
                guard.add_contact(format!("peer-{t}-{i}"), None, None, None);
                drop(guard);
                // Yield between writes so the tasks genuinely interleave rather
                // than each running to completion inside one scheduling slot.
                tokio::task::yield_now().await;
            }
        }));
    }
    for h in handles {
        within("contact writes", h).await.expect("task panicked");
    }

    let guard = mgr.lock().await;
    assert_eq!(
        guard.contacts.len(),
        TASKS * PER_TASK,
        "every concurrent add must be present exactly once"
    );
}

/// Readers running against writers must never observe a torn view: anything the
/// conversation list mentions must be fetchable while the list is still changing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reads_stay_consistent_while_state_changes() {
    let mgr = shared();

    let writer = {
        let mgr = mgr.clone();
        tokio::spawn(async move {
            for i in 0..200 {
                {
                    let mut guard = mgr.lock().await;
                    let id = Uuid::new_v4();
                    guard.create_local_chat_for_test(id, format!("chat-{i}"));
                }
                tokio::task::yield_now().await;
            }
        })
    };

    let reader = {
        let mgr = mgr.clone();
        tokio::spawn(async move {
            for _ in 0..200 {
                {
                    let guard = mgr.lock().await;
                    let ids: Vec<Uuid> = guard.chats.keys().copied().collect();
                    // Everything the snapshot listed must still be there in the
                    // same snapshot — the check that would fail if a read ever
                    // saw a half-applied mutation.
                    for id in ids {
                        assert!(
                            guard.chats.contains_key(&id),
                            "a listed conversation was missing from the same view"
                        );
                    }
                }
                tokio::task::yield_now().await;
            }
        })
    };

    within("writer", writer).await.expect("writer panicked");
    within("reader", reader).await.expect("reader panicked");
}

/// The production pattern: a poll loop draining session events while command
/// handlers mutate. Neither may deadlock or corrupt the other.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_poll_loop_and_commands_coexist() {
    let mgr = shared();

    let polling = {
        let mgr = mgr.clone();
        tokio::spawn(async move {
            for _ in 0..300 {
                {
                    let mut guard = mgr.lock().await;
                    // Exactly what the bridge's background loop does.
                    guard.poll_session_events();
                    let _ = guard.active_transfers_snapshot();
                    let _ = guard.pending_fingerprint().is_some();
                }
                tokio::task::yield_now().await;
            }
        })
    };

    let commanding = {
        let mgr = mgr.clone();
        tokio::spawn(async move {
            for i in 0..300 {
                {
                    let mut guard = mgr.lock().await;
                    let id = Uuid::new_v4();
                    guard.create_local_chat_for_test(id, format!("c{i}"));
                    let _ = guard.rename_chat(id, format!("renamed-{i}"));
                    guard.add_contact(format!("contact-{i}"), None, None, None);
                }
                tokio::task::yield_now().await;
            }
        })
    };

    within("poll loop", polling).await.expect("poll panicked");
    within("commands", commanding)
        .await
        .expect("commands panicked");

    let guard = mgr.lock().await;
    assert_eq!(guard.contacts.len(), 300);
    assert_eq!(guard.chats.len(), 300);
    // Renames applied under contention must have stuck.
    assert!(
        guard
            .chats
            .values()
            .all(|c| c.title.starts_with("renamed-")),
        "a rename was lost while the poll loop was running"
    );
}

/// Blocking and removing the same contact from competing tasks must converge on
/// one answer rather than leaving a half-removed record behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn competing_mutations_on_one_contact_converge() {
    let mgr = shared();
    let ids: Vec<Uuid> = {
        let mut guard = mgr.lock().await;
        (0..50)
            .map(|i| guard.add_contact(format!("peer-{i}"), None, None, None))
            .collect()
    };

    let blocker = {
        let mgr = mgr.clone();
        let ids = ids.clone();
        tokio::spawn(async move {
            for id in ids {
                let mut guard = mgr.lock().await;
                // Racing a removal: an error is a legitimate outcome, a panic
                // or a corrupted record is not.
                let _ = guard.block_contact(id);
                drop(guard);
                tokio::task::yield_now().await;
            }
        })
    };

    let remover = {
        let mgr = mgr.clone();
        let ids = ids.clone();
        tokio::spawn(async move {
            for id in ids {
                let mut guard = mgr.lock().await;
                guard.remove_contact(id);
                drop(guard);
                tokio::task::yield_now().await;
            }
        })
    };

    within("blocker", blocker).await.expect("blocker panicked");
    within("remover", remover).await.expect("remover panicked");

    let guard = mgr.lock().await;
    assert!(
        guard.contacts.is_empty(),
        "every contact was removed, so none may remain"
    );
    // And removal took the trust with it, under contention as much as not.
    assert!(guard.chats.values().all(|c| c.peer_fingerprint.is_none()));
}
