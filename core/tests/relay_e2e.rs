//! End-to-end tests for the relay rendezvous: a self-hosted server pairs a
//! host and a joiner by token. Punch-capable peers hole punch a direct TCP
//! connection (the relay never carries session bytes); when punching is
//! disabled the relay bridges the (already-encrypted) v3 session traffic as
//! before. Both paths must complete the full handshake and deliver an
//! application message without the relay ever terminating the chat encryption.

use messenger_core::core::{generate_rsa_keypair, ProtocolMessage};
use messenger_core::network::{
    generate_relay_token, run_client_session_via_relay, run_host_session_via_relay,
    run_relay_server, NO_HOLEPUNCH_ENV,
};
use messenger_core::types::SessionEvent;
use messenger_core::RSA_KEY_BITS;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::mpsc;

const STEP_TIMEOUT: Duration = Duration::from_secs(25);

/// `NO_HOLEPUNCH_ENV` is process-global; serialize the tests that read it so
/// `cargo test` (threaded) behaves like `cargo nextest` (process-per-test).
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Host emits `NewConnection`; client emits `ShowFingerprintVerification`. Both
/// mean "peer identity received, awaiting TOFU confirmation".
fn awaiting_confirmation(ev: &SessionEvent) -> bool {
    matches!(
        ev,
        SessionEvent::NewConnection { .. } | SessionEvent::ShowFingerprintVerification { .. }
    )
}

async fn wait_until<F>(
    rx: &mut mpsc::UnboundedReceiver<SessionEvent>,
    label: &str,
    mut pred: F,
) -> SessionEvent
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
            }
            Ok(None) => panic!("[{label}] event channel closed before match"),
            Err(_) => panic!("[{label}] timed out waiting for event"),
        }
    }
}

/// Reserve an ephemeral loopback port, then free it so the relay can bind it.
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    listener.local_addr().expect("local addr").port()
}

/// Drive a full host+joiner pairing through a fresh relay server: handshake,
/// TOFU confirmation on both ends, and one host→client message. Returns the
/// peer labels observed on each side (`p2p:*` for a punched direct socket,
/// `relay:*` when bridged) so callers can assert which transport was used.
async fn pair_and_exchange() -> (String, String) {
    let port = free_port();
    let relay_addr = format!("127.0.0.1:{port}");

    tokio::spawn(async move {
        let _ = run_relay_server(port).await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;

    let token = generate_relay_token();
    let host_priv = generate_rsa_keypair(RSA_KEY_BITS).expect("host key");
    let client_priv = generate_rsa_keypair(RSA_KEY_BITS).expect("client key");

    let (host_ev_tx, mut host_events) = mpsc::unbounded_channel();
    let (host_out, host_out_rx) = mpsc::unbounded_channel();
    let (host_confirm, host_confirm_rx) = mpsc::unbounded_channel();

    let (client_ev_tx, mut client_events) = mpsc::unbounded_channel();
    let (_client_out, client_out_rx) = mpsc::unbounded_channel();
    let (client_confirm, client_confirm_rx) = mpsc::unbounded_channel();

    // Host registers the token first.
    let host_relay = relay_addr.clone();
    let host_token = token.clone();
    let host_handle = tokio::spawn(async move {
        run_host_session_via_relay(
            &host_relay,
            &host_token,
            host_priv,
            host_ev_tx,
            host_out_rx,
            host_confirm_rx,
            uuid::Uuid::new_v4(),
        )
        .await
    });

    // Joiner connects shortly after so the token is already registered.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let client_relay = relay_addr.clone();
    let client_token = token.clone();
    let client_handle = tokio::spawn(async move {
        run_client_session_via_relay(
            &client_relay,
            &client_token,
            client_priv,
            client_ev_tx,
            client_out_rx,
            client_confirm_rx,
            uuid::Uuid::new_v4(),
        )
        .await
    });

    // The client announces the transport it ended up with before handshaking.
    let client_label = match wait_until(&mut client_events, "client", |ev| {
        matches!(ev, SessionEvent::Connected { .. })
    })
    .await
    {
        SessionEvent::Connected { peer } => peer,
        _ => unreachable!(),
    };

    // TOFU confirmation on both ends; the host's NewConnection carries its
    // own view of the peer label.
    let host_label = match wait_until(&mut host_events, "host", awaiting_confirmation).await {
        SessionEvent::NewConnection { peer_addr, .. } => peer_addr,
        other => panic!("host expected NewConnection, got {other:?}"),
    };
    wait_until(&mut client_events, "client", awaiting_confirmation).await;
    host_confirm.send(true).unwrap();
    client_confirm.send(true).unwrap();

    wait_until(&mut host_events, "host", |ev| {
        matches!(ev, SessionEvent::Ready)
    })
    .await;
    wait_until(&mut client_events, "client", |ev| {
        matches!(ev, SessionEvent::Ready)
    })
    .await;

    // A message sent by the host arrives at the client, proving the selected
    // transport carries the encrypted session end to end.
    host_out
        .send(ProtocolMessage::Text {
            text: "rendezvous hello".to_string(),
            timestamp: 1,
            seq: 1,
        })
        .unwrap();

    let received = wait_until(&mut client_events, "client", |ev| {
        matches!(
            ev,
            SessionEvent::MessageReceived(ProtocolMessage::Text { .. })
        )
    })
    .await;
    match received {
        SessionEvent::MessageReceived(ProtocolMessage::Text { text, .. }) => {
            assert_eq!(text, "rendezvous hello");
        }
        _ => unreachable!(),
    }

    host_handle.abort();
    client_handle.abort();
    (host_label, client_label)
}

// Each test runs on its own single-threaded runtime, so blocking that thread
// on the std mutex cannot deadlock — the lock exists to serialize env access.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn two_peers_hole_punch_a_direct_session_via_the_rendezvous() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    std::env::remove_var(NO_HOLEPUNCH_ENV);

    let (host_label, client_label) = pair_and_exchange().await;
    assert!(
        host_label.starts_with("p2p:"),
        "host should see a punched direct peer, got {host_label}"
    );
    assert!(
        client_label.starts_with("p2p:"),
        "client should see a punched direct peer, got {client_label}"
    );
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn two_peers_pair_through_the_bridged_relay_when_punching_is_disabled() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    std::env::set_var(NO_HOLEPUNCH_ENV, "1");

    let (host_label, client_label) = pair_and_exchange().await;
    std::env::remove_var(NO_HOLEPUNCH_ENV);

    assert!(
        host_label.starts_with("relay:"),
        "host should be bridged, got {host_label}"
    );
    assert!(
        client_label.starts_with("relay:"),
        "client should be bridged, got {client_label}"
    );
}
