//! End-to-end tests driving the real v3 session functions over an in-memory
//! duplex stream: version exchange → X25519 → key derivation → encrypted
//! identity proof → TOFU → encrypted transport → disconnect.

use messenger_core::core::{generate_rsa_keypair, ProtocolMessage};
use messenger_core::network::{run_client_session_over_stream, run_host_session_over_stream};
use messenger_core::types::SessionEvent;
use messenger_core::RSA_KEY_BITS;
use std::time::Duration;
use tokio::sync::mpsc;

type Events = mpsc::UnboundedReceiver<SessionEvent>;
type Outbound = mpsc::UnboundedSender<ProtocolMessage>;
type SessionTask = tokio::task::JoinHandle<anyhow::Result<()>>;

const STEP_TIMEOUT: Duration = Duration::from_secs(20);

/// The host's `NewConnection` or the client's `ShowFingerprintVerification`.
fn awaiting_confirmation(ev: &SessionEvent) -> bool {
    matches!(
        ev,
        SessionEvent::NewConnection { .. } | SessionEvent::ShowFingerprintVerification { .. }
    )
}

fn is_ready(ev: &SessionEvent) -> bool {
    matches!(ev, SessionEvent::Ready)
}

fn is_error(ev: &SessionEvent) -> bool {
    matches!(ev, SessionEvent::Error(_))
}

/// Pump `rx` until an event matching `pred` arrives; fails instead of hanging.
async fn wait_until<F>(rx: &mut Events, label: &str, mut pred: F) -> SessionEvent
where
    F: FnMut(&SessionEvent) -> bool,
{
    let deadline = tokio::time::Instant::now() + STEP_TIMEOUT;
    loop {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .unwrap_or_default();
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some(ev)) if pred(&ev) => return ev,
            Ok(Some(_)) => {}
            Ok(None) => panic!("[{label}] event channel closed before match"),
            Err(_) => panic!("[{label}] timed out waiting for event"),
        }
    }
}

async fn next_message(rx: &mut Events, label: &str) -> ProtocolMessage {
    match wait_until(rx, label, |ev| {
        matches!(ev, SessionEvent::MessageReceived(_))
    })
    .await
    {
        SessionEvent::MessageReceived(m) => m,
        _ => unreachable!(),
    }
}

async fn next_text(rx: &mut Events, label: &str) -> String {
    match next_message(rx, label).await {
        ProtocolMessage::Text { text, .. } => text,
        other => panic!("[{label}] expected Text, got {other:?}"),
    }
}

async fn wait_for_error(rx: &mut Events, label: &str) -> String {
    match wait_until(rx, label, is_error).await {
        SessionEvent::Error(msg) => msg,
        _ => unreachable!(),
    }
}

/// The session task's result, which must arrive within the step timeout.
async fn finished(handle: SessionTask, what: &str) -> anyhow::Result<()> {
    tokio::time::timeout(STEP_TIMEOUT, handle)
        .await
        .unwrap_or_else(|_| panic!("{what} task should finish"))
        .unwrap()
}

fn text(text: &str, timestamp: u64, seq: u64) -> ProtocolMessage {
    ProtocolMessage::Text {
        text: text.to_string(),
        timestamp,
        seq,
    }
}

struct Peer {
    events: Events,
    outbound: Outbound,
    confirm: mpsc::UnboundedSender<bool>,
    handle: SessionTask,
    /// Kept alive: dropping the file lane would close the loop's file branch.
    _file_tx: mpsc::Sender<ProtocolMessage>,
}

impl Peer {
    fn send(&self, msg: ProtocolMessage) {
        self.outbound.send(msg).unwrap();
    }
}

async fn connect_pair() -> (Peer, Peer) {
    connect_pair_with_passwords(None, None).await
}

/// A host+client pair over a duplex stream, with optional host-required and
/// client-supplied connection passwords.
async fn connect_pair_with_passwords(
    host_password: Option<String>,
    client_password: Option<String>,
) -> (Peer, Peer) {
    let (host_stream, client_stream) = tokio::io::duplex(1 << 16);
    (
        spawn_peer(host_stream, true, host_password),
        spawn_peer(client_stream, false, client_password),
    )
}

