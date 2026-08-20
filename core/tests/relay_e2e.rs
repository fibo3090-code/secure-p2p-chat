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
    let relay_addr = start_relay().await;
    pair_and_exchange_on(&relay_addr, &generate_relay_token(), "rendezvous hello").await
}

/// Start a relay on a free port and wait for it to be listening.
async fn start_relay() -> String {
    let port = free_port();
    tokio::spawn(async move {
        let _ = run_relay_server(port).await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    format!("127.0.0.1:{port}")
}

/// One full pairing on an existing relay: handshake, TOFU on both ends, and one
/// host→client message carrying `payload`. Split out from
/// [`pair_and_exchange`] so several pairings can share one relay — which is
/// what the reconnect and concurrency tests need.
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
            host_file_rx,
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
            client_file_rx,
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

/// A relay is a long-lived service: peers drop and redial it constantly. One
/// completed session must leave it able to broker the next one — a rendezvous
/// slot that leaked, or a listener that died with its first pairing, would make
/// the app work exactly once per relay restart.
#[tokio::test]
async fn one_relay_brokers_a_second_session_after_the_first_ends() {
    let relay_addr = start_relay().await;

    let first = generate_relay_token();
    pair_and_exchange_on(&relay_addr, &first, "first session").await;

    // A fresh token, as a reconnecting pair would use.
    let second = generate_relay_token();
    pair_and_exchange_on(&relay_addr, &second, "second session").await;

    // And the very same token again, which is legitimate once its slot has been
    // released by the pairing that consumed it.
    pair_and_exchange_on(&relay_addr, &first, "third session").await;
}

/// Two pairings brokered at the same time by one relay. Each must reach its own
/// partner: crossing them would put two strangers into a session together, with
/// nothing but the v3 handshake's fingerprint check between them.
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

    // `pair_and_exchange_on` asserts each side received its *own* payload, so
    // completing at all is the crossing check.
    a.await.expect("pairing A");
    b.await.expect("pairing B");
}

/// A joiner that dials a token nobody registered must fail, and must fail
/// *quickly* — the relay answers with an error rather than parking the client
/// on a five-minute wait. The relay must also still be serving afterwards.
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

    // The relay survived the refusal and still brokers a real pairing.
    pair_and_exchange_on(&relay_addr, &generate_relay_token(), "still serving").await;
}

/// The accept loop must survive connections that fail, one after another.
///
/// Both public accept loops log a failed `accept()` and continue rather than
/// propagating it, because a transient failure — the peer vanishing between the
/// SYN and the accept, or a momentary EMFILE — would otherwise take the whole
/// rendezvous down for the life of the process. That is a one-line decision with
/// no test behind it, and its absence is invisible until the day it matters.
///
/// What this injects is the failure that actually happens constantly in the
/// wild: peers that connect and hang up without speaking. True EMFILE injection
/// is deliberately not attempted — lowering `RLIMIT_NOFILE` would destabilise the
/// test process itself and has no Windows equivalent, so it would trade a real
/// check for a flaky one. This covers the same `continue`.
#[tokio::test]
async fn the_relay_keeps_serving_after_a_burst_of_failed_connections() {
    let relay_addr = start_relay().await;

    // Connect and drop, repeatedly. Each one reaches the accept loop and then
    // fails in `handle_relay_connection` when the hello never arrives.
    // Under the relay's own per-address cap (20 per 30s), with room left for the
    // pairing below — otherwise this would be testing the rate limiter instead.
    for _ in 0..6 {
        if let Ok(stream) = tokio::net::TcpStream::connect(&relay_addr).await {
            drop(stream);
        }
    }
    // And some that connect, send garbage, and leave.
    for _ in 0..6 {
        if let Ok(mut stream) = tokio::net::TcpStream::connect(&relay_addr).await {
            use tokio::io::AsyncWriteExt;
            let _ = stream.write_all(b"not a relay frame").await;
            drop(stream);
        }
    }

    // The listener is still there and still brokers a real pairing.
    pair_and_exchange_on(&relay_addr, &generate_relay_token(), "still serving").await;
}
