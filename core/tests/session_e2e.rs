//! End-to-end pipeline tests driving the real Protocol v3 session functions
//! (`run_host_session_over_stream` / `run_client_session_over_stream`) over an
//! in-memory duplex stream — the full A-to-Z path: version exchange → X25519
//! ephemeral key exchange → session-key derivation → encrypted identity proof →
//! TOFU fingerprint confirmation → encrypted transport (text, typing, file
//! transfer, ping) → disconnect.

use messenger_core::core::{generate_rsa_keypair, ProtocolMessage};
use messenger_core::network::{run_client_session_over_stream, run_host_session_over_stream};
use messenger_core::types::SessionEvent;
use messenger_core::RSA_KEY_BITS;
use std::time::Duration;
use tokio::sync::mpsc;

type Events = mpsc::UnboundedReceiver<SessionEvent>;
type Outbound = mpsc::UnboundedSender<ProtocolMessage>;

const STEP_TIMEOUT: Duration = Duration::from_secs(20);

/// Both handshake roles pause for TOFU confirmation, but emit different events:
/// the host emits `NewConnection`, the client emits `ShowFingerprintVerification`.
fn awaiting_confirmation(ev: &SessionEvent) -> bool {
    matches!(
        ev,
        SessionEvent::NewConnection { .. } | SessionEvent::ShowFingerprintVerification { .. }
    )
}

/// Pump `rx` until an event matching `pred` arrives, returning it. Fails (rather
/// than hanging) if no matching event arrives within the timeout.
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
            Ok(Some(ev)) => {
                if pred(&ev) {
                    return ev;
                }
                // Otherwise keep draining (e.g. skip intermediate handshake events).
            }
            Ok(None) => panic!("[{label}] event channel closed before match"),
            Err(_) => panic!("[{label}] timed out waiting for event"),
        }
    }
}

/// Wait for the next `MessageReceived` and return the contained ProtocolMessage.
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

struct Peer {
    events: Events,
    outbound: Outbound,
    confirm: mpsc::UnboundedSender<bool>,
    handle: tokio::task::JoinHandle<anyhow::Result<()>>,
}

/// Spin up a connected host+client pair over a duplex stream and run both
/// sessions through the handshake. Returns both peers ready for the confirm step.
async fn connect_pair() -> (Peer, Peer) {
    connect_pair_with_passwords(None, None).await
}

/// As [`connect_pair`], but with optional host-required / client-supplied
/// connection passwords to exercise the password gate.
async fn connect_pair_with_passwords(
    host_password: Option<String>,
    client_password: Option<String>,
) -> (Peer, Peer) {
    let host_priv = generate_rsa_keypair(RSA_KEY_BITS).expect("host key");
    let client_priv = generate_rsa_keypair(RSA_KEY_BITS).expect("client key");
    let (mut host_stream, mut client_stream) = tokio::io::duplex(1 << 16);

    let (host_ev_tx, host_events) = mpsc::unbounded_channel();
    let (host_out, host_out_rx) = mpsc::unbounded_channel();
    let (host_confirm, host_confirm_rx) = mpsc::unbounded_channel();
    let host_chat = uuid::Uuid::new_v4();

    let (client_ev_tx, client_events) = mpsc::unbounded_channel();
    let (client_out, client_out_rx) = mpsc::unbounded_channel();
    let (client_confirm, client_confirm_rx) = mpsc::unbounded_channel();
    let client_chat = uuid::Uuid::new_v4();

    let host_handle = tokio::spawn(async move {
        run_host_session_over_stream(
            &mut host_stream,
            "client-peer".to_string(),
            host_priv,
            host_ev_tx,
            host_out_rx,
            host_confirm_rx,
            host_chat,
            host_password,
        )
        .await
    });

    let client_handle = tokio::spawn(async move {
        run_client_session_over_stream(
            &mut client_stream,
            "host-peer".to_string(),
            client_priv,
            client_ev_tx,
            client_out_rx,
            client_confirm_rx,
            client_chat,
            client_password,
        )
        .await
    });

    (
        Peer {
            events: host_events,
            outbound: host_out,
            confirm: host_confirm,
            handle: host_handle,
        },
        Peer {
            events: client_events,
            outbound: client_out,
            confirm: client_confirm,
            handle: client_handle,
        },
    )
}