fn spawn_peer(mut stream: tokio::io::DuplexStream, host: bool, password: Option<String>) -> Peer {
    let key = generate_rsa_keypair(RSA_KEY_BITS).expect("key");
    let (ev_tx, events) = mpsc::unbounded_channel();
    let (outbound, out_rx) = mpsc::unbounded_channel();
    let (file_tx, file_rx) = mpsc::channel(8);
    let (confirm, confirm_rx) = mpsc::unbounded_channel();
    let chat = uuid::Uuid::new_v4();

    let handle = tokio::spawn(async move {
        if host {
            run_host_session_over_stream(
                &mut stream,
                "client-peer".to_string(),
                key,
                ev_tx,
                out_rx,
                file_rx,
                confirm_rx,
                chat,
                password,
            )
            .await
        } else {
            run_client_session_over_stream(
                &mut stream,
                "host-peer".to_string(),
                key,
                ev_tx,
                out_rx,
                file_rx,
                confirm_rx,
                chat,
                password,
            )
            .await
        }
    });
    Peer {
        events,
        outbound,
        confirm,
        handle,
        _file_tx: file_tx,
    }
}

/// Drive a freshly connected pair through TOFU confirmation to Ready.
async fn reach_ready(host: &mut Peer, client: &mut Peer) {
    wait_until(&mut host.events, "host", awaiting_confirmation).await;
    wait_until(&mut client.events, "client", awaiting_confirmation).await;
    host.confirm.send(true).unwrap();
    client.confirm.send(true).unwrap();
    wait_until(&mut host.events, "host", is_ready).await;
    wait_until(&mut client.events, "client", is_ready).await;
}

#[tokio::test]
async fn full_pipeline_handshake_messages_typing_file_and_disconnect() {
    let (mut host, mut client) = connect_pair().await;
    reach_ready(&mut host, &mut client).await;

    host.send(text("hello from host", 1, 1));
    assert_eq!(
        next_text(&mut client.events, "client").await,
        "hello from host"
    );
    client.send(text("hi back from client", 2, 1));
    assert_eq!(
        next_text(&mut host.events, "host").await,
        "hi back from client"
    );

    host.send(ProtocolMessage::TypingStart { seq: 2 });
    assert!(matches!(
        next_message(&mut client.events, "client").await,
        ProtocolMessage::TypingStart { .. }
    ));
    host.send(ProtocolMessage::TypingStop { seq: 3 });
    assert!(matches!(
        next_message(&mut client.events, "client").await,
        ProtocolMessage::TypingStop { .. }
    ));

    // File transfer (meta → chunk → end), sharing the per-session seq space.
    let payload = vec![0xABu8; 4096];
    host.send(ProtocolMessage::FileMeta {
        filename: "photo.png".to_string(),
        size: payload.len() as u64,
        seq: 4,
    });
    match next_message(&mut client.events, "client").await {
        ProtocolMessage::FileMeta { filename, size, .. } => {
            assert_eq!(filename, "photo.png");
            assert_eq!(size, payload.len() as u64);
        }
        other => panic!("expected FileMeta, got {other:?}"),
    }
    host.send(ProtocolMessage::FileChunk {
        chunk: payload.clone(),
        seq: 5,
    });
    match next_message(&mut client.events, "client").await {
        ProtocolMessage::FileChunk { chunk, .. } => assert_eq!(chunk, payload),
        other => panic!("expected FileChunk, got {other:?}"),
    }
    host.send(ProtocolMessage::FileEnd { seq: 6 });
    assert!(matches!(
        next_message(&mut client.events, "client").await,
        ProtocolMessage::FileEnd { .. }
    ));

    // The sender learns when the final frame hit the wire (queueing is not
    // delivery). The event's seq is the transport's, so only the variant counts.
    wait_until(&mut host.events, "host", |ev| {
        matches!(ev, SessionEvent::FileSendComplete { .. })
    })
    .await;

    // FileCancel traverses the transport and the replay check like any frame.
    host.send(ProtocolMessage::FileCancel { seq: 7 });
    assert!(matches!(
        next_message(&mut client.events, "client").await,
        ProtocolMessage::FileCancel { .. }
    ));

    // A Ping is consumed by the transport: the next app-visible message is the
    // following Text.
    host.send(ProtocolMessage::Ping { seq: 7 });
    host.send(text("after-ping", 99, 8));
    assert_eq!(next_text(&mut client.events, "client").await, "after-ping");

    // Tearing down the client makes the host observe the drop.
    client.handle.abort();
    drop(client.outbound);
    wait_until(&mut host.events, "host", |ev| {
        matches!(ev, SessionEvent::Disconnected | SessionEvent::Error(_))
    })
    .await;

    host.handle.abort();
}

