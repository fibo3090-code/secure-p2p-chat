//! Per-connection server runtime (Phase 1, slice 3).
//!
//! Reuses the extracted Protocol v3 [`host_handshake`] to establish an
//! authenticated, encrypted tunnel to a client, then runs the Party request loop:
//! decrypt a [`PartyRequest`] off the tunnel, apply it via [`handle_request`],
//! and encrypt the [`PartyResponse`]s back. Cross-connection broadcast/fan-out of
//! newly posted messages is the next step.

use std::sync::Arc;

use messenger_core::network::host_handshake;
use messenger_core::party::{recv_framed, send_framed, PartyRequest, PartyResponse};
use rsa::RsaPrivateKey;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::dispatch::{handle_request, ConnState};
use crate::state::PartyState;

/// Serve one client connection to completion (until the peer disconnects or a
/// frame fails to authenticate). The server's identity (`server_privkey`) is
/// TOFU-verified by the client; the client's handshake-verified fingerprint is
/// bound to its membership on join.
pub async fn serve_connection<S>(
    stream: &mut S,
    server_privkey: &RsaPrivateKey,
    state: Arc<Mutex<PartyState>>,
) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let tunnel = host_handshake(stream, server_privkey, Uuid::new_v4()).await?;
    let cipher = tunnel.cipher;
    let aad = tunnel.transport_aad;
    let mut conn = ConnState::with_fingerprint(tunnel.peer_fingerprint);

    loop {
        let req_bytes = match recv_framed(stream, &cipher, &aad).await {
            Ok(bytes) => bytes,
            // A receive error means the peer closed the connection (or sent a
            // frame that failed to authenticate); end the session cleanly.
            Err(_) => break,
        };

        let responses = match PartyRequest::from_bytes(&req_bytes) {
            Some(req) => {
                let mut st = state.lock().await;
                handle_request(&mut st, &mut conn, req)
            }
            None => vec![PartyResponse::Error("malformed request".to_string())],
        };

        for resp in responses {
            send_framed(stream, &cipher, &aad, &resp.to_bytes()).await?;
        }
    }

    // Mark the member offline when their connection drops.
    if let Some(member) = conn.member() {
        state.lock().await.set_online(member, false);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use messenger_core::core::generate_rsa_keypair;
    use messenger_core::network::client_handshake;
    use messenger_core::party::MessagePayload;
    use messenger_core::RSA_KEY_BITS;

    /// End-to-end over an in-memory tunnel: a client completes the v3 handshake to
    /// the server, joins, posts a channel message, and fetches history — and the
    /// server's durable state reflects the post.
    #[tokio::test]
    async fn client_joins_posts_and_fetches_history_over_the_tunnel() {
        let server_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let client_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();

        let state = Arc::new(Mutex::new(PartyState::new("TestSrv", None)));
        let channel = state.lock().await.default_channel();

        let (mut server_stream, mut client_stream) = tokio::io::duplex(1 << 16);

        let server_state = state.clone();
        let server = tokio::spawn(async move {
            let _ = serve_connection(&mut server_stream, &server_priv, server_state).await;
        });

        // Client side of the handshake.
        let tunnel = client_handshake(&mut client_stream, &client_priv, Uuid::new_v4())
            .await
            .expect("client handshake");
        let cipher = tunnel.cipher;
        let aad = tunnel.transport_aad;

        // Helper: send one request, read one response.
        async fn round_trip(
            stream: &mut tokio::io::DuplexStream,
            cipher: &messenger_core::core::AesCipher,
            aad: &[u8],
            req: PartyRequest,
        ) -> PartyResponse {
            send_framed(stream, cipher, aad, &req.to_bytes())
                .await
                .unwrap();
            let bytes = recv_framed(stream, cipher, aad).await.unwrap();
            PartyResponse::from_bytes(&bytes).unwrap()
        }

        // Join.
        let joined = round_trip(
            &mut client_stream,
            &cipher,
            &aad,
            PartyRequest::Join {
                username: "alice".to_string(),
                password: None,
            },
        )
        .await;
        assert!(
            matches!(joined, PartyResponse::Joined { .. }),
            "got {joined:?}"
        );

        // Post a message.
        let posted = round_trip(
            &mut client_stream,
            &cipher,
            &aad,
            PartyRequest::PostMessage {
                channel,
                text: "hello server".to_string(),
            },
        )
        .await;
        match posted {
            PartyResponse::MessagePosted { channel: c, seq } => {
                assert_eq!(c, channel);
                assert_eq!(seq, 1);
            }
            other => panic!("expected MessagePosted, got {other:?}"),
        }

        // Fetch history.
        let history = round_trip(
            &mut client_stream,
            &cipher,
            &aad,
            PartyRequest::FetchHistory {
                channel,
                since_seq: 0,
            },
        )
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

        // The server stored it durably (offline buffering).
        assert_eq!(state.lock().await.history_since(channel, 0).len(), 1);

        drop(client_stream);
        server.abort();
    }

    #[tokio::test]
    async fn requests_before_join_are_rejected_over_the_tunnel() {
        let server_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let client_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let state = Arc::new(Mutex::new(PartyState::new("TestSrv", None)));
        let (mut server_stream, mut client_stream) = tokio::io::duplex(1 << 16);

        let server_state = state.clone();
        let server = tokio::spawn(async move {
            let _ = serve_connection(&mut server_stream, &server_priv, server_state).await;
        });

        let tunnel = client_handshake(&mut client_stream, &client_priv, Uuid::new_v4())
            .await
            .unwrap();
        send_framed(
            &mut client_stream,
            &tunnel.cipher,
            &tunnel.transport_aad,
            &PartyRequest::ListMembers.to_bytes(),
        )
        .await
        .unwrap();
        let bytes = recv_framed(&mut client_stream, &tunnel.cipher, &tunnel.transport_aad)
            .await
            .unwrap();
        match PartyResponse::from_bytes(&bytes).unwrap() {
            PartyResponse::Error(msg) => assert_eq!(msg, "join required"),
            other => panic!("expected join-required error, got {other:?}"),
        }

        drop(client_stream);
        server.abort();
    }
}
