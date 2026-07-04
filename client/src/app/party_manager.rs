//! Client-side Party manager (Phase 1, slice 4).
//!
//! The business-logic half of the Party tab: it owns the client's connections to
//! Party servers, runs a background read/write task per connection (mirroring the
//! P2P session model), and tracks per-server state — directory, channels, and
//! per-channel message history — that the GUI/TUI render. State updates are driven
//! by `poll_events`, polled from the UI loop just like `poll_session_events`.
//!
//! Message handling: the server broadcasts a post to every member *except* the
//! sender, so the manager appends the sender's own message optimistically when
//! `post` is called, and appends others' messages as `Message` broadcasts arrive.
//! A one-time `FetchHistory` on join seeds the channel from durable history.

use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Result};
use messenger_core::party::{
    blob_hash, dm_thread_id, ChannelInfo, Envelope, FileMeta, MemberInfo, MessagePayload,
    PartyRequest, PartyResponse, TrustTier, MAX_INLINE_FILE_BYTES,
};
use messenger_core::util::{current_timestamp_millis, parse_host_port};
use rsa::RsaPrivateKey;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

/// Connection lifecycle status for a joined server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartyStatus {
    Connecting,
    Joined,
    Rejected(String),
    Disconnected,
}

/// Internal events delivered from the connection task to the manager.
enum Incoming {
    Response(PartyResponse),
    Disconnected,
}

/// Per-server connection state shown in the Party tab.
pub struct PartyServerConn {
    pub address: String,
    pub server_name: String,
    pub server_fingerprint: String,
    pub member_id: Option<Uuid>,
    pub status: PartyStatus,
    pub channels: Vec<ChannelInfo>,
    pub members: Vec<MemberInfo>,
    /// Per-channel message history (delivery order).
    pub messages: HashMap<Uuid, Vec<Envelope>>,
    /// The most recent error this server replied with (e.g. a rejected post),
    /// surfaced in the UI until superseded. Without this, server errors were
    /// silently dropped and a failed action looked like it simply did nothing.
    pub last_error: Option<String>,
    outgoing_tx: mpsc::UnboundedSender<PartyRequest>,
    incoming_rx: mpsc::UnboundedReceiver<Incoming>,
    /// In-flight file downloads, keyed by content hash. A `DownloadFile` request
    /// registers a one-shot here; the matching `FileData` response (drained by
    /// `poll_events`) completes it with the bytes. A server `Error` (e.g. the file
    /// is gone or access is denied — the error is not hash-correlated on the wire)
    /// or a disconnect fails every in-flight download instead, so a caller awaiting
    /// the receiver never hangs forever.
    pending_downloads: HashMap<String, oneshot::Sender<Result<Vec<u8>, String>>>,
}

impl PartyServerConn {
    fn send(&self, req: PartyRequest) -> Result<()> {
        self.outgoing_tx
            .send(req)
            .map_err(|_| anyhow!("connection to server is closed"))
    }
}

/// Owns the client's Party-server connections.
#[derive(Default)]
pub struct PartyManager {
    servers: HashMap<Uuid, PartyServerConn>,
    /// Last connection error, surfaced in the UI until dismissed or superseded.
    last_error: Option<String>,
}