#[tokio::test]
async fn both_peers_derive_the_same_short_authentication_string() {
    let (mut host, mut client) = connect_pair().await;

    // Both roles derive the SAS independently from the transcript (a MITM
    // would run two handshakes and the codes would differ).
    let host_sas = match wait_until(&mut host.events, "host", awaiting_confirmation).await {
        SessionEvent::NewConnection { sas, .. } => sas,
        other => panic!("host expected NewConnection, got {other:?}"),
    };
    let client_sas = match wait_until(&mut client.events, "client", awaiting_confirmation).await {
        SessionEvent::ShowFingerprintVerification { sas, .. } => sas,
        other => panic!("client expected ShowFingerprintVerification, got {other:?}"),
    };

    assert_eq!(host_sas, client_sas, "both ends must show the same SAS");
    assert!(!host_sas.is_empty(), "SAS must be populated");
    // "NN-NN-NN" digits followed by three emoji groups.
    assert_eq!(
        host_sas.split(' ').count(),
        4,
        "unexpected SAS shape: {host_sas}"
    );

    host.confirm.send(true).unwrap();
    client.confirm.send(true).unwrap();
    host.handle.abort();
    client.handle.abort();
}

#[tokio::test]
async fn rejecting_the_fingerprint_aborts_the_session() {
    let (mut host, mut client) = connect_pair().await;

    wait_until(&mut host.events, "host", awaiting_confirmation).await;
    wait_until(&mut client.events, "client", awaiting_confirmation).await;

    // Host rejects, client accepts: the host must error out, never Ready.
    host.confirm.send(false).unwrap();
    client.confirm.send(true).unwrap();

    let msg = wait_for_error(&mut host.events, "host").await;
    assert!(msg.to_lowercase().contains("reject"));
    assert!(
        finished(host.handle, "host").await.is_err(),
        "host session must return an error after a rejected fingerprint"
    );

    client.handle.abort();
}

#[tokio::test]
async fn correct_connection_password_is_accepted() {
    let (mut host, mut client) =
        connect_pair_with_passwords(Some("hunter2".to_string()), Some("hunter2".to_string())).await;

    // The password gate runs before TOFU; both sides then reach Ready.
    reach_ready(&mut host, &mut client).await;

    host.handle.abort();
    client.handle.abort();
}

#[tokio::test]
async fn wrong_connection_password_is_rejected_before_tofu() {
    let (mut host, client) =
        connect_pair_with_passwords(Some("hunter2".to_string()), Some("wrong".to_string())).await;

    // The host rejects at the password gate, never surfacing the peer for TOFU.
    let msg = wait_for_error(&mut host.events, "host").await;
    assert!(msg.to_lowercase().contains("password"), "got: {msg}");
    assert!(finished(host.handle, "host").await.is_err());

    client.handle.abort();
}

#[tokio::test]
async fn missing_connection_password_is_rejected() {
    let (mut host, client) = connect_pair_with_passwords(Some("hunter2".to_string()), None).await;

    wait_for_error(&mut host.events, "host").await;
    assert!(finished(host.handle, "host").await.is_err());
    // The client learned a password was required but had none.
    assert!(finished(client.handle, "client").await.is_err());
}

