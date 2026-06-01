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
    dm_thread_id, ChannelInfo, Envelope, MemberInfo, MessagePayload, PartyRequest, PartyResponse,
    TrustTier,
};
use messenger_core::util::{current_timestamp_millis, parse_host_port};
use rsa::RsaPrivateKey;
use tokio::sync::mpsc;
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
    outgoing_tx: mpsc::UnboundedSender<PartyRequest>,
    incoming_rx: mpsc::UnboundedReceiver<Incoming>,
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
            outgoing_tx,
            incoming_rx,
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
                    Ok(Incoming::Disconnected) => conn.status = PartyStatus::Disconnected,
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        conn.status = PartyStatus::Disconnected;
                        break;
                    }
                }
            }
        }
    }

    pub fn server(&self, server_id: Uuid) -> Option<&PartyServerConn> {
        self.servers.get(&server_id)
    }

    pub fn server_ids(&self) -> Vec<Uuid> {
        self.servers.keys().copied().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
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
        PartyResponse::Error(_) => {}
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
            outgoing_tx,
            incoming_rx,
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
    fn post_before_join_fails() {
        let (mut mgr, id, _tx, _out) = manager_with_server();
        assert!(mgr.post(id, Uuid::new_v4(), "x".to_string()).is_err());
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
    fn last_error_set_and_cleared() {
        let mut mgr = PartyManager::new();
        assert!(mgr.last_error().is_none());
        mgr.set_last_error("connect failed".to_string());
        assert_eq!(mgr.last_error(), Some("connect failed"));
        mgr.clear_last_error();
        assert!(mgr.last_error().is_none());
    }
}
