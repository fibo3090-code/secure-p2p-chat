//! Reconnecting to a peer you have already verified, end to end: two real
//! `ChatManager`s over loopback, a verified conversation, a restart of the
//! dialing side, and a re-dial by address — the way both UIs reconnect.
//!
//! What must hold afterwards, on **both** sides:
//!
//! - **One conversation per peer.** The re-dial starts a fresh chat (nothing at
//!   dial time says who will answer); once the peer's verified identity arrives
//!   it has to be folded back into the conversation that holds the history.
//!   The host always did this; the dialing side used to leave a second, empty
//!   "host:port" thread beside the real one.
//! - **No second safety-code prompt** for a peer both sides already verified.
//! - **Messages flow both ways in the old conversation**, which also proves
//!   the replay counters were reset for the new session.

use messenger_core::core::generate_rsa_keypair;
use messenger_core::types::{Config, MessageContent};
use messenger_core::RSA_KEY_BITS;
use p2pem_classic::app::ChatManager;
use std::path::Path;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// A hang detector, not a performance bound: debug-build RSA handshakes are
/// slow, and several run at once under a parallel runner.
const STEP_TIMEOUT: Duration = Duration::from_secs(90);

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    listener.local_addr().expect("local addr").port()
}

fn manager_in(dir: &Path, history_key: [u8; 32]) -> ChatManager {
    let download = dir.join("downloads");
    let temp = dir.join("tmp");
    std::fs::create_dir_all(&download).expect("download dir");
    std::fs::create_dir_all(&temp).expect("temp dir");
    let mut manager = ChatManager::new(Config {
        download_dir: download,
        temp_dir: temp,
        ..Config::default()
    });
    manager.set_history_key(history_key);
    manager
}

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
        assert!(Instant::now() < deadline, "[{label}] timed out");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Conversations, not listeners.
fn conversations(m: &ChatManager) -> Vec<Uuid> {
    m.chats
        .values()
        .filter(|c| !c.is_host_placeholder)
        .map(|c| c.id)
        .collect()
}

fn texts(m: &ChatManager, chat: Uuid) -> Vec<String> {
    m.chats[&chat]
        .messages
        .iter()
        .filter_map(|msg| match &msg.content {
            MessageContent::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redialing_a_verified_peer_continues_the_same_conversation() {
    let host_dir = tempfile::tempdir().expect("host dir");
    let client_dir = tempfile::tempdir().expect("client dir");
    let client_history = client_dir.path().join("history.json.enc");
    let client_history_key = [7u8; 32];
    let mut host = manager_in(host_dir.path(), [9u8; 32]);
    let mut client = manager_in(client_dir.path(), client_history_key);
    let host_key = generate_rsa_keypair(RSA_KEY_BITS).expect("host key");
    let client_key = generate_rsa_keypair(RSA_KEY_BITS).expect("client key");
    let port = free_port();

    // --- First contact: connect and verify on both ends. ---
    host.start_host(port, host_key.clone()).await.expect("host");
    let client_chat = client
        .connect_to_host("127.0.0.1", port, None, client_key.clone())
        .await
        .expect("dial");
    pump_until(&mut host, &mut client, "first prompts", |h, c| {
        h.pending_fingerprint().is_some() && c.pending_fingerprint().is_some()
    })
    .await;
    let prompt = host.pending_fingerprint().unwrap().session_id;
    host.confirm_fingerprint(prompt, true)
        .expect("host accepts");
    let prompt = client.pending_fingerprint().unwrap().session_id;
    client
        .confirm_fingerprint(prompt, true)
        .expect("client accepts");

    let mut host_chat = None;
    pump_until(&mut host, &mut client, "first session", |h, c| {
        host_chat = conversations(h).into_iter().find(|id| h.is_connected(id));
        host_chat.is_some() && c.is_connected(&client_chat)
    })
    .await;
    let host_chat = host_chat.unwrap();
    client
        .send_message(client_chat, "before".to_string())
        .expect("send before");
    pump_until(&mut host, &mut client, "first message", |h, _| {
        texts(h, host_chat) == ["before"]
    })
    .await;

    // --- The dialing side restarts. ---
    client.save_history(&client_history).expect("save");
    drop(client);
    // Dropping the manager must close its sessions, or the host would not
    // notice until its idle timeout.
    pump_until(
        &mut host,
        &mut ChatManager::default(),
        "host sees the drop",
        |h, _| !h.is_connected(&host_chat),
    )
    .await;
    let mut client = manager_in(client_dir.path(), client_history_key);
    client
        .load_history_auto(&client_history, &client_history_key)
        .expect("load");
    assert_eq!(conversations(&client), vec![client_chat]);

    // The host's listener was consumed by the first peer; re-open it the way
    // both shells' auto-rehost does.
    assert!(host.check_rehost_needed());
    host.start_host(port, host_key).await.expect("rehost");

    // --- Re-dial by address, exactly as the UIs do (no existing chat id). ---
    let redial = client
        .connect_to_host("127.0.0.1", port, None, client_key)
        .await
        .expect("redial");
    assert_ne!(redial, client_chat, "a dial by address starts a fresh chat");
    pump_until(&mut host, &mut client, "second session", |h, c| {
        h.is_connected(&host_chat) && c.is_connected(&client_chat)
    })
    .await;

    assert!(
        host.pending_fingerprint().is_none() && client.pending_fingerprint().is_none(),
        "a peer both sides already verified must not be prompted again"
    );
    assert_eq!(
        conversations(&client),
        vec![client_chat],
        "the re-dial left a second conversation beside the one with the history"
    );
    assert_eq!(conversations(&host), vec![host_chat]);
    assert_eq!(client.resolve_display_chat_id(redial), client_chat);

    // Both directions work in the original conversation.
    client
        .send_message(client_chat, "after".to_string())
        .expect("client sends after reconnect");
    host.send_message(host_chat, "reply".to_string())
        .expect("host sends after reconnect");
    // Each side lists its own message first, so compare without order.
    let sorted = |mut v: Vec<String>| {
        v.sort();
        v
    };
    pump_until(
        &mut host,
        &mut client,
        "messages after reconnect",
        |h, c| {
            sorted(texts(h, host_chat)) == ["after", "before", "reply"]
                && sorted(texts(c, client_chat)) == ["after", "before", "reply"]
        },
    )
    .await;
}