/// Regression for the 5-minute idle disconnect: with keep-alive pings an idle
/// session must outlive the idle window. The env hooks shrink it to a 1 s
/// keep-alive against a 3 s idle timeout, then 6 s of silence.
#[tokio::test]
async fn idle_session_survives_past_the_idle_timeout() {
    // Read at message-loop startup, so set before the pair connects, and
    // removed when the test ends — under nextest each test is its own process,
    // but under plain `cargo test` the rest of this binary would inherit a 3 s
    // idle timeout.
    struct ShortTimers;
    impl Drop for ShortTimers {
        fn drop(&mut self) {
            std::env::remove_var("P2PEM_TEST_KEEPALIVE_SECS");
            std::env::remove_var("P2PEM_TEST_IDLE_TIMEOUT_SECS");
        }
    }
    let _restore = ShortTimers;
    std::env::set_var("P2PEM_TEST_KEEPALIVE_SECS", "1");
    std::env::set_var("P2PEM_TEST_IDLE_TIMEOUT_SECS", "3");

    let (mut host, mut client) = connect_pair().await;
    reach_ready(&mut host, &mut client).await;

    tokio::time::sleep(std::time::Duration::from_secs(6)).await;

    // Still alive in both directions (a fired idle timer would surface as
    // Error/Disconnected and fail next_text).
    host.send(text("still here", 1, 1));
    assert_eq!(next_text(&mut client.events, "client").await, "still here");
    client.send(text("me too", 2, 1));
    assert_eq!(next_text(&mut host.events, "host").await, "me too");

    host.handle.abort();
    client.handle.abort();
}

/// NET-10: the two people compare the safety code at their own pace. The
/// first to accept enters its message loop and its idle timer starts, while
/// the other is still reading the prompt and sends nothing — so confirming
/// more than the idle window apart (five minutes in production, comparing a
/// code over the phone) used to end the session on "Receive idle timeout"
/// before the second side had even answered. Shrunk here to a 3 s window
/// and a 6 s gap with the debug-only timer hooks.
#[tokio::test]
async fn confirming_the_safety_code_minutes_apart_keeps_the_session() {
    struct ShortTimers;
    impl Drop for ShortTimers {
        fn drop(&mut self) {
            std::env::remove_var("P2PEM_TEST_KEEPALIVE_SECS");
            std::env::remove_var("P2PEM_TEST_IDLE_TIMEOUT_SECS");
        }
    }
    let _restore = ShortTimers;
    std::env::set_var("P2PEM_TEST_KEEPALIVE_SECS", "1");
    std::env::set_var("P2PEM_TEST_IDLE_TIMEOUT_SECS", "3");

    let (mut host, mut client) = connect_pair().await;
    wait_until(&mut host.events, "host prompt", awaiting_confirmation).await;
    wait_until(&mut client.events, "client prompt", awaiting_confirmation).await;

    host.confirm.send(true).unwrap();
    // Twice the idle window before the other side answers.
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;
    client.confirm.send(true).unwrap();

    let text = |t: &str| ProtocolMessage::Text {
        text: t.into(),
        timestamp: 0,
        seq: 0,
    };
    host.outbound.send(text("to client")).unwrap();
    client.outbound.send(text("to host")).unwrap();
    for (peer, label) in [(&mut client, "client"), (&mut host, "host")] {
        match next_message(&mut peer.events, label).await {
            ProtocolMessage::Text { text, .. } => assert_eq!(text, format!("to {label}")),
            other => panic!("{label} expected text, got {other:?}"),
        }
    }
    host.handle.abort();
    client.handle.abort();
}

/// Crossing `REKEY_MESSAGE_COUNT` (100) mid-stream must still deliver every
/// message, in order.
#[tokio::test]
async fn messages_keep_flowing_across_a_rekey() {
    let (mut host, mut client) = connect_pair().await;
    reach_ready(&mut host, &mut client).await;

    const COUNT: usize = 130;
    for i in 0..COUNT {
        host.send(text(&format!("msg-{i}"), i as u64, (i + 1) as u64));
    }
    for i in 0..COUNT {
        assert_eq!(
            next_text(&mut client.events, "client").await,
            format!("msg-{i}")
        );
    }

    host.handle.abort();
    client.handle.abort();
}