impl PartyManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a connection error (e.g. a failed handshake) for the UI to show.
    pub fn set_last_error(&mut self, message: String) {
        self.last_error = Some(message);
    }

    /// The last connection error, if any.
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Clear the last connection error (e.g. when the user dismisses it).
    pub fn clear_last_error(&mut self) {
        self.last_error = None;
    }

    /// Connect to a Party server, complete the v3 handshake, and queue a join plus
    /// an initial directory + history fetch. Returns the local server id. The
    /// server fingerprint is captured for TOFU display; the caller is responsible
    /// for surfacing it for verification.
    pub async fn connect_and_join(
        &mut self,
        address: &str,
        username: &str,
        password: Option<String>,
        privkey: &RsaPrivateKey,
    ) -> Result<Uuid> {
        use messenger_core::party::PartyClient;

        self.last_error = None;
        let (host, port) = parse_host_port(address, Some(messenger_core::PORT_DEFAULT))?;
        let stream = tokio::net::TcpStream::connect((host.as_str(), port)).await?;
        let client = PartyClient::connect(stream, privkey, Uuid::new_v4()).await?;
        let fingerprint = client.server_fingerprint().to_string();
        let (mut reader, mut writer) = client.split();

        let (outgoing_tx, mut outgoing_rx) = mpsc::unbounded_channel::<PartyRequest>();
        let (incoming_tx, incoming_rx) = mpsc::unbounded_channel::<Incoming>();

        // Connection task: pump outgoing requests and forward incoming responses.
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    outgoing = outgoing_rx.recv() => match outgoing {
                        Some(req) => {
                            if writer.send(&req).await.is_err() {
                                break;
                            }
                        }
                        None => break, // manager dropped this connection
                    },
                    incoming = reader.recv() => match incoming {
                        Ok(resp) => {
                            if incoming_tx.send(Incoming::Response(resp)).is_err() {
                                break;
                            }
                        }
                        Err(_) => break, // server closed or a frame failed to authenticate
                    },
                }
            }
            let _ = incoming_tx.send(Incoming::Disconnected);
        });

        let conn = PartyServerConn {
            address: address.to_string(),
            server_name: String::new(),
            server_fingerprint: fingerprint,
            member_id: None,
            status: PartyStatus::Connecting,
            channels: Vec::new(),
            members: Vec::new(),
            messages: HashMap::new(),
            last_error: None,
            outgoing_tx,
            incoming_rx,
            pending_downloads: HashMap::new(),
        };

        // Join, then load the directory.
        conn.send(PartyRequest::Join {
            username: username.to_string(),
            password,
        })?;
        conn.send(PartyRequest::ListChannels)?;
        conn.send(PartyRequest::ListMembers)?;

        let server_id = Uuid::new_v4();
        self.servers.insert(server_id, conn);
        Ok(server_id)
    }

    /// Post a message to a channel. The message is appended locally right away
    /// (the server won't echo the sender's own broadcast).
    pub fn post(&mut self, server_id: Uuid, channel: Uuid, text: String) -> Result<()> {
        let conn = self
            .servers
            .get_mut(&server_id)
            .ok_or_else(|| anyhow!("unknown server"))?;
        let sender = conn.member_id.ok_or_else(|| anyhow!("not joined yet"))?;
        conn.messages.entry(channel).or_default().push(Envelope {
            tier: TrustTier::Administered,
            sender,
            channel,
            seq: 0, // provisional; ordering is by arrival
            timestamp: current_timestamp_millis(),
            payload: MessagePayload::Text(text.clone()),
        });
        conn.send(PartyRequest::PostMessage { channel, text })
    }

    /// Send a direct message to another member. Stored locally under the (shared)
    /// DM thread id immediately; the server delivers it to the recipient.
    pub fn send_dm(&mut self, server_id: Uuid, to: Uuid, text: String) -> Result<()> {
        let conn = self
            .servers
            .get_mut(&server_id)
            .ok_or_else(|| anyhow!("unknown server"))?;
        let me = conn.member_id.ok_or_else(|| anyhow!("not joined yet"))?;
        let thread = dm_thread_id(me, to);
        conn.messages.entry(thread).or_default().push(Envelope {
            tier: TrustTier::Administered,
            sender: me,
            channel: thread,
            seq: 0,
            timestamp: current_timestamp_millis(),
            payload: MessagePayload::Text(text.clone()),
        });
        conn.send(PartyRequest::SendDm { to, text })
    }

    /// Request DM history with another member (offline catch-up).
    pub fn fetch_dm_history(&self, server_id: Uuid, with: Uuid) -> Result<()> {
        let conn = self
            .servers
            .get(&server_id)
            .ok_or_else(|| anyhow!("unknown server"))?;
        conn.send(PartyRequest::FetchDmHistory { with, since_seq: 0 })
    }

    /// Messages in the DM thread with `peer` on a given server (empty if none).
    pub fn dm_messages(&self, server_id: Uuid, peer: Uuid) -> Vec<Envelope> {
        self.servers
            .get(&server_id)
            .and_then(|conn| {
                conn.member_id.map(|me| {
                    conn.messages
                        .get(&dm_thread_id(me, peer))
                        .cloned()
                        .unwrap_or_default()
                })
            })
            .unwrap_or_default()
    }

    /// Create a new channel on the server. The server replies with the refreshed
    /// channel list, applied on the next poll.
    pub fn create_channel(&self, server_id: Uuid, name: String) -> Result<()> {
        let conn = self
            .servers
            .get(&server_id)
            .ok_or_else(|| anyhow!("unknown server"))?;
        conn.send(PartyRequest::CreateChannel { name })
    }

    /// Upload a file to a channel: append it optimistically (so the sender sees it
    /// immediately, since the server won't echo their own broadcast) and send a
    /// `PostFile`. `name`/`mime` describe the file; `data` is its bytes. Errors if
    /// the payload exceeds [`MAX_INLINE_FILE_BYTES`] (the server would reject it).
    pub fn send_file(
        &mut self,
        server_id: Uuid,
        channel: Uuid,
        name: String,
        mime: String,
        data: Vec<u8>,
    ) -> Result<()> {
        let conn = self
            .servers
            .get_mut(&server_id)
            .ok_or_else(|| anyhow!("unknown server"))?;
        let sender = conn.member_id.ok_or_else(|| anyhow!("not joined yet"))?;
        let meta = file_meta(&name, &mime, &data)?;
        conn.messages.entry(channel).or_default().push(Envelope {
            tier: TrustTier::Administered,
            sender,
            channel,
            seq: 0,
            timestamp: current_timestamp_millis(),
            payload: MessagePayload::File(meta),
        });
        conn.send(PartyRequest::PostFile {
            channel,
            name,
            mime,
            data,
        })
    }

    /// Upload a file as a direct message to another member. Mirrors [`send_file`]
    /// but stores under the shared DM thread id and sends `SendFileDm`.
    pub fn send_file_dm(
        &mut self,
        server_id: Uuid,
        to: Uuid,
        name: String,
        mime: String,
        data: Vec<u8>,
    ) -> Result<()> {
        let conn = self
            .servers
            .get_mut(&server_id)
            .ok_or_else(|| anyhow!("unknown server"))?;
        let me = conn.member_id.ok_or_else(|| anyhow!("not joined yet"))?;
        let thread = dm_thread_id(me, to);
        let meta = file_meta(&name, &mime, &data)?;
        conn.messages.entry(thread).or_default().push(Envelope {
            tier: TrustTier::Administered,
            sender: me,
            channel: thread,
            seq: 0,
            timestamp: current_timestamp_millis(),
            payload: MessagePayload::File(meta),
        });
        conn.send(PartyRequest::SendFileDm {
            to,
            name,
            mime,
            data,
        })
    }

    /// Request a stored file's bytes by content hash. Returns a receiver that
    /// resolves once the matching `FileData` response is drained by `poll_events`
    /// (or errors if the connection drops first, dropping the sender). The server
    /// only returns bytes the requester is allowed to see (access-checked there).
    pub fn request_download(
        &mut self,
        server_id: Uuid,
        hash: String,
    ) -> Result<oneshot::Receiver<Result<Vec<u8>, String>>> {
        let conn = self
            .servers
            .get_mut(&server_id)
            .ok_or_else(|| anyhow!("unknown server"))?;
        let (tx, rx) = oneshot::channel();
        conn.pending_downloads.insert(hash.clone(), tx);
        conn.send(PartyRequest::DownloadFile { hash })?;
        Ok(rx)
    }

    /// Request a channel's full history (offline catch-up).
    pub fn fetch_history(&self, server_id: Uuid, channel: Uuid) -> Result<()> {
        let conn = self
            .servers
            .get(&server_id)
            .ok_or_else(|| anyhow!("unknown server"))?;
        conn.send(PartyRequest::FetchHistory {
            channel,
            since_seq: 0,
        })
    }

    /// Drain incoming events from every connection and update state. Call this from
    /// the UI loop, like `poll_session_events`.
    pub fn poll_events(&mut self) {
        for conn in self.servers.values_mut() {
            loop {
                match conn.incoming_rx.try_recv() {
                    Ok(Incoming::Response(resp)) => apply(conn, resp),
                    Ok(Incoming::Disconnected) => {
                        conn.status = PartyStatus::Disconnected;
                        fail_pending_downloads(conn, "connection closed");
                    }
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        conn.status = PartyStatus::Disconnected;
                        fail_pending_downloads(conn, "connection closed");
                        break;
                    }
                }
            }
        }
    }

    pub fn server(&self, server_id: Uuid) -> Option<&PartyServerConn> {
        self.servers.get(&server_id)
    }

    /// Clear a server's last surfaced error (e.g. after the UI shows and the user
    /// dismisses a rejected-post banner). No-op for an unknown server.
    pub fn clear_server_error(&mut self, server_id: Uuid) {
        if let Some(conn) = self.servers.get_mut(&server_id) {
            conn.last_error = None;
        }
    }

    pub fn server_ids(&self) -> Vec<Uuid> {
        self.servers.keys().copied().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }
}

