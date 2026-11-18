//! Per-connection server runtime.
//!
//! Runs the v3 [`host_handshake`], then the Party loop: decrypt incoming
//! [`PartyRequest`]s, apply them via [`handle_request`] and reply, while writing
//! down the tunnel any [`PartyResponse`] another connection pushed through the
//! [`Hub`].

use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use messenger_core::network::host_handshake;
use messenger_core::party::{
    recv_framed, send_framed, FrameSeq, PartyRequest, PartyResponse, MAX_PARTY_PAYLOAD_BYTES,
};
use rsa::RsaPrivateKey;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

use crate::dispatch::{
    download_reply, handle_request, upload_committed, ConnState, Deferred, Dispatch,
};
use crate::hub::Hub;
use crate::state::PartyState;

/// Timeouts applied to one connection, so an unauthenticated peer cannot hold
/// a task and a socket indefinitely, nor a stuck reader hold up broadcasts.
#[derive(Debug, Clone)]
pub struct ConnectionLimits {
    /// Ceiling on the whole handshake (each read also has its own timeout), so
    /// a peer that trickles bytes cannot stretch it out.
    pub handshake_timeout: Duration,
    /// How long a connection may stay unjoined after its handshake (a client
    /// joins in its first frame).
    pub join_timeout: Duration,
    /// Ceiling on writing one frame; a client that stopped reading is
    /// disconnected rather than waited on.
    pub write_timeout: Duration,
}

impl Default for ConnectionLimits {
    fn default() -> Self {
        Self {
            handshake_timeout: Duration::from_secs(messenger_core::HANDSHAKE_TIMEOUT_SECS + 5),
            join_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
        }
    }
}

/// Write one frame, giving up after `timeout`.
async fn send_with_timeout<W>(
    wr: &mut W,
    cipher: &messenger_core::core::AesCipher,
    aad: &[u8],
    seq: &mut FrameSeq,
    bytes: &[u8],
    timeout: Duration,
) -> anyhow::Result<()>
where
    W: AsyncWrite + Unpin,
{
    match tokio::time::timeout(timeout, send_framed(wr, cipher, aad, seq, bytes)).await {
        Ok(sent) => sent,
        Err(_) => Err(anyhow!(
            "timed out after {timeout:?} writing to a client that stopped reading"
        )),
    }
}

/// Read one frame, taking ownership of the read half and the sequence counter
/// and handing both back, so the caller can hold one read future across
/// `select!` iterations (see the call site).
async fn read_frame<R>(
    mut rd: R,
    cipher: &messenger_core::core::AesCipher,
    aad: &[u8],
    mut seq: FrameSeq,
) -> (R, FrameSeq, anyhow::Result<Vec<u8>>)
where
    R: AsyncRead + Unpin,
{
    let result = recv_framed(&mut rd, cipher, aad, &mut seq).await;
    (rd, seq, result)
}

/// Encode a reply, answering with an error when it is too large for a frame:
/// a failed send would end the connection, and a client re-asking on
/// reconnect could never stay connected. Every page is budgeted to fit, so
/// this is defence in depth.
fn encode_reply(resp: &PartyResponse) -> Vec<u8> {
    encode_bounded(resp, "a reply").unwrap_or_else(|| {
        PartyResponse::Error("the server's reply was too large to send".to_string()).to_bytes()
    })
}

/// Encode a pushed broadcast, or `None` when it is too large for one frame:
/// dropping one live update (recovered from history) beats disconnecting every
/// recipient.
fn encode_push(resp: &PartyResponse) -> Option<Vec<u8>> {
    encode_bounded(resp, "a broadcast")
}

fn encode_bounded(resp: &PartyResponse, what: &str) -> Option<Vec<u8>> {
    let bytes = resp.to_bytes();
    if bytes.len() <= MAX_PARTY_PAYLOAD_BYTES {
        return Some(bytes);
    }
    tracing::error!(
        size = bytes.len(),
        limit = MAX_PARTY_PAYLOAD_BYTES,
        "{what} was too large to send"
    );
    None
}