/// CRYPTO-06: several rekeys under traffic crossing in both directions.
///
/// Only the host initiates a rekey, and only one previous key is retained to
/// decrypt frames the client sent before it switched. Both directions count
/// toward the threshold, so with the client streaming too the host can start
/// a *second* rekey before any frame under the first has come back — which
/// discards the key the client's in-flight frames still use, and the session
/// dies on "Decryption failed". One-way tests cannot see this: with one
/// direction quiet, every rekey completes before the next is due.
#[tokio::test]
async fn several_rekeys_under_crossing_traffic_lose_nothing() {
    let (mut host, mut client) = connect_pair().await;
    reach_ready(&mut host, &mut client).await;

    // ~1000 frames against a 100-message threshold: about ten rekeys.
    const COUNT: usize = 500;
    for i in 0..COUNT {
        for (peer, side) in [(&host, "host"), (&client, "client")] {
            peer.outbound
                .send(ProtocolMessage::Text {
                    text: format!("{side}-{i}"),
                    timestamp: i as u64,
                    seq: 0,
                })
                .unwrap();
        }
    }
    for (peer, from, label) in [
        (&mut client, "host", "client"),
        (&mut host, "client", "host"),
    ] {
        for i in 0..COUNT {
            match next_message(&mut peer.events, label).await {
                ProtocolMessage::Text { text, .. } => {
                    assert_eq!(
                        text,
                        format!("{from}-{i}"),
                        "{label} lost or reordered a frame"
                    )
                }
                other => panic!("{label} expected text {i}, got {other:?}"),
            }
        }
    }

    host.handle.abort();
    client.handle.abort();
}

/// Regression for the non-cancel-safe receive: a send winning the select while
/// a frame was half-read threw the consumed bytes away and desynced framing.
/// Needs both peers sending near-cap frames through a 64 KiB duplex at once.
#[tokio::test]
async fn simultaneous_traffic_in_both_directions_keeps_framing_intact() {
    let (mut host, mut client) = connect_pair().await;
    reach_ready(&mut host, &mut client).await;

    const COUNT: usize = 60;
    let body = |side: &str, i: usize| format!("{side}-{i}-{}", "x".repeat(48 * 1024));
    for i in 0..COUNT {
        for (peer, side) in [(&host, "host"), (&client, "client")] {
            peer.send(text(&body(side, i), i as u64, 0));
        }
    }

    for (peer, from, label) in [
        (&mut client, "host", "client"),
        (&mut host, "client", "host"),
    ] {
        for i in 0..COUNT {
            assert!(
                next_text(&mut peer.events, label).await == body(from, i),
                "{label} received a corrupted or out-of-order frame at {i}"
            );
        }
    }

    host.handle.abort();
    client.handle.abort();
}

/// NET-02: the host listener used to be one-shot, so a silent connection
/// closed the port. A real peer must get through while such connections idle.
#[tokio::test]
async fn silent_connections_do_not_lock_a_real_peer_out_of_the_host() {
    use messenger_core::network::{bind_host_listener, run_client_session, run_host_session};

    let listener = bind_host_listener(0).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let host_key = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
    let client_key = generate_rsa_keypair(RSA_KEY_BITS).unwrap();

    let (host_tx, mut host_events) = mpsc::unbounded_channel();
    let (_host_out, host_out_rx) = mpsc::unbounded_channel();
    let (_host_file, host_file_rx) = mpsc::channel(4);
    let (_host_confirm, host_confirm_rx) = mpsc::unbounded_channel();
    let host = tokio::spawn(run_host_session(
        listener,
        host_key,
        host_tx,
        host_out_rx,
        host_file_rx,
        host_confirm_rx,
        uuid::Uuid::new_v4(),
        None,
    ));
    wait_until(&mut host_events, "listening", |e| {
        matches!(e, SessionEvent::Listening { .. })
    })
    .await;

    // Two connections that never speak (under the per-IP rate limit).
    let _idle_a = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let _idle_b = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();

    let (client_tx, _client_events) = mpsc::unbounded_channel();
    let (_client_out, client_out_rx) = mpsc::unbounded_channel();
    let (_client_file, client_file_rx) = mpsc::channel(4);
    let (_client_confirm, client_confirm_rx) = mpsc::unbounded_channel();
    let client = tokio::spawn(run_client_session(
        "127.0.0.1",
        port,
        client_key,
        client_tx,
        client_out_rx,
        client_file_rx,
        client_confirm_rx,
        uuid::Uuid::new_v4(),
        None,
    ));

    // Well inside the 15 s an idle connection used to hold the port for.
    let ev = tokio::time::timeout(
        Duration::from_secs(10),
        wait_until(&mut host_events, "real peer", |e| {
            matches!(e, SessionEvent::NewConnection { .. })
        }),
    )
    .await
    .expect("the real peer was locked out by idle connections");
    assert!(matches!(ev, SessionEvent::NewConnection { .. }));
    host.abort();
    client.abort();
}
