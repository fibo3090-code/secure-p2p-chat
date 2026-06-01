//! Per-connection server runtime (Phase 1, slice 3).
//!
//! Reuses the extracted Protocol v3 [`host_handshake`] to establish an
//! authenticated, encrypted tunnel to a client, then runs the Party loop:
//! it concurrently (a) decrypts incoming [`PartyRequest`]s, applies them via
//! [`handle_request`], and replies, and (b) writes down the tunnel any
//! [`PartyResponse`] broadcast to it by another connection through the [`Hub`].
//! Newly posted messages are fanned out to every *other* connected member.

use std::sync::Arc;

use messenger_core::network::host_handshake;
use messenger_core::party::{recv_framed, send_framed, PartyRequest, PartyResponse};
use rsa::RsaPrivateKey;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

use crate::dispatch::{handle_request, ConnState, Dispatch};
use crate::hub::Hub;
use crate::state::PartyState;

/// Serve one client connection to completion (until the peer disconnects or a
/// frame fails to authenticate). The server's identity (`server_privkey`) is
/// TOFU-verified by the client; the client's handshake-verified fingerprint is
/// bound to its membership on join. Once joined, the connection is registered with
/// `hub` to receive broadcasts.
pub async fn serve_connection<S>(
    stream: &mut S,
    server_privkey: &RsaPrivateKey,
    state: Arc<Mutex<PartyState>>,
    hub: Arc<Hub>,
) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let tunnel = host_handshake(stream, server_privkey, Uuid::new_v4()).await?;
    let cipher = tunnel.cipher;
    let aad = tunnel.transport_aad;
    let conn_id = Uuid::new_v4();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<PartyResponse>();
    let mut conn = ConnState::with_fingerprint(tunnel.peer_fingerprint);
    let mut registered = false;

    // Split so reads (incoming requests) and writes (replies + pushed broadcasts)
    // can be driven independently inside the select loop.
    let (mut rd, mut wr) = tokio::io::split(&mut *stream);

    loop {
        tokio::select! {
            // A broadcast pushed to us by another connection.
            Some(push) = out_rx.recv() => {
                send_framed(&mut wr, &cipher, &aad, &push.to_bytes()).await?;
            }
            // An incoming request from this client.
            incoming = recv_framed(&mut rd, &cipher, &aad) => {
                let req_bytes = match incoming {
                    Ok(bytes) => bytes,
                    Err(_) => break, // peer closed, or a frame failed to authenticate
                };

                let outcome = match PartyRequest::from_bytes(&req_bytes) {
                    Some(req) => {
                        let mut st = state.lock().await;
                        handle_request(&mut st, &mut conn, req)
                    }
                    None => Dispatch {
                        replies: vec![PartyResponse::Error("malformed request".to_string())],
                        broadcast: Vec::new(),
                    },
                };

                // Start receiving broadcasts once this connection has joined.
                if !registered && conn.member().is_some() {
                    hub.register(conn_id, out_tx.clone());
                    registered = true;
                }

                for resp in outcome.replies {
                    send_framed(&mut wr, &cipher, &aad, &resp.to_bytes()).await?;
                }
                for env in outcome.broadcast {
                    hub.broadcast_except(conn_id, PartyResponse::Message(env));
                }
            }
        }
    }

    hub.unregister(conn_id);
    if let Some(member) = conn.member() {
        state.lock().await.set_online(member, false);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use messenger_core::core::{generate_rsa_keypair, AesCipher};
    use messenger_core::network::client_handshake;
    use messenger_core::party::MessagePayload;
    use messenger_core::RSA_KEY_BITS;
    use rsa::RsaPrivateKey;
    use tokio::io::DuplexStream;

    /// Drive a connected client end: a duplex stream plus its negotiated tunnel.
    struct Client {
        stream: DuplexStream,
        cipher: AesCipher,
        aad: Vec<u8>,
    }

    impl Client {
        async fn send(&mut self, req: PartyRequest) {
            send_framed(&mut self.stream, &self.cipher, &self.aad, &req.to_bytes())
                .await
                .unwrap();
        }
        async fn recv(&mut self) -> PartyResponse {
            let bytes = recv_framed(&mut self.stream, &self.cipher, &self.aad)
                .await
                .unwrap();
            PartyResponse::from_bytes(&bytes).unwrap()
        }
        async fn request(&mut self, req: PartyRequest) -> PartyResponse {
            self.send(req).await;
            self.recv().await
        }
    }

    /// Spawn a `serve_connection` task and complete the client handshake against it,
    /// returning the driveable client end.
    async fn connect(
        server_priv: RsaPrivateKey,
        client_priv: &RsaPrivateKey,
        state: Arc<Mutex<PartyState>>,
        hub: Arc<Hub>,
    ) -> (Client, tokio::task::JoinHandle<()>) {
        let (mut server_stream, mut client_stream) = tokio::io::duplex(1 << 16);
        let handle = tokio::spawn(async move {
            let _ = serve_connection(&mut server_stream, &server_priv, state, hub).await;
        });
        let tunnel = client_handshake(&mut client_stream, client_priv, Uuid::new_v4())
            .await
            .expect("client handshake");
        (
            Client {
                stream: client_stream,
                cipher: tunnel.cipher,
                aad: tunnel.transport_aad,
            },
            handle,
        )
    }

    #[tokio::test]
    async fn client_joins_posts_and_fetches_history_over_the_tunnel() {
        let server_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let client_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let state = Arc::new(Mutex::new(PartyState::new("TestSrv", None)));
        let hub = Arc::new(Hub::new());
        let channel = state.lock().await.default_channel();

        let (mut client, server) =
            connect(server_priv, &client_priv, state.clone(), hub.clone()).await;

        let joined = client
            .request(PartyRequest::Join {
                username: "alice".to_string(),
                password: None,
            })
            .await;
        assert!(
            matches!(joined, PartyResponse::Joined { .. }),
            "got {joined:?}"
        );

        let posted = client
            .request(PartyRequest::PostMessage {
                channel,
                text: "hello server".to_string(),
            })
            .await;
        match posted {
            PartyResponse::MessagePosted { channel: c, seq } => {
                assert_eq!(c, channel);
                assert_eq!(seq, 1);
            }
            other => panic!("expected MessagePosted, got {other:?}"),
        }

        let history = client
            .request(PartyRequest::FetchHistory {
                channel,
                since_seq: 0,
            })
            .await;
        match history {
            PartyResponse::History(items) => {
                assert_eq!(items.len(), 1);
                assert_eq!(
                    items[0].payload,
                    MessagePayload::Text("hello server".to_string())
                );
            }
            other => panic!("expected History, got {other:?}"),
        }

        assert_eq!(state.lock().await.history_since(channel, 0).len(), 1);
        drop(client);
        server.abort();
    }

    #[tokio::test]
    async fn requests_before_join_are_rejected_over_the_tunnel() {
        let server_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let client_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let state = Arc::new(Mutex::new(PartyState::new("TestSrv", None)));
        let hub = Arc::new(Hub::new());

        let (mut client, server) =
            connect(server_priv, &client_priv, state.clone(), hub.clone()).await;

        match client.request(PartyRequest::ListMembers).await {
            PartyResponse::Error(msg) => assert_eq!(msg, "join required"),
            other => panic!("expected join-required error, got {other:?}"),
        }
        drop(client);
        server.abort();
    }

    #[tokio::test]
    async fn posted_message_is_broadcast_to_other_members() {
        let server_priv_a = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let server_priv_b = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let alice_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let bob_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();

        let state = Arc::new(Mutex::new(PartyState::new("TestSrv", None)));
        let hub = Arc::new(Hub::new());
        let channel = state.lock().await.default_channel();

        // Two clients connect to the same shared state + hub.
        let (mut alice, alice_srv) =
            connect(server_priv_a, &alice_priv, state.clone(), hub.clone()).await;
        let (mut bob, bob_srv) =
            connect(server_priv_b, &bob_priv, state.clone(), hub.clone()).await;

        // Both join (so both are registered for broadcasts).
        assert!(matches!(
            alice
                .request(PartyRequest::Join {
                    username: "alice".to_string(),
                    password: None,
                })
                .await,
            PartyResponse::Joined { .. }
        ));
        assert!(matches!(
            bob.request(PartyRequest::Join {
                username: "bob".to_string(),
                password: None,
            })
            .await,
            PartyResponse::Joined { .. }
        ));
        // Bob is registered once his join is acknowledged.
        assert_eq!(hub.len(), 2);

        // Alice posts; she gets the ack, Bob receives the broadcast live.
        let ack = alice
            .request(PartyRequest::PostMessage {
                channel,
                text: "hi bob".to_string(),
            })
            .await;
        assert!(matches!(ack, PartyResponse::MessagePosted { .. }));

        match bob.recv().await {
            PartyResponse::Message(env) => {
                assert_eq!(env.channel, channel);
                assert_eq!(env.payload, MessagePayload::Text("hi bob".to_string()));
            }
            other => panic!("expected broadcast Message, got {other:?}"),
        }

        drop(alice);
        drop(bob);
        alice_srv.abort();
        bob_srv.abort();
    }
}