/// Perform the I/O `handle_request` deferred, once the state lock is released,
/// so one member's 100 MiB transfer does not stall everyone else's messages.
///
/// `spawn_blocking` rather than `block_in_place`: plain file I/O borrowing
/// nothing from the state, and it works on a current-thread runtime (tests).
async fn perform_deferred(
    work: Deferred,
    state: &Mutex<PartyState>,
    conn: &mut ConnState,
) -> Dispatch {
    match work {
        Deferred::Download { hash, offset, plan } => {
            let total = plan.total();
            let bytes = tokio::task::spawn_blocking(move || plan.perform())
                .await
                .unwrap_or(None);
            download_reply(hash, offset, total, bytes)
        }
        Deferred::Upload(staged) => {
            let staged_id = staged.id();
            let member = conn.member().unwrap_or_else(Uuid::nil);
            // Phase 2: hash and write, off the lock.
            let written = tokio::task::spawn_blocking(move || {
                let mut staged = staged;
                let outcome = staged.write_staging();
                (staged, outcome)
            })
            .await;
            let out = match written {
                Ok((staged, Ok(()))) => {
                    // Phase 3: re-check and commit under the lock — only a
                    // rename and some rows, not the write.
                    let mut st = state.lock().await;
                    let committed = st.commit_upload(*staged);
                    upload_committed(&st, member, committed)
                }
                Ok((staged, Err(reason))) => {
                    state.lock().await.discard_upload(*staged);
                    Dispatch::action_failed(Uuid::nil(), reason)
                }
                Err(_) => {
                    // The blocking task panicked; only the reservation is left.
                    state
                        .lock()
                        .await
                        .cancel_connection_uploads(member, &[staged_id]);
                    Dispatch::action_failed(
                        Uuid::nil(),
                        "the server could not store this file".to_string(),
                    )
                }
            };
            conn.finish_upload(staged_id);
            out
        }
    }
}

/// Serve one client connection with the default [`ConnectionLimits`].
pub async fn serve_connection<S>(
    stream: &mut S,
    server_privkey: &RsaPrivateKey,
    state: Arc<Mutex<PartyState>>,
    hub: Arc<Hub>,
) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    serve_connection_with(
        stream,
        server_privkey,
        state,
        hub,
        &ConnectionLimits::default(),
    )
    .await
}

