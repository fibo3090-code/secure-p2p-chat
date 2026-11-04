//! Host-listener lifecycle: binding is the caller's business, and the port is
//! released the moment a peer completes the handshake — not before, or one
//! connection that never speaks closes the port for everyone (audit NET-02).
//!
//! Both properties exist because of one bug: `run_host_session` used to bind the
//! port itself and hold the listener for the whole conversation. After the first
//! peer connected, the app's auto-rehost called `start_host` again, the bind
//! failed with EADDRINUSE *inside the spawned task* (logged and nothing else) —
//! and the UI was left showing a "Host on :port" conversation that reported
//! itself connected while nothing was listening. Hosting accepted exactly one
//! peer, ever, and said nothing about it.

use messenger_core::core::generate_rsa_keypair;
use messenger_core::network::{
    bind_host_listener, run_client_session_over_stream, run_host_session,
};
use messenger_core::types::SessionEvent;
use messenger_core::RSA_KEY_BITS;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::mpsc;

/// Try to bind `port` for up to ~2s. The release happens on the session task's
/// timeline (right after a handshake completes), so a single immediate attempt
/// would be racy; a bounded retry is not.
async fn rebind_within(port: u16) -> bool {
    for _ in 0..100 {
        if bind_host_listener(port).await.is_ok() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

#[tokio::test]
async fn host_keeps_its_port_through_idle_connections_and_releases_it_for_a_peer() {
    let listener = bind_host_listener(0)
        .await
        .expect("binding an ephemeral port");
    let port = listener.local_addr().unwrap().port();

    let (to_app_tx, mut to_app_rx) = mpsc::unbounded_channel();
    let (_from_app_tx, from_app_rx) = mpsc::unbounded_channel();
    let (_file_tx, file_rx) = mpsc::channel(4);
    let (_confirm_tx, confirm_rx) = mpsc::unbounded_channel();
    let privkey = generate_rsa_keypair(RSA_KEY_BITS).expect("keypair");

    let session = tokio::spawn(run_host_session(
        listener,
        privkey,
        to_app_tx,
        from_app_rx,
        file_rx,
        confirm_rx,
        uuid::Uuid::new_v4(),
        None,
    ));

    // The session reports the port it actually bound.
    match tokio::time::timeout(Duration::from_secs(5), to_app_rx.recv()).await {
        Ok(Some(SessionEvent::Listening { port: reported })) => assert_eq!(reported, port),
        other => panic!("expected Listening, got {other:?}"),
    }

    // A connection that never speaks must NOT take the port with it.
    let idle = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connecting to the host");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        bind_host_listener(port).await.is_err(),
        "an idle connection must not make the host give up its port"
    );

    // A real peer completes the handshake (and the password gate): now the
    // listener is released.
    let mut peer = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connecting to the host");
    let client_key = generate_rsa_keypair(RSA_KEY_BITS).expect("keypair");
    let (client_tx, _client_rx) = mpsc::unbounded_channel();
    let (_client_out, client_out_rx) = mpsc::unbounded_channel();
    let (_client_file, client_file_rx) = mpsc::channel(4);
    let (_client_confirm, client_confirm_rx) = mpsc::unbounded_channel();
    let client = tokio::spawn(async move {
        run_client_session_over_stream(
            &mut peer,
            "host".into(),
            client_key,
            client_tx,
            client_out_rx,
            client_file_rx,
            client_confirm_rx,
            uuid::Uuid::new_v4(),
            None,
        )
        .await
    });

    assert!(
        rebind_within(port).await,
        "the host must release its port once a peer is accepted, or every \
         auto-rehost after the first peer fails with EADDRINUSE"
    );

    drop(idle);
    client.abort();
    session.abort();
}

/// A bind failure has to surface to the caller, so it can refuse to create the
/// chat and session state that made the phantom "Host on :port" conversation.
#[tokio::test]
async fn binding_an_occupied_port_is_an_error_the_caller_can_see() {
    let held = bind_host_listener(0).await.expect("first bind");
    let port = held.local_addr().unwrap().port();

    let err = bind_host_listener(port)
        .await
        .expect_err("a second bind on a held port must fail");
    assert!(
        err.to_string().contains(&port.to_string()),
        "the error should name the port it could not take, got: {err}"
    );
}