#[tokio::test]
async fn full_pipeline_handshake_messages_typing_file_and_disconnect() {
    let (mut host, mut client) = connect_pair().await;

    // TOFU: both sides surface the peer's fingerprint and wait for confirmation.
    wait_until(&mut host.events, "host", awaiting_confirmation).await;
    wait_until(&mut client.events, "client", awaiting_confirmation).await;

    // Accept on both ends.
    host.confirm.send(true).unwrap();
    client.confirm.send(true).unwrap();

    // Both transition to Ready (encrypted tunnel established).
    wait_until(&mut host.events, "host", |ev| {
        matches!(ev, SessionEvent::Ready)
    })
    .await;
    wait_until(&mut client.events, "client", |ev| {
        matches!(ev, SessionEvent::Ready)
    })
    .await;

    // --- Bidirectional text ---
    host.outbound
        .send(ProtocolMessage::Text {
            text: "hello from host".to_string(),
            timestamp: 1,
            seq: 1,
        })
        .unwrap();
    match next_message(&mut client.events, "client").await {
        ProtocolMessage::Text { text, .. } => assert_eq!(text, "hello from host"),
        other => panic!("expected Text, got {other:?}"),
    }

    client
        .outbound
        .send(ProtocolMessage::Text {
            text: "hi back from client".to_string(),
            timestamp: 2,
            seq: 1,
        })
        .unwrap();
    match next_message(&mut host.events, "host").await {
        ProtocolMessage::Text { text, .. } => assert_eq!(text, "hi back from client"),
        other => panic!("expected Text, got {other:?}"),
    }

    // --- Typing indicators ---
    host.outbound
        .send(ProtocolMessage::TypingStart { seq: 2 })
        .unwrap();
    assert!(matches!(
        next_message(&mut client.events, "client").await,
        ProtocolMessage::TypingStart { .. }
    ));
    host.outbound
        .send(ProtocolMessage::TypingStop { seq: 3 })
        .unwrap();
    assert!(matches!(
        next_message(&mut client.events, "client").await,
        ProtocolMessage::TypingStop { .. }
    ));

    // --- File transfer (meta → chunk → end), sharing the per-session seq space ---
    let payload = vec![0xABu8; 4096];
    host.outbound
        .send(ProtocolMessage::FileMeta {
            filename: "photo.png".to_string(),
            size: payload.len() as u64,
            seq: 4,
        })
        .unwrap();
    match next_message(&mut client.events, "client").await {
        ProtocolMessage::FileMeta { filename, size, .. } => {
            assert_eq!(filename, "photo.png");
            assert_eq!(size, payload.len() as u64);
        }
        other => panic!("expected FileMeta, got {other:?}"),
    }
    host.outbound
        .send(ProtocolMessage::FileChunk {
            chunk: payload.clone(),
            seq: 5,
        })
        .unwrap();
    match next_message(&mut client.events, "client").await {
        ProtocolMessage::FileChunk { chunk, .. } => assert_eq!(chunk, payload),
        other => panic!("expected FileChunk, got {other:?}"),
    }
    host.outbound
        .send(ProtocolMessage::FileEnd { seq: 6 })
        .unwrap();
    assert!(matches!(
        next_message(&mut client.events, "client").await,
        ProtocolMessage::FileEnd { .. }
    ));

    // The SENDER must be told when the file's final frame actually hit the
    // wire — queueing is not delivery. (The seq in the event is the transport
    // sequence the loop stamped, not the app-supplied one, so only the variant
    // is asserted.)
    wait_until(&mut host.events, "host", |ev| {
        matches!(ev, SessionEvent::FileSendComplete { .. })
    })
    .await;

    // --- Keep-alive ping: transport plumbing, consumed silently ---
    // A Ping must keep the session healthy but never surface to the app: the
    // very next app-visible message after it is the following Text, not the Ping.
    host.outbound
        .send(ProtocolMessage::Ping { seq: 7 })
        .unwrap();
    host.outbound
        .send(ProtocolMessage::Text {
            text: "after-ping".into(),
            timestamp: 99,
            seq: 8,
        })
        .unwrap();
    match next_message(&mut client.events, "client").await {
        ProtocolMessage::Text { text, .. } => assert_eq!(text, "after-ping"),
        other => panic!("ping must be consumed by the transport, got {other:?}"),
    }

    // --- Disconnect: tearing down the client makes the host observe the drop ---
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

    // The SAS is carried on the confirmation-request events; both roles derive
    // it independently from the shared transcript, so the two must be equal and
    // well-formed. (A MITM would run two handshakes and the codes would differ.)
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

    // Host rejects; client accepts. The host session must error out and never
    // reach Ready.
    host.confirm.send(false).unwrap();
    client.confirm.send(true).unwrap();

    let ev = wait_until(&mut host.events, "host", |ev| {
        matches!(ev, SessionEvent::Error(_))
    })
    .await;
    match ev {
        SessionEvent::Error(msg) => assert!(msg.to_lowercase().contains("reject")),
        _ => unreachable!(),
    }

    let result = tokio::time::timeout(STEP_TIMEOUT, host.handle)
        .await
        .expect("host task should finish after rejection");
    assert!(
        result.unwrap().is_err(),
        "host session must return an error after a rejected fingerprint"
    );

    client.handle.abort();
}

