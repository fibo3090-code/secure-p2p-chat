//! End-to-end test for the relay path: a self-hosted rendezvous server pairs a
//! host and a joiner by token and forwards their (already-encrypted) v3 session
//! traffic. Verifies that two peers who both reach the relay complete the full
//! handshake and exchange an application message — without the relay ever
//! terminating the chat encryption.

use messenger_core::core::{generate_rsa_keypair, ProtocolMessage};
use messenger_core::network::{
    generate_relay_token, run_client_session_via_relay, run_host_session_via_relay,
    run_relay_server,
};
use messenger_core::types::SessionEvent;
use messenger_core::RSA_KEY_BITS;
use std::time::Duration;
use tokio::sync::mpsc;

const STEP_TIMEOUT: Duration = Duration::from_secs(25);

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

#[tokio::test]
async fn two_peers_pair_through_relay_and_exchange_a_message() {
    let port = free_port();
    let relay_addr = format!("127.0.0.1:{port}");

    // Start the relay server in the background.
    tokio::spawn(async move {
        let _ = run_relay_server(port).await;
    });
    // Give the listener a moment to come up.
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

    // TOFU confirmation on both ends.
    wait_until(&mut host_events, "host", awaiting_confirmation).await;
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

    // A message sent by the host arrives at the client, proving the relay
    // forwarded the encrypted transport end to end.
    host_out
        .send(ProtocolMessage::Text {
            text: "relayed hello".to_string(),
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
            assert_eq!(text, "relayed hello");
        }
        _ => unreachable!(),
    }

    host_handle.abort();
    client_handle.abort();
}