/// Serve one client connection to completion (until the peer disconnects, a
/// frame fails to authenticate, a timeout in `limits` expires, or the hub
/// evicts it). The client's verified fingerprint is bound to its membership on
/// join, after which the connection is registered with `hub` for broadcasts.
pub async fn serve_connection_with<S>(
    stream: &mut S,
    server_privkey: &RsaPrivateKey,
    state: Arc<Mutex<PartyState>>,
    hub: Arc<Hub>,
    limits: &ConnectionLimits,
) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let tunnel = tokio::time::timeout(
        limits.handshake_timeout,
        host_handshake(stream, server_privkey, Uuid::new_v4()),
    )
    .await
    .map_err(|_| {
        anyhow!(
            "handshake did not complete within {:?}",
            limits.handshake_timeout
        )
    })??;
    let cipher = tunnel.cipher;
    let aad = tunnel.transport_aad;
    let conn_id = Uuid::new_v4();
    let write_timeout = limits.write_timeout;
    // Bounded (see `hub::BROADCAST_QUEUE_DEPTH`), and the sender is *moved*
    // into the hub on registration, never cloned: with the hub holding the only
    // sender, eviction closes the lane, `recv` returns `None`, and the loop ends
    // so the client reconnects and catches up — instead of staying connected
    // but deaf.
    let (out_tx, mut out_rx) = mpsc::channel::<PartyResponse>(crate::hub::BROADCAST_QUEUE_DEPTH);
    let mut out_tx = Some(out_tx);
    let mut conn = ConnState::with_fingerprint(tunnel.peer_fingerprint);
    let mut registered = false;
    // Per-direction frame counters (inside the AEAD) reject replayed frames.
    let mut send_seq = FrameSeq::new();
    let recv_seq = FrameSeq::new();

    let (rd, mut wr) = tokio::io::split(&mut *stream);

    // The in-flight read, kept alive ACROSS loop iterations. `select!` drops
    // the losing branch's future and `recv_framed` is not cancel-safe (length
    // prefix, then body): a broadcast arriving mid-frame used to discard the
    // bytes read so far and desynchronise the stream. Holding one future means
    // a cancelled poll resumes instead.
    let mut pending = Box::pin(read_frame(rd, &cipher, &aad, recv_seq));

    // The pre-join deadline: a `Sleep` polled by reference is cancel-safe.
    let join_deadline = tokio::time::sleep(limits.join_timeout);
    tokio::pin!(join_deadline);

    // How the connection ended. A failed write is recorded rather than
    // returned with `?`, so the teardown below still runs (unregister, go
    // offline, release reservations).
    let mut served: anyhow::Result<()> = Ok(());

    loop {
        // Handed back when a frame completes, to start the next read.
        let mut finished = None;
        let awaiting_join = conn.member().is_none();

        tokio::select! {
            // A broadcast pushed to us by another connection. `recv` IS
            // cancel-safe: a message is either taken from the queue or left in it.
            push = out_rx.recv() => {
                let Some(push) = push else {
                    // Only the hub holds the sender: this is an eviction.
                    tracing::warn!(conn = %conn_id, "evicted for not reading broadcasts; closing so the client reconnects");
                    served = Err(anyhow!("evicted: the client stopped reading broadcasts"));
                    break;
                };
                let Some(bytes) = encode_push(&push) else { continue };
                if let Err(e) = send_with_timeout(&mut wr, &cipher, &aad, &mut send_seq, &bytes, write_timeout).await {
                    served = Err(e);
                    break;
                }
            }
            // Still not joined: a probe or a socket being held open.
            () = &mut join_deadline, if awaiting_join => {
                served = Err(anyhow!("did not join within {:?}", limits.join_timeout));
                break;
            }
            // An incoming request from this client.
            (rd_back, seq_back, incoming) = &mut pending => {
                finished = Some((rd_back, seq_back));

                let req_bytes = match incoming {
                    Ok(bytes) => bytes,
                    Err(_) => break, // peer closed, or a frame failed to authenticate
                };

                let mut outcome = match PartyRequest::from_bytes(&req_bytes) {
                    Some(req) => {
                        let mut st = state.lock().await;
                        handle_request(&mut st, &mut conn, req)
                    }
                    None => Dispatch::reply(PartyResponse::Error("malformed request".to_string())),
                };

                // The lock is released; move any real bytes now.
                if let Some(work) = outcome.deferred.take() {
                    let done = perform_deferred(work, &state, &mut conn).await;
                    outcome.replies.extend(done.replies);
                    outcome.broadcast.extend(done.broadcast);
                    outcome.directed.extend(done.directed);
                }

                // Register for broadcasts/DMs once this connection has joined.
                if !registered {
                    if let Some(member) = conn.member() {
                        let tx = out_tx.take().expect("registered at most once");
                        hub.register(conn_id, member, tx);
                        registered = true;
                    }
                }

                for resp in outcome.replies {
                    if let Err(e) = send_with_timeout(&mut wr, &cipher, &aad, &mut send_seq, &encode_reply(&resp), write_timeout).await {
                        served = Err(e);
                        break;
                    }
                }
                for resp in outcome.broadcast {
                    hub.broadcast_except(conn_id, resp);
                }
                for (member, resp) in outcome.directed {
                    // Excluding this connection reaches the sender's *other*
                    // devices without duplicating the optimistic row here.
                    hub.send_to_member_except(member, conn_id, resp);
                }
                // Fanned out first, so what was stored reaches everyone else
                // even when this client's own reply could not be written.
                if served.is_err() {
                    break;
                }
            }
        }

        if let Some((rd_back, seq_back)) = finished {
            pending = Box::pin(read_frame(rd_back, &cipher, &aad, seq_back));
        }
    }

    hub.unregister(conn_id);
    if let Some(member) = conn.member() {
        // Release this connection's uploads (not the member's: their other
        // devices may have transfers in flight). Every path that ends an upload
        // clears its id, so what is left here is exactly what must be released.
        let open = conn.open_uploads().to_vec();
        if !open.is_empty() {
            state.lock().await.cancel_connection_uploads(member, &open);
        }
        // Go offline only when the member's *last* connection is gone.
        if !hub.member_is_connected(member) {
            let directory = {
                let mut st = state.lock().await;
                st.set_online(member, false);
                st.members()
            };
            // Tell everyone still connected.
            hub.broadcast_all(PartyResponse::Members(directory));
        }
    }
    served
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
    use std::time::Duration;
    use tokio::io::DuplexStream;

    type TestClient = PartyClient<DuplexStream>;
    type Served = tokio::task::JoinHandle<anyhow::Result<()>>;

    fn key() -> RsaPrivateKey {
        generate_rsa_keypair(RSA_KEY_BITS).unwrap()
    }

    fn fixture() -> (Arc<Mutex<PartyState>>, Arc<Hub>) {
        (
            Arc::new(Mutex::new(PartyState::new("TestSrv", None))),
            Arc::new(Hub::new()),
        )
    }

    /// Send a request and read the reply, skipping the unsolicited `Members` /
    /// `DirectoryChanged` pushes that joins and channel changes queue ahead of
    /// it (real clients apply both from one stream).
    async fn request(client: &mut TestClient, req: PartyRequest) -> PartyResponse {
        client.send(&req).await.unwrap();
        loop {
            match client.recv().await.unwrap() {
                PartyResponse::Members(_) | PartyResponse::DirectoryChanged => continue,
                other => return other,
            }
        }
    }

    async fn post(
        client: &mut TestClient,
        channel: Uuid,
        text: impl Into<String>,
    ) -> PartyResponse {
        let text = text.into();
        request(client, PartyRequest::PostMessage { channel, text }).await
    }

    /// Spawn `serve_connection_with` over a duplex of `buffer` bytes, handing
    /// back the client end and the server task.
    fn spawn_with_limits(
        state: Arc<Mutex<PartyState>>,
        hub: Arc<Hub>,
        buffer: usize,
        limits: ConnectionLimits,
    ) -> (DuplexStream, Served) {
        let (mut server_stream, client_stream) = tokio::io::duplex(buffer);
        let server_priv = key();
        let handle = tokio::spawn(async move {
            serve_connection_with(&mut server_stream, &server_priv, state, hub, &limits).await
        });
        (client_stream, handle)
    }

    async fn handshake(stream: DuplexStream) -> TestClient {
        PartyClient::connect(stream, &key(), Uuid::new_v4())
            .await
            .expect("client handshake")
    }

    /// A client over a duplex of `buffer` bytes; a tiny buffer forces the
    /// server to read each frame in pieces.
    async fn connect_with_buffer(
        state: &Arc<Mutex<PartyState>>,
        hub: &Arc<Hub>,
        buffer: usize,
    ) -> (TestClient, Served) {
        let (stream, served) = spawn_with_limits(
            state.clone(),
            hub.clone(),
            buffer,
            ConnectionLimits::default(),
        );
        (handshake(stream).await, served)
    }

    async fn connect(state: &Arc<Mutex<PartyState>>, hub: &Arc<Hub>) -> (TestClient, Served) {
        connect_with_buffer(state, hub, 1 << 16).await
    }

    /// Join and return the assigned member id.
    async fn join_as(client: &mut TestClient, name: &str) -> Uuid {
        match client.join(name, None).await.unwrap() {
            PartyResponse::Joined { member_id, .. } => member_id,
            other => panic!("expected Joined, got {other:?}"),
        }
    }

    /// Read whatever is already queued and stop when it goes quiet, so a later
    /// assertion sees only frames caused by what follows.
    async fn drain(client: &mut TestClient) -> Vec<PartyResponse> {
        let mut seen = Vec::new();
        while let Ok(Ok(resp)) =
            tokio::time::timeout(Duration::from_millis(200), client.recv()).await
        {
            seen.push(resp);
        }
        seen
    }

    async fn is_online(state: &Mutex<PartyState>, id: Uuid) -> bool {
        state
            .lock()
            .await
            .members()
            .iter()
            .any(|m| m.id == id && m.online)
    }

    fn short_limits() -> ConnectionLimits {
        let t = Duration::from_millis(500);
        ConnectionLimits {
            handshake_timeout: t,
            join_timeout: t,
            write_timeout: t,
        }
    }

    /// A server-side stream that, once `cut`, never yields another byte and
    /// refuses every write: a client gone in the way that leaves the server's
    /// read pending, so it only finds out by failing to write.
    struct Severable {
        inner: DuplexStream,
        cut: Arc<std::sync::atomic::AtomicBool>,
    }

    impl Severable {
        fn is_cut(&self) -> bool {
            self.cut.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl tokio::io::AsyncRead for Severable {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            if self.is_cut() {
                return std::task::Poll::Pending;
            }
            std::pin::Pin::new(&mut self.inner).poll_read(cx, buf)
        }
    }

    impl tokio::io::AsyncWrite for Severable {
        fn poll_write(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            if self.is_cut() {
                return std::task::Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
            }
            std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
        }

        fn poll_flush(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::pin::Pin::new(&mut self.inner).poll_flush(cx)
        }

        fn poll_shutdown(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
        }
    }

    /// A client that vanishes while the server is writing to it must still be
    /// torn down: unregistered from the hub, marked offline, its reservations
    /// released (a failed write used to `?` straight past all of that).
    #[tokio::test]
    async fn a_client_lost_during_a_write_is_still_torn_down() {
        let (state, hub) = fixture();
        let channel = state.lock().await.default_channel();

        let cut = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (server_stream, client_stream) = tokio::io::duplex(1 << 16);
        let mut server_stream = Severable {
            inner: server_stream,
            cut: cut.clone(),
        };
        let server_priv = key();
        let (st, h) = (state.clone(), hub.clone());
        let served =
            tokio::spawn(
                async move { serve_connection(&mut server_stream, &server_priv, st, h).await },
            );
        let mut alice = handshake(client_stream).await;
        let alice_id = join_as(&mut alice, "alice").await;

        let (mut bob, _b) = connect(&state, &hub).await;
        join_as(&mut bob, "bob").await;
        assert!(hub.member_is_connected(alice_id));

        // Alice's link dies; the next thing the server writes to her fails.
        cut.store(true, std::sync::atomic::Ordering::SeqCst);
        let reply = post(&mut bob, channel, "anyone there?").await;
        assert!(matches!(reply, PartyResponse::MessagePosted { .. }));

        let outcome = tokio::time::timeout(Duration::from_secs(30), served)
            .await
            .expect("the connection task ends")
            .unwrap();
        assert!(outcome.is_err(), "the failed write is still reported");
        assert!(
            !hub.member_is_connected(alice_id),
            "the dead connection is still registered"
        );
        assert!(
            !is_online(&state, alice_id).await,
            "alice is still shown online"
        );
    }

    /// A broadcast arriving while this client is mid-frame must not cost it the
    /// connection (`recv_framed` is not cancel-safe; see `serve_connection_with`).
    /// A tiny duplex buffer makes the big post take many reads while another
    /// member broadcasts throughout.
    #[tokio::test]
    async fn a_broadcast_arriving_mid_frame_does_not_break_the_connection() {
        let (state, hub) = fixture();
        let channel = state.lock().await.default_channel();

        let (mut alice, _a) = connect(&state, &hub).await;
        join_as(&mut alice, "alice").await;
        // 64 bytes: far smaller than one frame, so every frame is split.
        let (mut bob, _b) = connect_with_buffer(&state, &hub, 64).await;
        join_as(&mut bob, "bob").await;

        let (mut bob_rd, mut bob_wr) = bob.split();

        // Bob's reader runs as its own task, as a real client's does; reading
        // only after the write would deadlock the tiny duplex instead.
        let (seen_tx, mut seen_rx) = mpsc::unbounded_channel();
        let reader = tokio::spawn(async move {
            loop {
                match bob_rd.recv().await {
                    Ok(resp) => {
                        if seen_tx.send(Ok(resp)).is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        let _ = seen_tx.send(Err(e.to_string()));
                        return;
                    }
                }
            }
        });

        let big = "x".repeat(32 * 1024);
        let writer = tokio::spawn(async move {
            bob_wr
                .send(&PartyRequest::PostMessage { channel, text: big })
                .await
                .expect("bob's post is written");
        });

        // …while alice keeps broadcasting into it.
        for i in 0..10 {
            let _ = post(&mut alice, channel, format!("alice {i}")).await;
            tokio::task::yield_now().await;
        }
        writer.await.expect("writer task");

        // Bob's own post must still be acknowledged.
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                match seen_rx.recv().await {
                    Some(Ok(PartyResponse::MessagePosted { channel: c, .. })) if c == channel => {
                        return
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => panic!("bob's connection broke: {e}"),
                    None => panic!("bob's reader stopped"),
                }
            }
        })
        .await
        .expect("bob's post must be acknowledged");
        reader.abort();
    }

    #[tokio::test]
    async fn client_joins_posts_and_fetches_history_over_the_tunnel() {
        let (state, hub) = fixture();
        let channel = state.lock().await.default_channel();
        let (mut client, server) = connect(&state, &hub).await;
        join_as(&mut client, "alice").await;

        match post(&mut client, channel, "hello server").await {
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
        server.abort();
    }

    #[tokio::test]
    async fn requests_before_join_are_rejected_over_the_tunnel() {
        let (state, hub) = fixture();
        let (mut client, server) = connect(&state, &hub).await;
        match request(&mut client, PartyRequest::ListMembers).await {
            PartyResponse::Error(msg) => assert_eq!(msg, "join required"),
            other => panic!("expected join-required error, got {other:?}"),
        }
        server.abort();
    }

    #[tokio::test]
    async fn posted_message_is_broadcast_to_other_members() {
        let (state, hub) = fixture();
        let channel = state.lock().await.default_channel();
        let (mut alice, alice_srv) = connect(&state, &hub).await;
        let (mut bob, bob_srv) = connect(&state, &hub).await;
        join_as(&mut alice, "alice").await;
        join_as(&mut bob, "bob").await;
        // Bob is registered once his join is acknowledged.
        assert_eq!(hub.len(), 2);

        // Alice posts; she gets the ack, Bob receives the broadcast live.
        let ack = post(&mut alice, channel, "hi bob").await;
        assert!(matches!(ack, PartyResponse::MessagePosted { .. }));
        match bob.recv().await.unwrap() {
            PartyResponse::Message(env) => {
                assert_eq!(env.channel, channel);
                assert_eq!(env.payload, MessagePayload::Text("hi bob".to_string()));
            }
            other => panic!("expected broadcast Message, got {other:?}"),
        }
        alice_srv.abort();
        bob_srv.abort();
    }

    #[tokio::test]
    async fn direct_message_is_delivered_and_fetchable() {
        let (state, hub) = fixture();
        let (mut alice, alice_srv) = connect(&state, &hub).await;
        let (mut bob, bob_srv) = connect(&state, &hub).await;
        let alice_id = join_as(&mut alice, "alice").await;
        let bob_id = join_as(&mut bob, "bob").await;

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
        let history = request(
            &mut bob,
            PartyRequest::FetchDmHistory {
                with: alice_id,
                since_seq: 0,
            },
        )
        .await;
        match history {
            PartyResponse::History(items) => {
                assert_eq!(items.len(), 1);
                assert_eq!(
                    items[0].payload,
                    MessagePayload::Text("hey bob".to_string())
                );
            }
            other => panic!("expected DM history, got {other:?}"),
        }
        alice_srv.abort();
        bob_srv.abort();
    }

    /// A private channel's messages must not reach a non-member *over the
    /// wire*, not merely in the dispatcher's return value: the leak lived in the
    /// hub, which pushed every message to every registered connection.
    #[tokio::test]
    async fn a_private_channels_messages_never_reach_a_non_member_over_the_tunnel() {
        let (state, hub) = fixture();
        let (mut owner, owner_srv) = connect(&state, &hub).await;
        let (mut bob, bob_srv) = connect(&state, &hub).await;
        let (mut carol, carol_srv) = connect(&state, &hub).await;

        // The owner (who joins with the claim token) can make a private
        // channel; the other two are ordinary members.
        let token = state.lock().await.owner_claim_token().unwrap().to_string();
        match owner.join("owner", Some(token)).await.unwrap() {
            PartyResponse::Joined { .. } => {}
            other => panic!("expected Joined, got {other:?}"),
        }
        let bob_id = join_as(&mut bob, "bob").await;
        join_as(&mut carol, "carol").await;

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
            post(&mut owner, private, "members only").await,
            PartyResponse::MessagePosted { .. }
        ));

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
        let carols = drain(&mut carol).await;
        assert!(
            !carols
                .iter()
                .any(|r| matches!(r, PartyResponse::Message(_))),
            "a non-member received a private channel's message: {carols:?}"
        );
        owner_srv.abort();
        bob_srv.abort();
        carol_srv.abort();
    }

    /// A file too large for one frame goes up and comes back down over the real
    /// tunnel (framing, dispatcher and hub together), byte for byte.
    #[tokio::test]
    async fn a_chunked_upload_round_trips_over_the_tunnel() {
        let (state, hub) = fixture();
        let channel = state.lock().await.default_channel();
        let (mut client, server) = connect(&state, &hub).await;
        join_as(&mut client, "owner").await;

        // Not a whole number of chunks, so the short final chunk is exercised.
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

        // Chunks are not individually acknowledged.
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
            let chunk = PartyRequest::DownloadChunk {
                hash: hash.clone(),
                offset: got.len() as u64,
            };
            match request(&mut client, chunk).await {
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
        server.abort();
    }

    #[test]
    fn an_oversized_reply_becomes_an_error_and_an_oversized_push_is_dropped() {
        let huge = PartyResponse::Error("x".repeat(MAX_PARTY_PAYLOAD_BYTES));
        let bytes = encode_reply(&huge);
        assert!(bytes.len() <= MAX_PARTY_PAYLOAD_BYTES);
        assert!(matches!(
            PartyResponse::from_bytes(&bytes),
            Some(PartyResponse::Error(ref m)) if m.contains("too large")
        ));
        assert!(encode_push(&huge).is_none());

        let small = PartyResponse::Ok("fine".to_string());
        assert_eq!(encode_reply(&small), small.to_bytes());
        assert_eq!(encode_push(&small), Some(small.to_bytes()));
    }

    /// When a member's last connection goes away, everyone still connected is
    /// told, without having asked.
    #[tokio::test]
    async fn leaving_broadcasts_the_member_as_offline() {
        let (state, hub) = fixture();
        let (mut alice, alice_srv) = connect(&state, &hub).await;
        let (mut bob, bob_srv) = connect(&state, &hub).await;
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

        drop(bob);
        bob_srv.await.ok();

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
        alice_srv.abort();
    }

    /// SRV-02: eviction must close the connection so the client reconnects,
    /// rather than leave it answering requests but deaf to every broadcast.
    #[tokio::test]
    async fn an_evicted_connection_is_closed_rather_than_left_deaf() {
        let (state, hub) = fixture();
        let (mut alice, served) = connect(&state, &hub).await;
        let alice_id = join_as(&mut alice, "alice").await;
        // Registration happens after the reply is written; wait for it.
        tokio::time::timeout(Duration::from_secs(10), async {
            while !hub.member_is_connected(alice_id) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("alice registers");

        // On this current-thread runtime the connection task cannot run while
        // these pushes happen, so its queue fills and the hub evicts it.
        for _ in 0..=crate::hub::BROADCAST_QUEUE_DEPTH {
            hub.broadcast_all(PartyResponse::DirectoryChanged);
        }
        assert!(!hub.member_is_connected(alice_id), "evicted from the hub");

        // The client drains what was queued and then sees the connection end.
        let ended = tokio::time::timeout(Duration::from_secs(30), async {
            while alice.recv().await.is_ok() {}
        })
        .await;
        assert!(ended.is_ok(), "the evicted connection must be closed");
        let outcome = tokio::time::timeout(Duration::from_secs(10), served)
            .await
            .expect("the connection task ends")
            .unwrap();
        assert!(outcome.unwrap_err().to_string().contains("evicted"));
        // Torn down like any other disconnect: shown offline.
        assert!(!is_online(&state, alice_id).await);
    }

    /// SRV-03: a connection that completed its handshake could sit unjoined
    /// forever, holding a task and a file descriptor.
    #[tokio::test]
    async fn a_connection_that_never_joins_is_closed() {
        let (state, hub) = fixture();
        let (stream, served) = spawn_with_limits(state, hub, 1 << 16, short_limits());
        let mut client = handshake(stream).await;
        let outcome = tokio::time::timeout(Duration::from_secs(30), served)
            .await
            .expect("closed after the join deadline")
            .unwrap();
        assert!(outcome.unwrap_err().to_string().contains("did not join"));
        assert!(client.recv().await.is_err(), "the client sees it close");
    }

    /// A joined connection is not subject to the join deadline.
    #[tokio::test]
    async fn a_joined_connection_outlives_the_join_deadline() {
        let (state, hub) = fixture();
        let (stream, served) = spawn_with_limits(state, hub, 1 << 16, short_limits());
        let mut client = handshake(stream).await;
        join_as(&mut client, "alice").await;
        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert!(!served.is_finished(), "joined members may idle");
        assert!(matches!(
            request(&mut client, PartyRequest::ListChannels).await,
            PartyResponse::Channels(_)
        ));
        served.abort();
    }

    /// A peer that opens a socket and never speaks is dropped at the handshake
    /// deadline rather than held.
    #[tokio::test]
    async fn a_peer_that_never_handshakes_is_closed() {
        let (state, hub) = fixture();
        let (_silent, served) = spawn_with_limits(state, hub, 1 << 16, short_limits());
        let outcome = tokio::time::timeout(Duration::from_secs(30), served)
            .await
            .expect("closed at the handshake deadline")
            .unwrap();
        assert!(outcome.is_err());
    }

    /// SRV-03: a write to a client that stopped reading times out and the
    /// connection is torn down.
    #[tokio::test]
    async fn a_client_that_stops_reading_is_disconnected_by_the_write_timeout() {
        let (state, hub) = fixture();
        state.lock().await.set_write_rate_limit(None);
        let channel = state.lock().await.default_channel();

        // A small pipe, so a handful of broadcasts fills it.
        let (stream, served) =
            spawn_with_limits(state.clone(), hub.clone(), 4 * 1024, short_limits());
        let mut alice = handshake(stream).await;
        let alice_id = join_as(&mut alice, "alice").await;
        let (mut bob, _b) = connect(&state, &hub).await;
        join_as(&mut bob, "bob").await;
        // Alice never reads again. Far fewer broadcasts than the queue holds,
        // so it is the write timeout — not eviction — that has to end this.
        for i in 0..8 {
            let reply = post(&mut bob, channel, format!("{i}{}", "z".repeat(2048))).await;
            assert!(matches!(reply, PartyResponse::MessagePosted { .. }));
        }
        let outcome = tokio::time::timeout(Duration::from_secs(30), served)
            .await
            .expect("the stuck write times out")
            .unwrap();
        assert!(outcome.unwrap_err().to_string().contains("timed out"));
        assert!(!hub.member_is_connected(alice_id));
        drop(alice);
    }
}