#[tokio::test]
async fn correct_connection_password_is_accepted() {
    let (mut host, mut client) =
        connect_pair_with_passwords(Some("hunter2".to_string()), Some("hunter2".to_string())).await;

    // The password gate runs before TOFU; with the right password both sides
    // proceed to the confirmation step and then to Ready.
    wait_until(&mut host.events, "host", awaiting_confirmation).await;
    wait_until(&mut client.events, "client", awaiting_confirmation).await;
    host.confirm.send(true).unwrap();
    client.confirm.send(true).unwrap();
    wait_until(&mut host.events, "host", |ev| {
        matches!(ev, SessionEvent::Ready)
    })
    .await;
    wait_until(&mut client.events, "client", |ev| {
        matches!(ev, SessionEvent::Ready)
    })
    .await;

    host.handle.abort();
    client.handle.abort();
}

#[tokio::test]
async fn wrong_connection_password_is_rejected_before_tofu() {
    let (mut host, client) =
        connect_pair_with_passwords(Some("hunter2".to_string()), Some("wrong".to_string())).await;

    // The host rejects at the password gate and never surfaces the peer for TOFU.
    let ev = wait_until(&mut host.events, "host", |ev| {
        matches!(ev, SessionEvent::Error(_))
    })
    .await;
    match ev {
        SessionEvent::Error(msg) => {
            assert!(msg.to_lowercase().contains("password"), "got: {msg}")
        }
        _ => unreachable!(),
    }
    let result = tokio::time::timeout(STEP_TIMEOUT, host.handle)
        .await
        .expect("host task should finish after a bad password");
    assert!(result.unwrap().is_err());

    client.handle.abort();
}

#[tokio::test]
async fn missing_connection_password_is_rejected() {
    let (mut host, client) = connect_pair_with_passwords(Some("hunter2".to_string()), None).await;

    wait_until(&mut host.events, "host", |ev| {
        matches!(ev, SessionEvent::Error(_))
    })
    .await;
    let result = tokio::time::timeout(STEP_TIMEOUT, host.handle)
        .await
        .expect("host task should finish");
    assert!(result.unwrap().is_err());

    // The client also errors out (it learned a password was required but had none).
    let client_result = tokio::time::timeout(STEP_TIMEOUT, client.handle)
        .await
        .expect("client task should finish");
    assert!(client_result.unwrap().is_err());
}

