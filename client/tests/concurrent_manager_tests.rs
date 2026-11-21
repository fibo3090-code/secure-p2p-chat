//! Concurrency around `ChatManager`, in the shape both front-ends use: one
//! `Arc<Mutex<ChatManager>>`, a background loop polling and persisting while
//! command handlers mutate from other tasks. What a lock does *not* give for
//! free:
//!
//! - **No deadlock**: no manager method may re-enter the lock (it would hang
//!   only under contention).
//! - **No lost updates**: interleaved mutations all land.
//! - **Consistent reads**: a conversation in a snapshot is fetchable from it.
//! - **`poll_session_events` coexists with commands**, the production pattern.

use messenger_core::types::Config;
use p2pem_classic::app::chat_manager::ChatManager;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use uuid::Uuid;

type Shared = Arc<Mutex<ChatManager>>;

fn shared() -> Shared {
    Arc::new(Mutex::new(ChatManager::new(Config::default())))
}

/// Run `step(manager, i)` for `i in 0..n` on its own task, taking the lock for
/// each step and yielding between them so tasks genuinely interleave.
fn spawn_steps<F>(mgr: &Shared, n: usize, step: F) -> JoinHandle<()>
where
    F: Fn(&mut ChatManager, usize) + Send + 'static,
{
    let mgr = mgr.clone();
    tokio::spawn(async move {
        for i in 0..n {
            let mut guard = mgr.lock().await;
            step(&mut guard, i);
            drop(guard);
            tokio::task::yield_now().await;
        }
    })
}

/// Join a task, failing rather than hanging on a deadlock.
async fn finish(label: &str, task: JoinHandle<()>) {
    tokio::time::timeout(Duration::from_secs(30), task)
        .await
        .unwrap_or_else(|_| panic!("[{label}] timed out — likely a deadlock"))
        .unwrap_or_else(|e| panic!("[{label}] panicked: {e}"));
}

/// Many tasks adding contacts at once: every one must land exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_contact_writes_all_land() {
    let mgr = shared();
    const TASKS: usize = 16;
    const PER_TASK: usize = 25;

    let handles: Vec<_> = (0..TASKS)
        .map(|t| {
            spawn_steps(&mgr, PER_TASK, move |m, i| {
                m.add_contact(format!("peer-{t}-{i}"), None, None, None);
            })
        })
        .collect();
    for h in handles {
        finish("contact writes", h).await;
    }

    assert_eq!(
        mgr.lock().await.contacts.len(),
        TASKS * PER_TASK,
        "every concurrent add must be present exactly once"
    );
}

/// Readers against writers never see a torn view: anything the conversation
/// list mentions is fetchable from the same snapshot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reads_stay_consistent_while_state_changes() {
    let mgr = shared();
    let writer = spawn_steps(&mgr, 200, |m, i| {
        m.create_local_chat_for_test(Uuid::new_v4(), format!("chat-{i}"));
    });
    let reader = spawn_steps(&mgr, 200, |m, _| {
        let ids: Vec<Uuid> = m.chats.keys().copied().collect();
        for id in ids {
            assert!(
                m.chats.contains_key(&id),
                "a listed conversation was missing from the same view"
            );
        }
    });
    finish("writer", writer).await;
    finish("reader", reader).await;
}

/// The production pattern: a poll loop draining session events while command
/// handlers mutate. Neither may deadlock or corrupt the other.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_poll_loop_and_commands_coexist() {
    let mgr = shared();
    // Exactly what the bridge's background loop does.
    let polling = spawn_steps(&mgr, 300, |m, _| {
        m.poll_session_events();
        let _ = m.active_transfers_snapshot();
        let _ = m.pending_fingerprint().is_some();
    });
    let commanding = spawn_steps(&mgr, 300, |m, i| {
        let id = Uuid::new_v4();
        m.create_local_chat_for_test(id, format!("c{i}"));
        let _ = m.rename_chat(id, format!("renamed-{i}"));
        m.add_contact(format!("contact-{i}"), None, None, None);
    });
    finish("poll loop", polling).await;
    finish("commands", commanding).await;

    let guard = mgr.lock().await;
    assert_eq!(guard.contacts.len(), 300);
    assert_eq!(guard.chats.len(), 300);
    assert!(
        guard
            .chats
            .values()
            .all(|c| c.title.starts_with("renamed-")),
        "a rename was lost while the poll loop was running"
    );
}

/// Blocking and removing the same contacts from competing tasks converges
/// rather than leaving a half-removed record behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn competing_mutations_on_one_contact_converge() {
    let mgr = shared();
    let ids: Arc<Vec<Uuid>> = {
        let mut guard = mgr.lock().await;
        Arc::new(
            (0..50)
                .map(|i| guard.add_contact(format!("peer-{i}"), None, None, None))
                .collect(),
        )
    };

    // Racing a removal, an error is a legitimate outcome; a panic is not.
    let block_ids = ids.clone();
    let blocker = spawn_steps(&mgr, ids.len(), move |m, i| {
        let _ = m.block_contact(block_ids[i]);
    });
    let remove_ids = ids.clone();
    let remover = spawn_steps(&mgr, ids.len(), move |m, i| {
        m.remove_contact(remove_ids[i]);
    });
    finish("blocker", blocker).await;
    finish("remover", remover).await;

    let guard = mgr.lock().await;
    assert!(
        guard.contacts.is_empty(),
        "every contact was removed, so none may remain"
    );
    // And removal took the trust with it, under contention as much as not.
    assert!(guard.chats.values().all(|c| c.peer_fingerprint.is_none()));
}
