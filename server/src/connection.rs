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
use messenger_core::party::{recv_framed, send_framed, FrameSeq, PartyRequest, PartyResponse};
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
    // Bounded: an unbounded lane let a client that stopped reading its socket
    // accumulate every broadcast on our heap. See `hub::BROADCAST_QUEUE_DEPTH`.
    let (out_tx, mut out_rx) = mpsc::channel::<PartyResponse>(crate::hub::BROADCAST_QUEUE_DEPTH);
    let mut conn = ConnState::with_fingerprint(tunnel.peer_fingerprint);
    let mut registered = false;
    // Per-direction frame counters: every Party frame carries a sequence inside
    // the AEAD so a replayed frame cannot be accepted twice.
    let mut send_seq = FrameSeq::new();
    let mut recv_seq = FrameSeq::new();

    // Split so reads (incoming requests) and writes (replies + pushed broadcasts)
    // can be driven independently inside the select loop.
    let (mut rd, mut wr) = tokio::io::split(&mut *stream);

    loop {
        tokio::select! {
            // A broadcast pushed to us by another connection.
            Some(push) = out_rx.recv() => {
                send_framed(&mut wr, &cipher, &aad, &mut send_seq, &push.to_bytes()).await?;
            }
            // An incoming request from this client.
            incoming = recv_framed(&mut rd, &cipher, &aad, &mut recv_seq) => {
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
                        directed: Vec::new(),
                    },
                };

                // Register for broadcasts/DMs once this connection has joined.
                if !registered {
                    if let Some(member) = conn.member() {
                        hub.register(conn_id, member, out_tx.clone());
                        registered = true;
                    }
                }

                for resp in outcome.replies {
                    send_framed(&mut wr, &cipher, &aad, &mut send_seq, &resp.to_bytes()).await?;
                }
                for resp in outcome.broadcast {
                    hub.broadcast_except(conn_id, resp);
                }
                for (member, resp) in outcome.directed {
                    // Excluding this connection lets a DM be delivered to the
                    // sender's *other* devices without duplicating it on the one
                    // that sent it (which already appended it optimistically).
                    hub.send_to_member_except(member, conn_id, resp);
                }
            }
        }
    }

    hub.unregister(conn_id);
    if let Some(member) = conn.member() {
        // A client that vanished mid-transfer must not pin its spool for the
        // life of the process. Each pending upload can hold up to
        // MAX_PARTY_FILE_BYTES, so this is memory, not just tidiness.
        if !conn.open_uploads().is_empty() {
            state.lock().await.cancel_uploads_for(member);
        }
        // Presence is per-member but connections are per-device: only go offline
        // once this member's *last* connection is gone, or closing one of two
        // open clients would report them as offline while they are still here.
        if !hub.member_is_connected(member) {
            let directory = {
                let mut st = state.lock().await;
                st.set_online(member, false);
                st.members()
            };
            // Tell everyone still connected. Without this the online dots only
            // ever moved when a client happened to re-request the directory.
            hub.broadcast_all(PartyResponse::Members(directory));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use messenger_core::core::generate_rsa_keypair;
    use messenger_core::party::{
        ChannelKind, MessagePayload, PartyClient, UploadTarget, PARTY_CHUNK_BYTES,
    };
    use messenger_core::RSA_KEY_BITS;
    use rsa::RsaPrivateKey;
    use tokio::io::DuplexStream;

    type TestClient = PartyClient<DuplexStream>;

    /// Send a request and read the reply, skipping unsolicited pushes.
    ///
    /// Joining broadcasts the refreshed directory and a channel change
    /// broadcasts `DirectoryChanged`, so a client that was already online has
    /// those waiting ahead of its own next reply. Real clients apply pushes and
    /// replies from one interleaved stream (`PartyManager::apply` does); these
    /// single-stepped tests skip the pushes to assert on the reply they asked
    /// for.
    async fn request(client: &mut TestClient, req: PartyRequest) -> PartyResponse {
        client.send(&req).await.unwrap();
        loop {
            match client.recv().await.unwrap() {
                PartyResponse::Members(_) | PartyResponse::DirectoryChanged => continue,
                other => return other,
            }
        }
    }

    /// Spawn a `serve_connection` task and connect a real `PartyClient` to it.
    async fn connect(
        server_priv: RsaPrivateKey,
        client_priv: &RsaPrivateKey,
        state: Arc<Mutex<PartyState>>,
        hub: Arc<Hub>,
    ) -> (TestClient, tokio::task::JoinHandle<()>) {
        let (mut server_stream, client_stream) = tokio::io::duplex(1 << 16);
        let handle = tokio::spawn(async move {
            let _ = serve_connection(&mut server_stream, &server_priv, state, hub).await;
        });
        let client = PartyClient::connect(client_stream, client_priv, Uuid::new_v4())
            .await
            .expect("client handshake");
        (client, handle)
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

        let joined = client.join("alice", None).await.unwrap();
        assert!(
            matches!(joined, PartyResponse::Joined { .. }),
            "got {joined:?}"
        );

        let posted = request(
            &mut client,
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

        let history = request(
            &mut client,
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

        let guard = state.lock().await;
        let member = guard.members()[0].id;
        assert_eq!(guard.history_since(member, channel, 0).len(), 1);
        drop(guard);
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

        match request(&mut client, PartyRequest::ListMembers).await {
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
            alice.join("alice", None).await.unwrap(),
            PartyResponse::Joined { .. }
        ));
        assert!(matches!(
            bob.join("bob", None).await.unwrap(),
            PartyResponse::Joined { .. }
        ));
        // Bob is registered once his join is acknowledged.
        assert_eq!(hub.len(), 2);

        // Alice posts; she gets the ack, Bob receives the broadcast live.
        let ack = request(
            &mut alice,
            PartyRequest::PostMessage {
                channel,
                text: "hi bob".to_string(),
            },
        )
        .await;
        assert!(matches!(ack, PartyResponse::MessagePosted { .. }));

        match bob.recv().await.unwrap() {
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

    #[tokio::test]
    async fn direct_message_is_delivered_and_fetchable() {
        let server_priv_a = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let server_priv_b = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let alice_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let bob_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let state = Arc::new(Mutex::new(PartyState::new("TestSrv", None)));
        let hub = Arc::new(Hub::new());

        let (mut alice, alice_srv) =
            connect(server_priv_a, &alice_priv, state.clone(), hub.clone()).await;
        let (mut bob, bob_srv) =
            connect(server_priv_b, &bob_priv, state.clone(), hub.clone()).await;

        let alice_id = match alice.join("alice", None).await.unwrap() {
            PartyResponse::Joined { member_id, .. } => member_id,
            other => panic!("expected Joined, got {other:?}"),
        };
        let bob_id = match bob.join("bob", None).await.unwrap() {
            PartyResponse::Joined { member_id, .. } => member_id,
            other => panic!("expected Joined, got {other:?}"),
        };

        // Alice DMs Bob: she gets an ack; Bob receives the message live.
        let ack = request(
            &mut alice,
            PartyRequest::SendDm {
                to: bob_id,
                text: "hey bob".to_string(),
            },
        )
        .await;
        assert!(matches!(ack, PartyResponse::MessagePosted { .. }));

        match bob.recv().await.unwrap() {
            PartyResponse::Message(env) => {
                assert_eq!(env.sender, alice_id);
                assert_eq!(
                    env.channel,
                    messenger_core::party::dm_thread_id(alice_id, bob_id)
                );
                assert_eq!(env.payload, MessagePayload::Text("hey bob".to_string()));
            }
            other => panic!("expected DM Message, got {other:?}"),
        }

        // Bob catches up on DM history with Alice.
        match request(
            &mut bob,
            PartyRequest::FetchDmHistory {
                with: alice_id,
                since_seq: 0,
            },
        )
        .await
        {
            PartyResponse::History(items) => {
                assert_eq!(items.len(), 1);
                assert_eq!(
                    items[0].payload,
                    MessagePayload::Text("hey bob".to_string())
                );
            }
            other => panic!("expected DM history, got {other:?}"),
        }

        drop(alice);
        drop(bob);
        alice_srv.abort();
        bob_srv.abort();
    }

    /// Join and return the assigned member id.
    async fn join_as(client: &mut TestClient, name: &str) -> Uuid {
        match client.join(name, None).await.unwrap() {
            PartyResponse::Joined { member_id, .. } => member_id,
            other => panic!("expected Joined, got {other:?}"),
        }
    }

    /// Read whatever is already queued for this client and stop when it goes
    /// quiet, so a later assertion sees only frames caused by what follows.
    async fn drain(client: &mut TestClient) -> Vec<PartyResponse> {
        let mut seen = Vec::new();
        while let Ok(Ok(resp)) =
            tokio::time::timeout(std::time::Duration::from_millis(200), client.recv()).await
        {
            seen.push(resp);
        }
        seen
    }

    /// A private channel's messages must not reach a member who is not in it —
    /// over the wire, not merely in the dispatcher's return value.
    ///
    /// This is the end-to-end half of the access leak: the dispatcher deciding
    /// on a `directed` list proves nothing on its own, because the bug lived in
    /// the gap between what the dispatcher returned and who the hub actually
    /// sent it to. `broadcast_except` pushed to every registered connection, so
    /// a member correctly denied the channel in `ListChannels` and denied its
    /// history still received every message posted to it.
    #[tokio::test]
    async fn a_private_channels_messages_never_reach_a_non_member_over_the_tunnel() {
        let state = Arc::new(Mutex::new(PartyState::new("TestSrv", None)));
        let hub = Arc::new(Hub::new());

        let client_privs: Vec<RsaPrivateKey> = (0..3)
            .map(|_| generate_rsa_keypair(RSA_KEY_BITS).unwrap())
            .collect();
        let (mut owner, owner_srv) = connect(
            generate_rsa_keypair(RSA_KEY_BITS).unwrap(),
            &client_privs[0],
            state.clone(),
            hub.clone(),
        )
        .await;
        let (mut bob, bob_srv) = connect(
            generate_rsa_keypair(RSA_KEY_BITS).unwrap(),
            &client_privs[1],
            state.clone(),
            hub.clone(),
        )
        .await;
        let (mut carol, carol_srv) = connect(
            generate_rsa_keypair(RSA_KEY_BITS).unwrap(),
            &client_privs[2],
            state.clone(),
            hub.clone(),
        )
        .await;

        // The first to join owns the server, so the owner can make a private
        // channel; the other two are ordinary members.
        let _owner_id = join_as(&mut owner, "owner").await;
        let bob_id = join_as(&mut bob, "bob").await;
        let _carol_id = join_as(&mut carol, "carol").await;

        let channels = request(
            &mut owner,
            PartyRequest::CreateChannelOfKind {
                name: "secret".to_string(),
                kind: ChannelKind::Private,
                members: vec![bob_id],
            },
        )
        .await;
        let private = match channels {
            PartyResponse::Channels(list) => list.iter().find(|c| c.name == "secret").unwrap().id,
            other => panic!("expected the refreshed channel list, got {other:?}"),
        };

        // Carol cannot even see it.
        match request(&mut carol, PartyRequest::ListChannels).await {
            PartyResponse::Channels(list) => assert!(
                !list.iter().any(|c| c.id == private),
                "a private channel must not be listed to a non-member"
            ),
            other => panic!("expected Channels, got {other:?}"),
        }

        // Quieten both spectators, so anything below is caused by the post.
        drain(&mut bob).await;
        drain(&mut carol).await;

        assert!(matches!(
            request(
                &mut owner,
                PartyRequest::PostMessage {
                    channel: private,
                    text: "members only".to_string(),
                },
            )
            .await,
            PartyResponse::MessagePosted { .. }
        ));

        // Bob is in the channel, so it reaches him.
        let bobs = drain(&mut bob).await;
        assert!(
            bobs.iter().any(|r| matches!(
                r,
                PartyResponse::Message(env)
                    if env.channel == private
                        && env.payload == MessagePayload::Text("members only".to_string())
            )),
            "a channel member must receive the message; got {bobs:?}"
        );

        // Carol is not, so nothing about it reaches her socket at all.
        let carols = drain(&mut carol).await;
        assert!(
            !carols
                .iter()
                .any(|r| matches!(r, PartyResponse::Message(_))),
            "a non-member received a private channel's message: {carols:?}"
        );

        drop(owner);
        drop(bob);
        drop(carol);
        owner_srv.abort();
        bob_srv.abort();
        carol_srv.abort();
    }

    /// A file too large for one frame goes up and comes back down over the real
    /// tunnel, byte for byte.
    ///
    /// The pieces were unit-tested separately — `PartyState` directly, and the
    /// client against a mock channel — but the whole path (offer, stream,
    /// commit, then page back down) had never run through framing, the
    /// dispatcher and the hub together.
    #[tokio::test]
    async fn a_chunked_upload_round_trips_over_the_tunnel() {
        let state = Arc::new(Mutex::new(PartyState::new("TestSrv", None)));
        let hub = Arc::new(Hub::new());
        let channel = state.lock().await.default_channel();
        let client_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let (mut client, server) = connect(
            generate_rsa_keypair(RSA_KEY_BITS).unwrap(),
            &client_priv,
            state.clone(),
            hub.clone(),
        )
        .await;
        join_as(&mut client, "owner").await;

        // Deliberately not a whole number of chunks, so the final short chunk is
        // exercised — the case an off-by-one gets wrong.
        let payload: Vec<u8> = (0..(PARTY_CHUNK_BYTES * 2 + 1234))
            .map(|i| (i % 251) as u8)
            .collect();

        let upload = match request(
            &mut client,
            PartyRequest::StartUpload {
                name: "big.bin".to_string(),
                mime: "application/octet-stream".to_string(),
                size: payload.len() as u64,
                target: UploadTarget::Channel(channel),
            },
        )
        .await
        {
            PartyResponse::UploadReady { upload, chunk_size } => {
                assert_eq!(chunk_size as usize, PARTY_CHUNK_BYTES);
                upload
            }
            other => panic!("expected UploadReady, got {other:?}"),
        };

        // Chunks are not individually acknowledged, so they stream without a
        // reply until the upload is committed.
        for part in payload.chunks(PARTY_CHUNK_BYTES) {
            client
                .send(&PartyRequest::UploadChunk {
                    upload,
                    data: part.to_vec(),
                })
                .await
                .unwrap();
        }
        assert!(matches!(
            request(&mut client, PartyRequest::FinishUpload { upload }).await,
            PartyResponse::MessagePosted { .. }
        ));

        // The upload became an ordinary file message in the channel.
        let hash = match request(
            &mut client,
            PartyRequest::FetchHistory {
                channel,
                since_seq: 0,
            },
        )
        .await
        {
            PartyResponse::History(items) => match &items[0].payload {
                MessagePayload::File(f) => {
                    assert_eq!(f.size, payload.len() as u64);
                    assert_eq!(f.name, "big.bin");
                    f.hash.clone()
                }
                other => panic!("expected a File payload, got {other:?}"),
            },
            other => panic!("expected History, got {other:?}"),
        };
        assert_eq!(
            hash,
            messenger_core::party::blob_hash(&payload),
            "the server stored it under the content hash of what was sent"
        );

        // …and pages back down, in order, matching byte for byte.
        let mut got: Vec<u8> = Vec::new();
        loop {
            match request(
                &mut client,
                PartyRequest::DownloadChunk {
                    hash: hash.clone(),
                    offset: got.len() as u64,
                },
            )
            .await
            {
                PartyResponse::FileChunk {
                    offset,
                    total,
                    data,
                    ..
                } => {
                    assert_eq!(offset, got.len() as u64, "chunks arrive in order");
                    assert_eq!(total, payload.len() as u64);
                    if data.is_empty() {
                        break;
                    }
                    got.extend_from_slice(&data);
                    if got.len() as u64 >= total {
                        break;
                    }
                }
                other => panic!("expected FileChunk, got {other:?}"),
            }
        }
        assert_eq!(got, payload);

        drop(client);
        server.abort();
    }

    /// When a member's last connection goes away, everyone still connected must
    /// be told. Nothing announced this before, so a member who left showed as
    /// online in every other client until something else happened to refresh the
    /// directory — which, since nothing refreshed it either, meant forever.
    #[tokio::test]
    async fn leaving_broadcasts_the_member_as_offline() {
        let server_priv_a = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let server_priv_b = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let alice_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let bob_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let state = Arc::new(Mutex::new(PartyState::new("TestSrv", None)));
        let hub = Arc::new(Hub::new());

        let (mut alice, alice_srv) =
            connect(server_priv_a, &alice_priv, state.clone(), hub.clone()).await;
        let (mut bob, bob_srv) =
            connect(server_priv_b, &bob_priv, state.clone(), hub.clone()).await;

        alice.join("alice", None).await.unwrap();
        bob.join("bob", None).await.unwrap();

        // Alice sees the directory push from Bob's join: both online.
        match alice.recv().await.unwrap() {
            PartyResponse::Members(members) => {
                assert_eq!(members.len(), 2);
                assert!(members.iter().all(|m| m.online), "both are connected");
            }
            other => panic!("expected a Members push after Bob joined, got {other:?}"),
        }

        // Bob's connection ends.
        drop(bob);
        bob_srv.await.ok();

        // Alice is told, without having asked.
        match alice.recv().await.unwrap() {
            PartyResponse::Members(members) => {
                let bob = members
                    .iter()
                    .find(|m| m.username == "bob")
                    .expect("bob is still a member, just offline");
                assert!(!bob.online, "bob disconnected and must show as offline");
                let alice_entry = members.iter().find(|m| m.username == "alice").unwrap();
                assert!(alice_entry.online, "alice is still connected");
            }
            other => panic!("expected an offline directory push, got {other:?}"),
        }

        drop(alice);
        alice_srv.abort();
    }
}