/// Drive a freshly connected pair through TOFU confirmation to the Ready state.
async fn reach_ready(host: &mut Peer, client: &mut Peer) {
    wait_until(&mut host.events, "host", awaiting_confirmation).await;
    wait_until(&mut client.events, "client", awaiting_confirmation).await;
    host.confirm.send(true).unwrap();
    client.confirm.send(true).unwrap();
    wait_until(&mut host.events, "host", |ev| {
        matches!(ev, SessionEvent::Ready)
    })
    .await;
    wait_until(&mut client.events, "client", |ev| {
        matches!(ev, SessionEvent::Ready)
    })
    .await;
}

/// Regression test for the 5-minute idle disconnect: nothing sent keep-alives,
/// so both sides' receive-idle timers tore down any healthy-but-quiet session
/// (each peer saw either "Receive idle timeout (300s)" or the other side's
/// resulting "early eof"). With the transport's keep-alive pings, an idle
/// session must survive past the idle window and still deliver messages.
/// The env hooks shrink the windows so this verifies in seconds: keep-alive
/// every 1 s against a 3 s idle timeout, then 6 s of pure silence.
#[tokio::test]
async fn idle_session_survives_past_the_idle_timeout() {
    // Read at message-loop startup, so set before the pair connects. nextest
    // runs each test in its own process, so this cannot leak across tests.
    std::env::set_var("P2PEM_TEST_KEEPALIVE_SECS", "1");
    std::env::set_var("P2PEM_TEST_IDLE_TIMEOUT_SECS", "3");

    let (mut host, mut client) = connect_pair().await;
    reach_ready(&mut host, &mut client).await;

    // Twice the idle window with neither side sending anything.
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;

    // The session must still be alive and deliver in both directions. If the
    // idle timer had fired, the event stream would yield Error/Disconnected
    // here and next_message would fail the test.
    host.outbound
        .send(ProtocolMessage::Text {
            text: "still here".into(),
            timestamp: 1,
            seq: 1,
        })
        .unwrap();
    match next_message(&mut client.events, "client").await {
        ProtocolMessage::Text { text, .. } => assert_eq!(text, "still here"),
        other => panic!("expected Text after idle period, got {other:?}"),
    }
    client
        .outbound
        .send(ProtocolMessage::Text {
            text: "me too".into(),
            timestamp: 2,
            seq: 1,
        })
        .unwrap();
    match next_message(&mut host.events, "host").await {
        ProtocolMessage::Text { text, .. } => assert_eq!(text, "me too"),
        other => panic!("expected Text after idle period, got {other:?}"),
    }

    host.handle.abort();
    client.handle.abort();
}

/// Regression test for the rekey/replay desync: the transport rekeys after
/// `REKEY_MESSAGE_COUNT` (100) messages, and the rekey must not break the session.
/// Sending more than that many messages one way must still deliver every one.
#[tokio::test]
async fn messages_keep_flowing_across_a_rekey() {
    let (mut host, mut client) = connect_pair().await;
    reach_ready(&mut host, &mut client).await;

    // Cross the 100-message rekey threshold so a rekey fires mid-stream.
    const COUNT: usize = 130;
    for i in 0..COUNT {
        host.outbound
            .send(ProtocolMessage::Text {
                text: format!("msg-{i}"),
                timestamp: i as u64,
                seq: (i + 1) as u64,
            })
            .unwrap();
    }

    // Every message must arrive, in order, despite the mid-stream rekey.
    for i in 0..COUNT {
        match next_message(&mut client.events, "client").await {
            ProtocolMessage::Text { text, .. } => assert_eq!(text, format!("msg-{i}")),
            other => panic!("expected Text msg-{i}, got {other:?}"),
        }
    }

    host.handle.abort();
    client.handle.abort();
}
