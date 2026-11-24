//! Relay rendezvous end to end: a self-hosted relay pairs a host and a joiner
//! by token. Punch-capable peers go direct; with punching disabled the relay
//! bridges the already-encrypted session. Both must complete the handshake and
//! deliver a message without the relay terminating the chat encryption.

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
            Ok(Some(ev)) if pred(&ev) => return ev,
            Ok(Some(_)) => {}
            Ok(None) => panic!("[{label}] event channel closed before match"),
            Err(_) => panic!("[{label}] timed out waiting for event"),
        }
    }
}

/// Reserve an ephemeral loopback port, then free it for the relay.
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    listener.local_addr().expect("local addr").port()
}

async fn pair_and_exchange() -> (String, String) {
    let relay_addr = start_relay().await;
    pair_and_exchange_on(&relay_addr, &generate_relay_token(), "rendezvous hello").await
}

async fn start_relay() -> String {
    let port = free_port();
    tokio::spawn(async move {
        let _ = run_relay_server(port).await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    format!("127.0.0.1:{port}")
}

/// One full pairing on an existing relay: handshake, TOFU on both ends, and one
/// host→client message carrying `payload`. Returns the peer labels each side
/// saw (`p2p:*` when punched, `relay:*` when bridged).
async fn pair_and_exchange_on(relay_addr: &str, token: &str, payload: &str) -> (String, String) {
    let relay_addr = relay_addr.to_string();
    let token = token.to_string();
    let host_priv = generate_rsa_keypair(RSA_KEY_BITS).expect("host key");
    let client_priv = generate_rsa_keypair(RSA_KEY_BITS).expect("client key");

    let (host_ev_tx, mut host_events) = mpsc::unbounded_channel();
    let (host_out, host_out_rx) = mpsc::unbounded_channel();
    let (_host_file_tx, host_file_rx) = mpsc::channel(8);
    let (host_confirm, host_confirm_rx) = mpsc::unbounded_channel();

    let (client_ev_tx, mut client_events) = mpsc::unbounded_channel();
    let (_client_out, client_out_rx) = mpsc::unbounded_channel();
    let (_client_file_tx, client_file_rx) = mpsc::channel(8);
    let (client_confirm, client_confirm_rx) = mpsc::unbounded_channel();

    let host_relay = relay_addr.clone();
    let host_token = token.clone();
    let host_handle = tokio::spawn(async move {
        run_host_session_via_relay(
            &host_relay,
            &host_token,
            host_priv,
            host_ev_tx,
            host_out_rx,
            host_file_rx,
            host_confirm_rx,
            uuid::Uuid::new_v4(),
        )
        .await
    });

    // The joiner follows once the token is registered.
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
            client_file_rx,
            client_confirm_rx,
            uuid::Uuid::new_v4(),
        )
        .await
    });

    // The client announces the transport it got before handshaking.
    let client_label = match wait_until(&mut client_events, "client", |ev| {
        matches!(ev, SessionEvent::Connected { .. })
    })
    .await
    {
        SessionEvent::Connected { peer } => peer,
        _ => unreachable!(),
    };

    // The host's NewConnection carries its own view of the peer label.
    let host_label = match wait_until(&mut host_events, "host", awaiting_confirmation).await {
        SessionEvent::NewConnection { peer_addr, .. } => peer_addr,
        other => panic!("host expected NewConnection, got {other:?}"),
    };
    wait_until(&mut client_events, "client", awaiting_confirmation).await;
    host_confirm.send(true).unwrap();
    client_confirm.send(true).unwrap();

    wait_until(&mut host_events, "host", is_ready).await;
    wait_until(&mut client_events, "client", is_ready).await;

    // The host's message reaches the client over the selected transport.
    host_out
        .send(ProtocolMessage::Text {
            text: payload.to_string(),
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
            assert_eq!(
                text, payload,
                "a pairing received another pairing's message"
            );
        }
        _ => unreachable!(),
    }

    host_handle.abort();
    client_handle.abort();
    (host_label, client_label)
}

// Each test runs on its own single-threaded runtime, so holding the std mutex
// across awaits cannot deadlock.
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

/// One completed session must leave the relay able to broker the next.
#[tokio::test]
async fn one_relay_brokers_a_second_session_after_the_first_ends() {
    let relay_addr = start_relay().await;

    let first = generate_relay_token();
    pair_and_exchange_on(&relay_addr, &first, "first session").await;

    let second = generate_relay_token();
    pair_and_exchange_on(&relay_addr, &second, "second session").await;

    // And the same token again, legitimate once its slot was released.
    pair_and_exchange_on(&relay_addr, &first, "third session").await;
}

/// Two simultaneous pairings must each reach their own partner.
#[tokio::test]
async fn one_relay_brokers_two_pairings_at_once() {
    let relay_addr = start_relay().await;

    let a = {
        let relay_addr = relay_addr.clone();
        tokio::spawn(async move {
            pair_and_exchange_on(&relay_addr, &generate_relay_token(), "pairing A").await
        })
    };
    let b = {
        let relay_addr = relay_addr.clone();
        tokio::spawn(async move {
            pair_and_exchange_on(&relay_addr, &generate_relay_token(), "pairing B").await
        })
    };

    // Each pairing asserts it received its *own* payload.
    a.await.expect("pairing A");
    b.await.expect("pairing B");
}

/// An unknown token must fail *fast* (an error, not a five-minute wait) and
/// leave the relay serving.
#[tokio::test]
async fn an_unknown_token_fails_fast_and_leaves_the_relay_serving() {
    let relay_addr = start_relay().await;

    let (ev_tx, _events) = mpsc::unbounded_channel();
    let (_out, out_rx) = mpsc::unbounded_channel();
    let (_file_tx, file_rx) = mpsc::channel(8);
    let (_confirm, confirm_rx) = mpsc::unbounded_channel();
    let privkey = generate_rsa_keypair(RSA_KEY_BITS).expect("key");

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        run_client_session_via_relay(
            &relay_addr,
            &generate_relay_token(),
            privkey,
            ev_tx,
            out_rx,
            file_rx,
            confirm_rx,
            uuid::Uuid::new_v4(),
        ),
    )
    .await
    .expect("joining an unknown token must not hang");
    assert!(result.is_err(), "an unknown token must not pair");

    pair_and_exchange_on(&relay_addr, &generate_relay_token(), "still serving").await;
}

/// The accept loop must survive connections that fail (both accept loops log
/// and continue, or one transient error would take the rendezvous down). This
/// injects the common case, peers that hang up without speaking; real EMFILE
/// injection would destabilise the test process.
#[tokio::test]
async fn the_relay_keeps_serving_after_a_burst_of_failed_connections() {
    let relay_addr = start_relay().await;

    // Connect and drop: each fails in `handle_relay_connection` when no hello
    // arrives. Kept under the per-address cap (20 per 30 s) so this does not
    // test the rate limiter instead.
    for _ in 0..6 {
        if let Ok(stream) = tokio::net::TcpStream::connect(&relay_addr).await {
            drop(stream);
        }
    }
    // And some that send garbage first.
    for _ in 0..6 {
        if let Ok(mut stream) = tokio::net::TcpStream::connect(&relay_addr).await {
            use tokio::io::AsyncWriteExt;
            let _ = stream.write_all(b"not a relay frame").await;
            drop(stream);
        }
    }

    pair_and_exchange_on(&relay_addr, &generate_relay_token(), "still serving").await;
}