/// Build the [`FileMeta`] for an outgoing upload, computing its content hash and
/// rejecting payloads the server would refuse (`> MAX_INLINE_FILE_BYTES`) so the
/// user gets a clear error instead of a silent server-side rejection.
fn file_meta(name: &str, mime: &str, data: &[u8]) -> Result<FileMeta> {
    if data.len() > MAX_INLINE_FILE_BYTES {
        return Err(anyhow!(
            "file is too large to share here (max {} MiB)",
            MAX_INLINE_FILE_BYTES / (1024 * 1024)
        ));
    }
    Ok(FileMeta {
        hash: blob_hash(data),
        name: name.to_string(),
        size: data.len() as u64,
        mime: mime.to_string(),
    })
}

/// Fail every in-flight download on a connection with `reason`, so callers awaiting
/// a download don't hang when the server reports an error or the connection drops.
fn fail_pending_downloads(conn: &mut PartyServerConn, reason: &str) {
    for (_, tx) in conn.pending_downloads.drain() {
        let _ = tx.send(Err(reason.to_string()));
    }
}

/// Apply one server response to a connection's state.
fn apply(conn: &mut PartyServerConn, resp: PartyResponse) {
    match resp {
        PartyResponse::Joined {
            member_id,
            server_name,
            ..
        } => {
            conn.member_id = Some(member_id);
            conn.server_name = server_name;
            conn.status = PartyStatus::Joined;
            // Seed each known channel's history once we're in.
            let channels: Vec<Uuid> = conn.channels.iter().map(|c| c.id).collect();
            for ch in channels {
                let _ = conn.send(PartyRequest::FetchHistory {
                    channel: ch,
                    since_seq: 0,
                });
            }
        }
        PartyResponse::JoinRejected { reason } => conn.status = PartyStatus::Rejected(reason),
        PartyResponse::Members(members) => conn.members = members,
        PartyResponse::Channels(channels) => conn.channels = channels,
        PartyResponse::Message(env) => {
            conn.messages.entry(env.channel).or_default().push(env);
        }
        PartyResponse::History(items) => {
            // Replace each referenced channel's history with the authoritative set.
            let channels: HashSet<Uuid> = items.iter().map(|e| e.channel).collect();
            for ch in channels {
                let history: Vec<Envelope> =
                    items.iter().filter(|e| e.channel == ch).cloned().collect();
                conn.messages.insert(ch, history);
            }
        }
        // Ack carries no content; the optimistic local append already happened.
        PartyResponse::MessagePosted { .. } => {}
        // Downloaded file bytes: hand them to the awaiting `request_download` caller
        // (keyed by content hash). Unsolicited/duplicate data has no waiter and is
        // dropped.
        PartyResponse::FileData { hash, data } => {
            if let Some(tx) = conn.pending_downloads.remove(&hash) {
                let _ = tx.send(Ok(data));
            }
        }
        // Surface server-side failures (rejected post, unknown channel, …) so a
        // failed action is visible instead of silently doing nothing. A download
        // failure arrives as an `Error` too (the wire error is not hash-correlated),
        // so also fail any in-flight downloads rather than let their callers hang.
        PartyResponse::Error(message) => {
            fail_pending_downloads(conn, &message);
            conn.last_error = Some(message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use messenger_core::party::ChannelKind;

    /// Build a manager with one server connection wired to test channels, returning
    /// the server id, a sender to push server responses, and a receiver to observe
    /// the requests the manager emits.
    fn manager_with_server() -> (
        PartyManager,
        Uuid,
        mpsc::UnboundedSender<Incoming>,
        mpsc::UnboundedReceiver<PartyRequest>,
    ) {
        let (outgoing_tx, outgoing_rx) = mpsc::unbounded_channel();
        let (incoming_tx, incoming_rx) = mpsc::unbounded_channel();
        let conn = PartyServerConn {
            address: "127.0.0.1:12345".to_string(),
            server_name: String::new(),
            server_fingerprint: "FP".to_string(),
            member_id: None,
            status: PartyStatus::Connecting,
            channels: Vec::new(),
            members: Vec::new(),
            messages: HashMap::new(),
            last_error: None,
            outgoing_tx,
            incoming_rx,
            pending_downloads: HashMap::new(),
        };
        let mut mgr = PartyManager::new();
        let id = Uuid::new_v4();
        mgr.servers.insert(id, conn);
        (mgr, id, incoming_tx, outgoing_rx)
    }

    fn envelope(channel: Uuid, sender: Uuid, seq: u64, text: &str) -> Envelope {
        Envelope {
            tier: TrustTier::Administered,
            sender,
            channel,
            seq,
            timestamp: 1,
            payload: MessagePayload::Text(text.to_string()),
        }
    }

    #[test]
    fn poll_applies_join_directory_and_messages() {
        let (mut mgr, id, tx, _out) = manager_with_server();
        let me = Uuid::new_v4();
        let channel = Uuid::new_v4();

        tx.send(Incoming::Response(PartyResponse::Channels(vec![
            ChannelInfo {
                id: channel,
                name: "general".to_string(),
                kind: ChannelKind::Public,
            },
        ])))
        .unwrap();
        tx.send(Incoming::Response(PartyResponse::Joined {
            member_id: me,
            server_name: "Srv".to_string(),
            tier: TrustTier::Administered,
        }))
        .unwrap();
        tx.send(Incoming::Response(PartyResponse::Members(vec![
            MemberInfo {
                id: me,
                username: "alice".to_string(),
                online: true,
            },
        ])))
        .unwrap();
        tx.send(Incoming::Response(PartyResponse::Message(envelope(
            channel, me, 1, "hi",
        ))))
        .unwrap();

        mgr.poll_events();

        let conn = mgr.server(id).unwrap();
        assert_eq!(conn.status, PartyStatus::Joined);
        assert_eq!(conn.member_id, Some(me));
        assert_eq!(conn.server_name, "Srv");
        assert_eq!(conn.channels.len(), 1);
        assert_eq!(conn.members.len(), 1);
        assert_eq!(conn.messages.get(&channel).map(|m| m.len()), Some(1));
    }

    #[test]
    fn join_rejection_is_surfaced() {
        let (mut mgr, id, tx, _out) = manager_with_server();
        tx.send(Incoming::Response(PartyResponse::JoinRejected {
            reason: "wrong password".to_string(),
        }))
        .unwrap();
        mgr.poll_events();
        assert_eq!(
            mgr.server(id).unwrap().status,
            PartyStatus::Rejected("wrong password".to_string())
        );
    }

    #[test]
    fn post_appends_locally_and_emits_request() {
        let (mut mgr, id, _tx, mut out) = manager_with_server();
        // Pretend we've joined.
        let me = Uuid::new_v4();
        mgr.servers.get_mut(&id).unwrap().member_id = Some(me);
        let channel = Uuid::new_v4();

        mgr.post(id, channel, "hello".to_string()).unwrap();

        // Local optimistic append.
        let conn = mgr.server(id).unwrap();
        let msgs = conn.messages.get(&channel).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].payload, MessagePayload::Text("hello".to_string()));

        // The PostMessage request was emitted to the connection.
        match out.try_recv().unwrap() {
            PartyRequest::PostMessage { channel: c, text } => {
                assert_eq!(c, channel);
                assert_eq!(text, "hello");
            }
            other => panic!("expected PostMessage, got {other:?}"),
        }
    }

    #[test]
    fn send_file_appends_optimistically_and_emits_postfile() {
        let (mut mgr, id, _tx, mut out) = manager_with_server();
        let me = Uuid::new_v4();
        mgr.servers.get_mut(&id).unwrap().member_id = Some(me);
        let channel = Uuid::new_v4();
        let data = b"\x89PNG fake image bytes".to_vec();

        mgr.send_file(
            id,
            channel,
            "photo.png".to_string(),
            "image/png".to_string(),
            data.clone(),
        )
        .unwrap();

        // Optimistic local append is a File message with the content hash/size.
        let msgs = mgr.server(id).unwrap().messages.get(&channel).unwrap();
        assert_eq!(msgs.len(), 1);
        match &msgs[0].payload {
            MessagePayload::File(f) => {
                assert_eq!(f.name, "photo.png");
                assert_eq!(f.size, data.len() as u64);
                assert_eq!(f.hash, blob_hash(&data));
            }
            other => panic!("expected a File payload, got {other:?}"),
        }

        // The PostFile request carrying the bytes was emitted.
        match out.try_recv().unwrap() {
            PartyRequest::PostFile {
                channel: c,
                name,
                data: sent,
                ..
            } => {
                assert_eq!(c, channel);
                assert_eq!(name, "photo.png");
                assert_eq!(sent, data);
            }
            other => panic!("expected PostFile, got {other:?}"),
        }
    }

    #[test]
    fn request_download_resolves_when_filedata_arrives() {
        let (mut mgr, id, tx, mut out) = manager_with_server();
        mgr.servers.get_mut(&id).unwrap().member_id = Some(Uuid::new_v4());
        let data = b"downloaded file contents".to_vec();
        let hash = blob_hash(&data);

        let mut rx = mgr.request_download(id, hash.clone()).unwrap();
        // The DownloadFile request was emitted for the right hash.
        match out.try_recv().unwrap() {
            PartyRequest::DownloadFile { hash: h } => assert_eq!(h, hash),
            other => panic!("expected DownloadFile, got {other:?}"),
        }
        // Before the response, the receiver is still pending.
        assert!(rx.try_recv().is_err());

        // The server replies with the bytes; poll_events routes them to the waiter.
        tx.send(Incoming::Response(PartyResponse::FileData {
            hash: hash.clone(),
            data: data.clone(),
        }))
        .unwrap();
        mgr.poll_events();

        assert_eq!(rx.try_recv().unwrap(), Ok(data));
    }

    #[test]
    fn request_download_fails_when_the_server_reports_an_error() {
        let (mut mgr, id, tx, _out) = manager_with_server();
        mgr.servers.get_mut(&id).unwrap().member_id = Some(Uuid::new_v4());

        let mut rx = mgr
            .request_download(id, "deadbeef".to_string())
            .unwrap();

        // The server can't serve it (gone / not permitted); a download error is not
        // hash-correlated on the wire, so any in-flight download is failed.
        tx.send(Incoming::Response(PartyResponse::Error(
            "unknown file".to_string(),
        )))
        .unwrap();
        mgr.poll_events();

        // The caller gets an error instead of hanging forever.
        assert_eq!(rx.try_recv().unwrap(), Err("unknown file".to_string()));
    }

    #[test]
    fn oversized_file_upload_is_rejected_without_side_effects() {
        let (mut mgr, id, _tx, mut out) = manager_with_server();
        let me = Uuid::new_v4();
        mgr.servers.get_mut(&id).unwrap().member_id = Some(me);
        let channel = Uuid::new_v4();
        let too_big = vec![0u8; MAX_INLINE_FILE_BYTES + 1];

        assert!(
            mgr.send_file(id, channel, "big.bin".to_string(), "application/octet-stream".to_string(), too_big)
                .is_err(),
            "an oversized upload must be rejected"
        );
        // Nothing was appended locally and no request was emitted.
        assert!(!mgr.server(id).unwrap().messages.contains_key(&channel));
        assert!(out.try_recv().is_err());
    }

    #[test]
    fn post_before_join_fails() {
        let (mut mgr, id, _tx, _out) = manager_with_server();
        assert!(mgr.post(id, Uuid::new_v4(), "x".to_string()).is_err());
    }

    #[test]
    fn create_channel_emits_request() {
        let (mgr, id, _tx, mut out) = manager_with_server();
        mgr.create_channel(id, "random".to_string()).unwrap();
        match out.try_recv().unwrap() {
            PartyRequest::CreateChannel { name } => assert_eq!(name, "random"),
            other => panic!("expected CreateChannel, got {other:?}"),
        }
    }

    #[test]
    fn send_dm_appends_locally_and_emits_request() {
        let (mut mgr, id, _tx, mut out) = manager_with_server();
        let me = Uuid::new_v4();
        mgr.servers.get_mut(&id).unwrap().member_id = Some(me);
        let peer = Uuid::new_v4();

        mgr.send_dm(id, peer, "hi".to_string()).unwrap();

        let dm = mgr.dm_messages(id, peer);
        assert_eq!(dm.len(), 1);
        assert_eq!(dm[0].payload, MessagePayload::Text("hi".to_string()));

        match out.try_recv().unwrap() {
            PartyRequest::SendDm { to, text } => {
                assert_eq!(to, peer);
                assert_eq!(text, "hi");
            }
            other => panic!("expected SendDm, got {other:?}"),
        }
    }

    #[test]
    fn history_replaces_channel_messages() {
        let (mut mgr, id, tx, _out) = manager_with_server();
        let me = Uuid::new_v4();
        let channel = Uuid::new_v4();
        tx.send(Incoming::Response(PartyResponse::History(vec![
            envelope(channel, me, 1, "one"),
            envelope(channel, me, 2, "two"),
        ])))
        .unwrap();
        mgr.poll_events();
        assert_eq!(
            mgr.server(id)
                .unwrap()
                .messages
                .get(&channel)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn disconnect_marks_status() {
        let (mut mgr, id, tx, _out) = manager_with_server();
        tx.send(Incoming::Disconnected).unwrap();
        mgr.poll_events();
        assert_eq!(mgr.server(id).unwrap().status, PartyStatus::Disconnected);
    }

    #[test]
    fn server_error_response_is_surfaced_not_dropped() {
        let (mut mgr, id, tx, _out) = manager_with_server();
        tx.send(Incoming::Response(PartyResponse::Error(
            "channel is locked".to_string(),
        )))
        .unwrap();
        mgr.poll_events();
        assert_eq!(
            mgr.server(id).unwrap().last_error.as_deref(),
            Some("channel is locked"),
            "a server Error reply must surface, not be silently dropped"
        );

        // The UI can dismiss it once shown.
        mgr.clear_server_error(id);
        assert!(
            mgr.server(id).unwrap().last_error.is_none(),
            "a dismissed server error must be cleared"
        );
        // Clearing an unknown server is a harmless no-op.
        mgr.clear_server_error(Uuid::new_v4());
    }

    #[test]
    fn last_error_set_and_cleared() {
        let mut mgr = PartyManager::new();
        assert!(mgr.last_error().is_none());
        mgr.set_last_error("connect failed".to_string());
        assert_eq!(mgr.last_error(), Some("connect failed"));
        mgr.clear_last_error();
        assert!(mgr.last_error().is_none());
    }
}
