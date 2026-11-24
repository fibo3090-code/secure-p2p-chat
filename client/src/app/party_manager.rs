//! Client-side Party manager: owns the connections to community servers (one
//! background task each), and the per-server state the UIs render, updated by
//! `poll_events` from the UI loop.
//!
//! The server does not echo a sender's own broadcast, so outgoing posts are
//! shown optimistically and tracked in `awaiting` until the server's ordered
//! `MessagePosted` / `ActionFailed` reply confirms or retracts them.
//!
//! Durable history is requested once per thread when the *channel list*
//! arrives, not on `Joined` (the pipelined `Join` → `ListChannels` →
//! `ListMembers` answers `Joined` first, while the list is still empty). Pages
//! the server cut short are continued (see [`history_page_may_continue`]).

use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{anyhow, bail, Result};
use messenger_core::party::{
    blob_hash, dm_thread_id, history_page_may_continue, AuditEntry, ChannelInfo, ChannelKind,
    Envelope, FileEntry, FileMeta, FilePermissions, MemberInfo, MessagePayload, PartyRequest,
    PartyResponse, QuotaInfo, Role, TrustTier, UploadTarget, MAX_INLINE_FILE_BYTES,
    MAX_PARTY_FILE_BYTES, PARTY_CHUNK_BYTES, UNKNOWN_FILE_ERROR,
};
use messenger_core::util::{current_timestamp_millis, parse_host_port};
use rsa::PrivateKey as RsaPrivateKey;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

/// The smallest upload chunk the client will use, whatever the server asks for.
const MIN_UPLOAD_CHUNK_BYTES: usize = 16 * 1024;

/// Ceiling on reaching a community server: TCP connect plus the handshake.
pub const PARTY_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

const HASH_MISMATCH: &str =
    "The server sent a file that does not match the one that was requested. It was not saved.";

/// What happened when the client tried to join a community server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartyJoinOutcome {
    /// The server's identity matched the pinned/confirmed one, the credentials
    /// were sent, and the join is in flight (watch `PartyServerConn::status`).
    Joining {
        server_id: Uuid,
        fingerprint: String,
    },
    /// First contact with this address: nothing was sent. Show the fingerprint
    /// for an out-of-band check against the one the server prints at startup,
    /// then call again with it as `expected_fingerprint`. `sas` is diagnostic
    /// only — the operator never sees it.
    NeedsVerification { fingerprint: String, sas: String },
}

/// The result of [`PartyManager::dial`].
pub enum PartyDial {
    Verified(Box<VerifiedParty>),
    NeedsVerification { fingerprint: String, sas: String },
}

/// A handshaken connection whose fingerprint matched the verified one. Only
/// [`PartyManager::dial`] makes one and only [`PartyManager::attach`] consumes it.
pub struct VerifiedParty {
    client: messenger_core::party::PartyClient<tokio::net::TcpStream>,
    fingerprint: String,
}

impl std::fmt::Debug for VerifiedParty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifiedParty")
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

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

/// A request whose reply is still awaited. Replies arrive in request order on
/// one connection, and that order is the only correlation available.
enum AwaitingReply {
    /// Answered by `MessagePosted`/`ActionFailed`. `shown` is the optimistic
    /// copy on screen (`None` for a share, which shows nothing).
    Send {
        thread: Uuid,
        shown: Option<Envelope>,
    },
    /// A chunked-upload offer (answered by `UploadReady`/`ActionFailed`),
    /// holding the bytes to stream once accepted.
    UploadOffer {
        thread: Uuid,
        shown: Envelope,
        data: Vec<u8>,
    },
}

/// A chunked download in progress.
struct ChunkedDownload {
    data: Vec<u8>,
    tx: oneshot::Sender<Result<Vec<u8>, String>>,
}

/// Per-server connection state shown in the Party tab.
pub struct PartyServerConn {
    pub address: String,
    /// Kept so the UI can offer one-click rejoin.
    pub username: String,
    pub server_name: String,
    pub server_fingerprint: String,
    pub member_id: Option<Uuid>,
    pub status: PartyStatus,
    pub channels: Vec<ChannelInfo>,
    pub members: Vec<MemberInfo>,
    /// Per-thread message history (delivery order).
    pub messages: HashMap<Uuid, Vec<Envelope>>,
    /// The most recent server error, shown until superseded.
    pub last_error: Option<String>,
    /// The most recent successful governance action, for a confirmation toast.
    pub last_notice: Option<String>,
    /// Files this member may see (Drive panel), refreshed by `ListFiles`.
    pub files: Vec<FileEntry>,
    /// This member's storage usage, refreshed by `FetchQuota`.
    pub quota: Option<QuotaInfo>,
    /// The server's audit log, newest first (admins only).
    pub audit: Vec<AuditEntry>,
    /// Threads whose history was already requested on this connection.
    history_requested: HashSet<Uuid>,
    /// Requests answered by `MessagePosted`/`ActionFailed`/`UploadReady`, in
    /// send order — shares included, or their reply would hit the next send.
    /// The on-screen row is found by value (an unconfirmed seq-0 envelope equal
    /// to `shown`), never by index: a history page re-sorts the thread.
    awaiting: VecDeque<AwaitingReply>,
    outgoing_tx: mpsc::UnboundedSender<PartyRequest>,
    incoming_rx: mpsc::UnboundedReceiver<Incoming>,
    /// Chunked downloads in progress, keyed by content hash.
    chunked_downloads: HashMap<String, ChunkedDownload>,
    /// Single-frame downloads awaiting `FileData`, keyed by content hash.
    /// `UNKNOWN_FILE_ERROR` (not hash-correlated) or a disconnect fails them all.
    pending_downloads: HashMap<String, oneshot::Sender<Result<Vec<u8>, String>>>,
}

impl PartyServerConn {
    fn new(
        address: &str,
        username: &str,
        fingerprint: String,
        outgoing_tx: mpsc::UnboundedSender<PartyRequest>,
        incoming_rx: mpsc::UnboundedReceiver<Incoming>,
    ) -> Self {
        Self {
            address: address.to_string(),
            username: username.to_string(),
            server_name: String::new(),
            server_fingerprint: fingerprint,
            member_id: None,
            status: PartyStatus::Connecting,
            channels: Vec::new(),
            members: Vec::new(),
            messages: HashMap::new(),
            last_error: None,
            last_notice: None,
            files: Vec::new(),
            quota: None,
            audit: Vec::new(),
            history_requested: HashSet::new(),
            awaiting: VecDeque::new(),
            outgoing_tx,
            incoming_rx,
            pending_downloads: HashMap::new(),
            chunked_downloads: HashMap::new(),
        }
    }

    fn send(&self, req: PartyRequest) -> Result<()> {
        self.outgoing_tx
            .send(req)
            .map_err(|_| anyhow!("connection to server is closed"))
    }

    fn me(&self) -> Result<Uuid> {
        self.member_id.ok_or_else(|| anyhow!("not joined yet"))
    }

    /// Show an outgoing message and send the request that stores it.
    fn send_shown(&mut self, thread: Uuid, shown: Envelope, req: PartyRequest) -> Result<()> {
        self.show(thread, shown.clone());
        self.awaiting.push_back(AwaitingReply::Send {
            thread,
            shown: Some(shown.clone()),
        });
        self.send_or_take_back(req, thread, &shown)
    }

    /// Offer a chunked upload, holding its bytes until the server accepts (so a
    /// refused file costs one round trip, not a full transfer).
    fn offer_upload(
        &mut self,
        thread: Uuid,
        shown: Envelope,
        req: PartyRequest,
        data: Vec<u8>,
    ) -> Result<()> {
        self.show(thread, shown.clone());
        self.awaiting.push_back(AwaitingReply::UploadOffer {
            thread,
            shown: shown.clone(),
            data,
        });
        self.send_or_take_back(req, thread, &shown)
    }

    fn show(&mut self, thread: Uuid, shown: Envelope) {
        debug_assert_eq!(shown.seq, 0, "an unconfirmed send carries seq 0");
        self.messages.entry(thread).or_default().push(shown);
    }

    /// If the connection is gone nothing will answer: take the message back so
    /// the caller's error is the only outcome.
    fn send_or_take_back(
        &mut self,
        req: PartyRequest,
        thread: Uuid,
        shown: &Envelope,
    ) -> Result<()> {
        let sent = self.send(req);
        if sent.is_err() {
            self.awaiting.pop_back();
            self.take_back(thread, shown);
        }
        sent
    }

    fn find_unconfirmed(&self, thread: Uuid, shown: &Envelope) -> Option<usize> {
        self.messages
            .get(&thread)?
            .iter()
            .position(|e| e.seq == 0 && e == shown)
    }

    /// Remove an unconfirmed message. Returns whether it was still there.
    fn take_back(&mut self, thread: Uuid, shown: &Envelope) -> bool {
        let Some(index) = self.find_unconfirmed(thread, shown) else {
            return false;
        };
        self.messages
            .get_mut(&thread)
            .expect("found above")
            .remove(index);
        true
    }

    /// The server stored the oldest awaited send as `seq`: stamp it on screen.
    fn confirm_oldest(&mut self, seq: u64) {
        let Some(AwaitingReply::Send { thread, shown }) = self.awaiting.pop_front() else {
            tracing::warn!(seq, "MessagePosted with no send awaiting it");
            return;
        };
        let Some(shown) = shown else { return };
        let Some(index) = self.find_unconfirmed(thread, &shown) else {
            return;
        };
        let list = self.messages.get_mut(&thread).expect("found above");
        if list.iter().any(|e| e.seq == seq) {
            // A history page already delivered the stored copy.
            list.remove(index);
        } else {
            list[index].seq = seq;
        }
    }

    /// The server refused the oldest awaited send or offer: take its message
    /// back (and drop an offer's bytes). Returns whether one was retracted.
    fn retract_oldest(&mut self) -> bool {
        match self.awaiting.pop_front() {
            Some(AwaitingReply::Send {
                thread,
                shown: Some(shown),
            })
            | Some(AwaitingReply::UploadOffer { thread, shown, .. }) => {
                self.take_back(thread, &shown)
            }
            Some(AwaitingReply::Send { shown: None, .. }) | None => false,
        }
    }

    /// The server accepted the oldest offer as `upload`: stream and finish it.
    fn stream_accepted_upload(&mut self, upload: Uuid, chunk_size: u32) {
        let Some(AwaitingReply::UploadOffer {
            thread,
            shown,
            data,
        }) = self.awaiting.pop_front()
        else {
            tracing::warn!(%upload, "UploadReady with no upload offer awaiting it");
            let _ = self.send(PartyRequest::CancelUpload { upload });
            return;
        };
        // Bounded: a 0/1-byte chunk size would queue millions of requests.
        let chunk = (chunk_size as usize).clamp(MIN_UPLOAD_CHUNK_BYTES, PARTY_CHUNK_BYTES);
        for part in data.chunks(chunk) {
            let req = PartyRequest::UploadChunk {
                upload,
                data: part.to_vec(),
            };
            if self.send(req).is_err() {
                return;
            }
        }
        // `FinishUpload` is answered like a send; queued now, after anything
        // sent since the offer.
        self.awaiting.push_back(AwaitingReply::Send {
            thread,
            shown: Some(shown),
        });
        let _ = self.send(PartyRequest::FinishUpload { upload });
    }

    /// Ask for a thread's durable history once per connection.
    fn request_history_once(&mut self, thread: Uuid, dm_peer: Option<Uuid>) {
        if self.history_requested.insert(thread) {
            let _ = self.send(history_request(thread, dm_peer, 0));
        }
    }
}

fn history_request(thread: Uuid, dm_peer: Option<Uuid>, since_seq: u64) -> PartyRequest {
    match dm_peer {
        Some(with) => PartyRequest::FetchDmHistory { with, since_seq },
        None => PartyRequest::FetchHistory {
            channel: thread,
            since_seq,
        },
    }
}

/// Owns the client's Party-server connections.
#[derive(Default)]
pub struct PartyManager {
    servers: HashMap<Uuid, PartyServerConn>,
    /// Last connection error, shown until dismissed or superseded.
    last_error: Option<String>,
}

impl PartyManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_last_error(&mut self, message: String) {
        self.last_error = Some(message);
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn clear_last_error(&mut self) {
        self.last_error = None;
    }

    /// Connect, handshake and (once verified) join, queueing the directory
    /// fetch. `expected_fingerprint` is checked **before** the `Join` frame —
    /// which carries the username and password — is written. With `None` (first
    /// contact) it hangs up after the handshake and returns
    /// [`PartyJoinOutcome::NeedsVerification`]; the shell calls again with the
    /// fingerprint the user accepted. There is deliberately no "trust whatever
    /// answers" mode (audit SRV-05 / TAURI-01 / TUI-02 / UI-01).
    pub async fn connect_and_join(
        &mut self,
        address: &str,
        username: &str,
        password: Option<String>,
        privkey: &RsaPrivateKey,
        expected_fingerprint: Option<&str>,
    ) -> Result<PartyJoinOutcome> {
        self.last_error = None;
        match Self::dial(address, privkey, expected_fingerprint).await? {
            PartyDial::NeedsVerification { fingerprint, sas } => {
                Ok(PartyJoinOutcome::NeedsVerification { fingerprint, sas })
            }
            PartyDial::Verified(verified) => self.attach(address, username, password, *verified),
        }
    }

    /// The network half of [`connect_and_join`](Self::connect_and_join): reach
    /// the server, handshake, check the fingerprint. Touches no manager state
    /// (so a shell can run it without holding its lock) and sends nothing.
    pub async fn dial(
        address: &str,
        privkey: &RsaPrivateKey,
        expected_fingerprint: Option<&str>,
    ) -> Result<PartyDial> {
        use messenger_core::party::PartyClient;

        let (host, port) = parse_host_port(address, Some(messenger_core::PORT_DEFAULT))?;
        // Bounded: callers hold their UI loop or the party lock (TUI-08, APP-10).
        let client = tokio::time::timeout(PARTY_CONNECT_TIMEOUT, async {
            let stream = tokio::net::TcpStream::connect((host.as_str(), port)).await?;
            PartyClient::connect(stream, privkey, Uuid::new_v4()).await
        })
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "Could not reach {address} within {} seconds",
                PARTY_CONNECT_TIMEOUT.as_secs()
            )
        })??;
        let fingerprint = client.server_fingerprint().to_string();
        match expected_fingerprint
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
            // Returning drops `client`, closing the stream with nothing sent.
            Some(expected) if !expected.eq_ignore_ascii_case(&fingerprint) => {
                bail!(
                    "SECURITY: this server's identity is not the one you verified \
                     (expected {}…, got {}…). Your username and password were NOT sent. \
                     If the operator redeployed the server this may be expected — leave \
                     the saved community and rejoin to verify the new identity. \
                     Otherwise, do not proceed.",
                    &expected[..16.min(expected.len())],
                    &fingerprint[..16.min(fingerprint.len())]
                );
            }
            Some(_) => Ok(PartyDial::Verified(Box::new(VerifiedParty {
                client,
                fingerprint,
            }))),
            None => {
                let sas = client.sas().to_string();
                Ok(PartyDial::NeedsVerification { fingerprint, sas })
            }
        }
    }

    /// The state half of [`connect_and_join`](Self::connect_and_join): send the
    /// `Join` (the credentials) on a verified connection and register it.
    pub fn attach(
        &mut self,
        address: &str,
        username: &str,
        password: Option<String>,
        verified: VerifiedParty,
    ) -> Result<PartyJoinOutcome> {
        self.last_error = None;
        let VerifiedParty {
            client,
            fingerprint,
        } = verified;
        let (mut reader, mut writer) = client.split();

        let (outgoing_tx, mut outgoing_rx) = mpsc::unbounded_channel::<PartyRequest>();
        let (incoming_tx, incoming_rx) = mpsc::unbounded_channel::<Incoming>();

        // Two independent pumps, one per direction: `recv_framed` is not
        // cancel-safe, so a read must never lose a `select!` race mid-frame.
        tokio::spawn(async move {
            let send_requests = async {
                while let Some(req) = outgoing_rx.recv().await {
                    if writer.send(&req).await.is_err() {
                        break;
                    }
                }
            };
            let forward_responses = async {
                while let Ok(resp) = reader.recv().await {
                    if incoming_tx.send(Incoming::Response(resp)).is_err() {
                        break;
                    }
                }
            };
            // Either ending ends the connection; nothing reads after it.
            tokio::select! {
                _ = send_requests => {}
                _ = forward_responses => {}
            }
            let _ = incoming_tx.send(Incoming::Disconnected);
        });

        let conn = PartyServerConn::new(
            address,
            username,
            fingerprint.clone(),
            outgoing_tx,
            incoming_rx,
        );
        conn.send(PartyRequest::Join {
            username: username.to_string(),
            password,
        })?;
        conn.send(PartyRequest::ListChannels)?;
        conn.send(PartyRequest::ListMembers)?;

        let server_id = Uuid::new_v4();
        self.servers.insert(server_id, conn);
        Ok(PartyJoinOutcome::Joining {
            server_id,
            fingerprint,
        })
    }

    /// Post a message to a channel (shown locally right away).
    pub fn post(&mut self, server_id: Uuid, channel: Uuid, text: String) -> Result<()> {
        let conn = self.conn_mut(server_id)?;
        let shown = outgoing(conn.me()?, channel, MessagePayload::Text(text.clone()));
        conn.send_shown(channel, shown, PartyRequest::PostMessage { channel, text })
    }

    /// Send a direct message (shown locally under the shared DM thread id).
    pub fn send_dm(&mut self, server_id: Uuid, to: Uuid, text: String) -> Result<()> {
        let conn = self.conn_mut(server_id)?;
        let me = conn.me()?;
        let thread = dm_thread_id(me, to);
        let shown = outgoing(me, thread, MessagePayload::Text(text.clone()));
        conn.send_shown(thread, shown, PartyRequest::SendDm { to, text })
    }

    /// Request DM history with another member (once per connection).
    pub fn fetch_dm_history(&mut self, server_id: Uuid, with: Uuid) -> Result<()> {
        let conn = self.conn_mut(server_id)?;
        let thread = dm_thread_id(conn.me()?, with);
        conn.request_history_once(thread, Some(with));
        Ok(())
    }

    /// Messages in the DM thread with `peer` on a given server (empty if none).
    pub fn dm_messages(&self, server_id: Uuid, peer: Uuid) -> Vec<Envelope> {
        self.servers
            .get(&server_id)
            .and_then(|conn| conn.messages.get(&dm_thread_id(conn.member_id?, peer)))
            .cloned()
            .unwrap_or_default()
    }

    /// Create a channel; the refreshed list arrives on a later poll.
    pub fn create_channel(&self, server_id: Uuid, name: String) -> Result<()> {
        self.conn(server_id)?
            .send(PartyRequest::CreateChannel { name })
    }

    /// Create a channel of a specific kind (`members` seeds a private channel).
    pub fn create_channel_of_kind(
        &self,
        server_id: Uuid,
        name: String,
        kind: ChannelKind,
        members: Vec<Uuid>,
    ) -> Result<()> {
        self.conn(server_id)?
            .send(PartyRequest::CreateChannelOfKind {
                name,
                kind,
                members,
            })
    }

    /// Delete a channel and its history (admins only, enforced by the server).
    pub fn delete_channel(&self, server_id: Uuid, channel: Uuid) -> Result<()> {
        self.conn(server_id)?
            .send(PartyRequest::DeleteChannel { channel })
    }

    /// Change a channel's kind and private membership (admins only).
    pub fn set_channel_access(
        &self,
        server_id: Uuid,
        channel: Uuid,
        kind: ChannelKind,
        members: Vec<Uuid>,
    ) -> Result<()> {
        self.conn(server_id)?.send(PartyRequest::SetChannelAccess {
            channel,
            kind,
            members,
        })
    }

    /// Change another member's role (admins only).
    pub fn set_role(&self, server_id: Uuid, member: Uuid, role: Role) -> Result<()> {
        self.conn(server_id)?
            .send(PartyRequest::SetRole { member, role })
    }

    /// Refresh `PartyServerConn::files` and `quota`.
    pub fn refresh_files(&self, server_id: Uuid) -> Result<()> {
        let conn = self.conn(server_id)?;
        conn.send(PartyRequest::ListFiles)?;
        conn.send(PartyRequest::FetchQuota)
    }

    /// Delete one share of a file (its uploader, or an admin).
    pub fn delete_file(&self, server_id: Uuid, hash: String, location: Uuid) -> Result<()> {
        self.conn(server_id)?.send(PartyRequest::DeleteFile {
            hash,
            channel: location,
        })
    }

    /// Post a file you already hold into another location without re-uploading.
    /// `from` is where you are sharing it from (the source of your right to).
    pub fn share_file(
        &mut self,
        server_id: Uuid,
        hash: String,
        from: Uuid,
        target: UploadTarget,
    ) -> Result<()> {
        let conn = self.conn_mut(server_id)?;
        let thread = match target {
            UploadTarget::Channel(id) => id,
            UploadTarget::Dm(to) => dm_thread_id(conn.member_id.unwrap_or_default(), to),
        };
        // Answered like a post, so it takes its place in the reply order.
        conn.awaiting.push_back(AwaitingReply::Send {
            thread,
            shown: None,
        });
        let sent = conn.send(PartyRequest::ShareFile { hash, from, target });
        if sent.is_err() {
            conn.awaiting.pop_back();
        }
        sent
    }

    /// Change what a shared file grants, by default (`member: None`) or for one
    /// member. The server refuses anything the caller does not hold.
    pub fn set_file_permissions(
        &self,
        server_id: Uuid,
        hash: String,
        location: Uuid,
        member: Option<Uuid>,
        perms: FilePermissions,
    ) -> Result<()> {
        self.conn(server_id)?
            .send(PartyRequest::SetFilePermissions {
                hash,
                location,
                member,
                perms,
            })
    }

    /// Refresh `PartyServerConn::audit`.
    pub fn refresh_audit(&self, server_id: Uuid, limit: u32) -> Result<()> {
        self.conn(server_id)?
            .send(PartyRequest::FetchAuditLog { limit })
    }

    /// This client's role on a server, or `None` before the directory arrives.
    pub fn my_role(&self, server_id: Uuid) -> Option<Role> {
        let conn = self.servers.get(&server_id)?;
        let me = conn.member_id?;
        conn.members.iter().find(|m| m.id == me).map(|m| m.role)
    }

    /// Clear the last governance notice once the UI has shown it.
    pub fn clear_notice(&mut self, server_id: Uuid) {
        if let Some(conn) = self.servers.get_mut(&server_id) {
            conn.last_notice = None;
        }
    }

    fn conn(&self, server_id: Uuid) -> Result<&PartyServerConn> {
        self.servers
            .get(&server_id)
            .ok_or_else(|| anyhow!("unknown server"))
    }

    fn conn_mut(&mut self, server_id: Uuid) -> Result<&mut PartyServerConn> {
        self.servers
            .get_mut(&server_id)
            .ok_or_else(|| anyhow!("unknown server"))
    }

    /// Upload a file to a channel (shown locally right away).
    pub fn send_file(
        &mut self,
        server_id: Uuid,
        channel: Uuid,
        name: String,
        mime: String,
        data: Vec<u8>,
    ) -> Result<()> {
        self.upload(server_id, UploadTarget::Channel(channel), name, mime, data)
    }

    /// Upload a file as a direct message, like [`send_file`](Self::send_file).
    pub fn send_file_dm(
        &mut self,
        server_id: Uuid,
        to: Uuid,
        name: String,
        mime: String,
        data: Vec<u8>,
    ) -> Result<()> {
        self.upload(server_id, UploadTarget::Dm(to), name, mime, data)
    }

    /// Small files go inline in one request; larger ones (one frame is bounded
    /// by `MAX_PACKET_SIZE`) are offered and streamed.
    fn upload(
        &mut self,
        server_id: Uuid,
        target: UploadTarget,
        name: String,
        mime: String,
        data: Vec<u8>,
    ) -> Result<()> {
        let conn = self.conn_mut(server_id)?;
        let me = conn.me()?;
        let thread = match target {
            UploadTarget::Channel(channel) => channel,
            UploadTarget::Dm(to) => dm_thread_id(me, to),
        };
        let shown = outgoing(
            me,
            thread,
            MessagePayload::File(file_meta(&name, &mime, &data)?),
        );
        if data.len() <= MAX_INLINE_FILE_BYTES {
            let req = match target {
                UploadTarget::Channel(channel) => PartyRequest::PostFile {
                    channel,
                    name,
                    mime,
                    data,
                },
                UploadTarget::Dm(to) => PartyRequest::SendFileDm {
                    to,
                    name,
                    mime,
                    data,
                },
            };
            return conn.send_shown(thread, shown, req);
        }
        let req = PartyRequest::StartUpload {
            name,
            mime,
            size: data.len() as u64,
            target,
        };
        conn.offer_upload(thread, shown, req, data)
    }

    /// Request a stored file's bytes by content hash (the server access-checks
    /// it). The receiver resolves when `poll_events` drains the reply, or errors
    /// if the connection drops first.
    pub fn request_download(
        &mut self,
        server_id: Uuid,
        hash: String,
    ) -> Result<oneshot::Receiver<Result<Vec<u8>, String>>> {
        let conn = self.conn_mut(server_id)?;
        let (tx, rx) = oneshot::channel();
        // A file known to be past the inline limit must be streamed; unknown
        // sizes take the single-frame path.
        let size = conn
            .files
            .iter()
            .find(|f| f.hash == hash)
            .map(|f| f.size)
            .or_else(|| {
                conn.messages
                    .values()
                    .flatten()
                    .find_map(|env| match &env.payload {
                        MessagePayload::File(f) if f.hash == hash => Some(f.size),
                        _ => None,
                    })
            })
            .unwrap_or(0);
        if size > MAX_INLINE_FILE_BYTES as u64 {
            conn.chunked_downloads.insert(
                hash.clone(),
                ChunkedDownload {
                    data: Vec::with_capacity(size.min(1 << 20) as usize),
                    tx,
                },
            );
            conn.send(PartyRequest::DownloadChunk { hash, offset: 0 })?;
            return Ok(rx);
        }
        conn.pending_downloads.insert(hash.clone(), tx);
        conn.send(PartyRequest::DownloadFile { hash })?;
        Ok(rx)
    }

    /// Request a channel's durable history (once per connection).
    pub fn fetch_history(&mut self, server_id: Uuid, channel: Uuid) -> Result<()> {
        self.conn_mut(server_id)?
            .request_history_once(channel, None);
        Ok(())
    }

    /// Drain incoming events from every connection. Call from the UI loop.
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

    /// Clear a server's last error (no-op for an unknown server).
    pub fn clear_server_error(&mut self, server_id: Uuid) {
        if let Some(conn) = self.servers.get_mut(&server_id) {
            conn.last_error = None;
        }
    }

    /// Leave a community: drop the connection (its task exits, pending downloads
    /// error) and its local state. The membership stays server-side. Returns
    /// whether a server was removed.
    pub fn remove_server(&mut self, server_id: Uuid) -> bool {
        self.servers.remove(&server_id).is_some()
    }

    pub fn server_ids(&self) -> Vec<Uuid> {
        self.servers.keys().copied().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }
}

/// The optimistic copy of an outgoing message (seq 0 until confirmed).
fn outgoing(sender: Uuid, thread: Uuid, payload: MessagePayload) -> Envelope {
    Envelope {
        tier: TrustTier::Administered,
        sender,
        channel: thread,
        seq: 0,
        timestamp: current_timestamp_millis(),
        payload,
    }
}

/// The [`FileMeta`] for an upload. Only the server's hard ceiling is refused;
/// anything past the inline limit is streamed.
fn file_meta(name: &str, mime: &str, data: &[u8]) -> Result<FileMeta> {
    if data.len() as u64 > MAX_PARTY_FILE_BYTES {
        return Err(anyhow!(
            "file is too large to share here (max {})",
            messenger_core::util::format_size(MAX_PARTY_FILE_BYTES)
        ));
    }
    Ok(FileMeta {
        hash: blob_hash(data),
        name: name.to_string(),
        size: data.len() as u64,
        mime: mime.to_string(),
    })
}

/// Fail every in-flight download so no caller hangs.
fn fail_pending_downloads(conn: &mut PartyServerConn, reason: &str) {
    for (_, tx) in conn.pending_downloads.drain() {
        let _ = tx.send(Err(reason.to_string()));
    }
    for (_, state) in conn.chunked_downloads.drain() {
        let _ = state.tx.send(Err(reason.to_string()));
    }
}

/// Hand downloaded bytes to their waiter, verified against the requested hash:
/// the hash the server echoed proves nothing about the bytes attached to it.
fn deliver_verified(tx: oneshot::Sender<Result<Vec<u8>, String>>, hash: &str, data: Vec<u8>) {
    let actual = blob_hash(&data);
    let _ = if actual == hash {
        tx.send(Ok(data))
    } else {
        tracing::error!(
            expected = %hash,
            actual = %actual,
            "community server returned bytes that do not match the requested content hash"
        );
        tx.send(Err(HASH_MISMATCH.to_string()))
    };
}

/// Request history for every visible thread not yet asked about. Runs after
/// `Joined`, `Channels` and `Members`: channels decide which channels exist,
/// members which DM threads do, and any of the three can complete the picture.
fn seed_history(conn: &mut PartyServerConn) {
    let Some(me) = conn.member_id else {
        return; // the server refuses history until we have joined
    };
    let channels: Vec<Uuid> = conn.channels.iter().map(|c| c.id).collect();
    for ch in channels {
        conn.request_history_once(ch, None);
    }
    let peers: Vec<Uuid> = conn
        .members
        .iter()
        .map(|m| m.id)
        .filter(|id| *id != me)
        .collect();
    for peer in peers {
        conn.request_history_once(dm_thread_id(me, peer), Some(peer));
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
            seed_history(conn);
        }
        PartyResponse::JoinRejected { reason } => conn.status = PartyStatus::Rejected(reason),
        PartyResponse::Members(members) => {
            conn.members = members;
            seed_history(conn);
        }
        PartyResponse::Channels(channels) => {
            conn.channels = channels;
            seed_history(conn);
        }
        PartyResponse::Message(env) => {
            // Seq 0 is our "unconfirmed send" sentinel; an honest server never
            // sends it, and a planted one would make `ActionFailed` retract the
            // wrong row.
            if env.seq == 0 {
                tracing::warn!(
                    channel = %env.channel,
                    "server sent an envelope with sequence 0; ignoring it"
                );
                return;
            }
            let thread = conn.messages.entry(env.channel).or_default();
            // Deduplicate against a racing history page by seq. Confirmed
            // envelopes are in ascending order, so a seq above the newest is new
            // without a scan (PERF-05).
            let newest = thread.iter().rev().find(|e| e.seq != 0).map(|e| e.seq);
            if newest.is_none_or(|n| env.seq > n) || !thread.iter().any(|e| e.seq == env.seq) {
                thread.push(env);
            }
        }
        PartyResponse::History(items) => {
            // A page is cut short by count *or* size.
            let page_was_full = history_page_may_continue(&items);
            let threads: HashSet<Uuid> = items.iter().map(|e| e.channel).collect();
            // DM thread -> peer, built once per page and only when continuing
            // (PERF-05).
            let dm_peers: HashMap<Uuid, Uuid> = match conn.member_id {
                Some(me) if page_was_full => conn
                    .members
                    .iter()
                    .map(|m| (dm_thread_id(me, m.id), m.id))
                    .collect(),
                _ => HashMap::new(),
            };
            for th in threads {
                // Same seq-0 rule as `Message`.
                let page = items.iter().filter(|e| e.channel == th && e.seq != 0);
                let existing = conn.messages.entry(th).or_default();
                // Merge by seq (pages must not replace each other), keeping
                // unconfirmed sends; one known-seq set per page keeps it linear
                // (PERF-05), and the sort runs only when something was added.
                let mut known: HashSet<u64> =
                    existing.iter().map(|e| e.seq).filter(|&s| s != 0).collect();
                let before = existing.len();
                for env in page {
                    if known.insert(env.seq) {
                        existing.push(env.clone());
                    }
                }
                if existing.len() != before {
                    existing.sort_by_key(|e| if e.seq == 0 { u64::MAX } else { e.seq });
                }
                if page_was_full {
                    if let Some(last) = existing.iter().rfind(|e| e.seq > 0) {
                        let req = history_request(th, dm_peers.get(&th).copied(), last.seq);
                        let _ = conn.send(req);
                    }
                }
            }
        }
        PartyResponse::MessagePosted { seq, .. } => conn.confirm_oldest(seq),
        PartyResponse::ActionFailed { reason, .. } => {
            let retracted = conn.retract_oldest();
            conn.last_error = Some(if retracted {
                format!("Not sent: {reason}")
            } else {
                reason
            });
        }
        // Unsolicited/duplicate data has no waiter and is dropped.
        PartyResponse::FileData { hash, data } => {
            if let Some(tx) = conn.pending_downloads.remove(&hash) {
                deliver_verified(tx, &hash, data);
            }
        }
        // Only the not-hash-correlated download error fails in-flight downloads;
        // any other error (a refused channel name) must not cancel them.
        PartyResponse::Error(message) => {
            if message == UNKNOWN_FILE_ERROR {
                fail_pending_downloads(conn, &message);
            }
            conn.last_error = Some(message);
        }
        // The channel list is per member, so the server nudges and each client
        // fetches its own view.
        PartyResponse::DirectoryChanged => {
            let _ = conn.send(PartyRequest::ListChannels);
            let _ = conn.send(PartyRequest::ListMembers);
        }
        PartyResponse::UploadReady { upload, chunk_size } => {
            conn.stream_accepted_upload(upload, chunk_size)
        }
        PartyResponse::FileChunk {
            hash,
            offset,
            total,
            data,
        } => {
            let Some(state) = conn.chunked_downloads.get_mut(&hash) else {
                return; // unsolicited, or already completed/failed
            };
            // Out of order, or more than any server may store / than announced:
            // fail rather than assemble something wrong or buffer on its say-so.
            let grown = state.data.len() as u64 + data.len() as u64;
            let refusal = if offset != state.data.len() as u64 {
                Some("the server sent file data out of order")
            } else if total > MAX_PARTY_FILE_BYTES || grown > total {
                Some("the server sent more file data than the file can hold")
            } else {
                None
            };
            if let Some(reason) = refusal {
                if let Some(state) = conn.chunked_downloads.remove(&hash) {
                    let _ = state.tx.send(Err(reason.to_string()));
                }
                return;
            }
            state.data.extend_from_slice(&data);
            let received = state.data.len() as u64;
            if received < total && !data.is_empty() {
                let _ = conn.send(PartyRequest::DownloadChunk {
                    hash: hash.clone(),
                    offset: received,
                });
                return;
            }
            if let Some(state) = conn.chunked_downloads.remove(&hash) {
                deliver_verified(state.tx, &hash, state.data);
            }
        }
        PartyResponse::Files(files) => {
            // Only a full listing may be truncated; page on only then, since an
            // older server cannot decode the paged request (audit SRV-01).
            let full = files.len() >= messenger_core::party::MAX_FILE_LISTING;
            conn.files = files;
            if full {
                let offset = u32::try_from(conn.files.len()).unwrap_or(u32::MAX);
                let _ = conn.send(PartyRequest::ListFilesPage { offset });
            }
        }
        PartyResponse::FilesPage {
            files,
            offset,
            next,
        } => {
            // A stale page (the list was refreshed meanwhile) is dropped.
            if offset as usize == conn.files.len() {
                conn.files.extend(files);
                if let Some(next) = next {
                    let _ = conn.send(PartyRequest::ListFilesPage { offset: next });
                }
            }
        }
        PartyResponse::Quota(quota) => conn.quota = Some(quota),
        PartyResponse::AuditLog(entries) => conn.audit = entries,
        PartyResponse::Ok(message) => conn.last_notice = Some(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use messenger_core::party::{HISTORY_PAGE_BYTES, MAX_HISTORY_BATCH};

    /// A manager with one server connection wired to test channels.
    struct Fx {
        mgr: PartyManager,
        id: Uuid,
        tx: mpsc::UnboundedSender<Incoming>,
        out: mpsc::UnboundedReceiver<PartyRequest>,
    }

    impl Fx {
        fn new() -> Self {
            let (outgoing_tx, out) = mpsc::unbounded_channel();
            let (tx, incoming_rx) = mpsc::unbounded_channel();
            let conn = PartyServerConn::new(
                "127.0.0.1:12345",
                "alice",
                "FP".to_string(),
                outgoing_tx,
                incoming_rx,
            );
            let mut mgr = PartyManager::new();
            let id = Uuid::new_v4();
            mgr.servers.insert(id, conn);
            Self { mgr, id, tx, out }
        }

        /// A connection that has joined as a fresh member id.
        fn joined() -> (Self, Uuid) {
            let mut fx = Self::new();
            let me = Uuid::new_v4();
            fx.mgr.servers.get_mut(&fx.id).unwrap().member_id = Some(me);
            (fx, me)
        }

        /// Deliver a server response and poll it in.
        fn respond(&mut self, resp: PartyResponse) {
            self.tx.send(Incoming::Response(resp)).unwrap();
            self.mgr.poll_events();
        }

        fn conn(&self) -> &PartyServerConn {
            self.mgr.server(self.id).unwrap()
        }

        fn conn_mut(&mut self) -> &mut PartyServerConn {
            self.mgr.servers.get_mut(&self.id).unwrap()
        }

        fn thread(&self, channel: Uuid) -> &Vec<Envelope> {
            &self.conn().messages[&channel]
        }

        fn thread_is_empty(&self, channel: Uuid) -> bool {
            self.conn()
                .messages
                .get(&channel)
                .is_none_or(|t| t.is_empty())
        }

        /// Every request emitted so far.
        fn sent(&mut self) -> Vec<PartyRequest> {
            std::iter::from_fn(|| self.out.try_recv().ok()).collect()
        }

        fn download(&mut self, hash: &str) -> oneshot::Receiver<Result<Vec<u8>, String>> {
            self.mgr
                .request_download(self.id, hash.to_string())
                .unwrap()
        }

        fn send_file(&mut self, channel: Uuid, name: &str, data: Vec<u8>) -> Result<()> {
            self.mgr
                .send_file(self.id, channel, name.into(), "x".into(), data)
        }

        /// Put a file message of `size` into history so a download knows its size.
        fn known_file(&mut self, channel: Uuid, hash: &str, size: u64) {
            self.conn_mut()
                .messages
                .entry(channel)
                .or_default()
                .push(Envelope {
                    tier: TrustTier::Administered,
                    sender: Uuid::new_v4(),
                    channel,
                    seq: 1,
                    timestamp: 1,
                    payload: MessagePayload::File(FileMeta {
                        hash: hash.to_string(),
                        name: "big.bin".into(),
                        size,
                        mime: "application/octet-stream".into(),
                    }),
                });
        }
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

    fn general(id: Uuid) -> PartyResponse {
        PartyResponse::Channels(vec![ChannelInfo {
            id,
            name: "general".to_string(),
            kind: ChannelKind::Public,
            members: Vec::new(),
        }])
    }

    fn joined_as(member_id: Uuid) -> PartyResponse {
        PartyResponse::Joined {
            member_id,
            server_name: "Srv".to_string(),
            tier: TrustTier::Administered,
        }
    }

    fn member(id: Uuid, username: &str) -> MemberInfo {
        MemberInfo {
            id,
            username: username.to_string(),
            online: true,
            role: Role::Member,
        }
    }

    fn failed(channel: Uuid, reason: &str) -> PartyResponse {
        PartyResponse::ActionFailed {
            channel,
            reason: reason.to_string(),
        }
    }

    fn posted(channel: Uuid, seq: u64) -> PartyResponse {
        PartyResponse::MessagePosted { channel, seq }
    }

    fn ready(upload: Uuid, chunk_size: u32) -> PartyResponse {
        PartyResponse::UploadReady { upload, chunk_size }
    }

    #[test]
    fn poll_applies_join_directory_and_messages() {
        let mut fx = Fx::new();
        let me = Uuid::new_v4();
        let channel = Uuid::new_v4();

        fx.respond(general(channel));
        fx.respond(joined_as(me));
        fx.respond(PartyResponse::Members(vec![member(me, "alice")]));
        fx.respond(PartyResponse::Message(envelope(channel, me, 1, "hi")));

        let conn = fx.conn();
        assert_eq!(conn.status, PartyStatus::Joined);
        assert_eq!(conn.member_id, Some(me));
        assert_eq!(conn.server_name, "Srv");
        assert_eq!(conn.channels.len(), 1);
        assert_eq!(conn.members.len(), 1);
        assert_eq!(conn.messages.get(&channel).map(|m| m.len()), Some(1));
    }

    #[test]
    fn join_rejection_is_surfaced() {
        let mut fx = Fx::new();
        fx.respond(PartyResponse::JoinRejected {
            reason: "wrong password".to_string(),
        });
        assert_eq!(
            fx.conn().status,
            PartyStatus::Rejected("wrong password".to_string())
        );
    }

    #[test]
    fn post_appends_locally_and_emits_request() {
        let (mut fx, _) = Fx::joined();
        let channel = Uuid::new_v4();
        fx.mgr.post(fx.id, channel, "hello".to_string()).unwrap();

        let msgs = fx.thread(channel);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].payload, MessagePayload::Text("hello".to_string()));
        assert_eq!(
            fx.out.try_recv().unwrap(),
            PartyRequest::PostMessage {
                channel,
                text: "hello".into()
            }
        );
    }

    #[test]
    fn post_before_join_fails() {
        let mut fx = Fx::new();
        assert!(fx.mgr.post(fx.id, Uuid::new_v4(), "x".to_string()).is_err());
    }

    #[test]
    fn send_dm_appends_locally_and_emits_request() {
        let (mut fx, _) = Fx::joined();
        let peer = Uuid::new_v4();
        fx.mgr.send_dm(fx.id, peer, "hi".to_string()).unwrap();

        let dm = fx.mgr.dm_messages(fx.id, peer);
        assert_eq!(dm.len(), 1);
        assert_eq!(dm[0].payload, MessagePayload::Text("hi".to_string()));
        assert_eq!(
            fx.out.try_recv().unwrap(),
            PartyRequest::SendDm {
                to: peer,
                text: "hi".into()
            }
        );
    }

    #[test]
    fn create_channel_emits_request() {
        let mut fx = Fx::new();
        fx.mgr.create_channel(fx.id, "random".to_string()).unwrap();
        assert_eq!(
            fx.out.try_recv().unwrap(),
            PartyRequest::CreateChannel {
                name: "random".into()
            }
        );
    }

    #[test]
    fn send_file_appends_optimistically_and_emits_postfile() {
        let (mut fx, _) = Fx::joined();
        let channel = Uuid::new_v4();
        let data = b"\x89PNG fake image bytes".to_vec();
        fx.mgr
            .send_file(
                fx.id,
                channel,
                "photo.png".to_string(),
                "image/png".to_string(),
                data.clone(),
            )
            .unwrap();

        // Optimistic local append is a File message with the content hash/size.
        let msgs = fx.thread(channel);
        assert_eq!(msgs.len(), 1);
        match &msgs[0].payload {
            MessagePayload::File(f) => {
                assert_eq!(f.name, "photo.png");
                assert_eq!(f.size, data.len() as u64);
                assert_eq!(f.hash, blob_hash(&data));
            }
            other => panic!("expected a File payload, got {other:?}"),
        }
        match fx.out.try_recv().unwrap() {
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
    fn oversized_file_upload_is_rejected_without_side_effects() {
        let (mut fx, _) = Fx::joined();
        let channel = Uuid::new_v4();
        // Past the server's ceiling, not merely the inline limit.
        let too_big = vec![0u8; MAX_PARTY_FILE_BYTES as usize + 1];
        assert!(
            fx.send_file(channel, "big.bin", too_big).is_err(),
            "a file past the server ceiling must be rejected"
        );
        assert!(!fx.conn().messages.contains_key(&channel));
        assert!(fx.out.try_recv().is_err());
    }

    /// A file too large for one frame is offered first and streamed only once
    /// the server accepts it.
    #[test]
    fn a_large_file_is_offered_first_then_streamed_in_chunks() {
        let (mut fx, _) = Fx::joined();
        let channel = Uuid::new_v4();
        let payload: Vec<u8> = (0..(MAX_INLINE_FILE_BYTES + 1234))
            .map(|i| (i % 253) as u8)
            .collect();
        fx.send_file(channel, "big.bin", payload.clone()).unwrap();

        match fx.out.try_recv().unwrap() {
            PartyRequest::StartUpload { size, target, .. } => {
                assert_eq!(size, payload.len() as u64);
                assert_eq!(target, UploadTarget::Channel(channel));
            }
            other => panic!("expected StartUpload, got {other:?}"),
        }
        assert!(
            fx.out.try_recv().is_err(),
            "nothing streams before acceptance"
        );
        assert_eq!(fx.thread(channel).len(), 1, "already on screen");

        let upload = Uuid::new_v4();
        fx.respond(ready(upload, PARTY_CHUNK_BYTES as u32));

        let mut streamed = Vec::new();
        let mut finished = false;
        for req in fx.sent() {
            match req {
                PartyRequest::UploadChunk { upload: u, data } => {
                    assert_eq!(u, upload);
                    assert!(data.len() <= PARTY_CHUNK_BYTES);
                    streamed.extend_from_slice(&data);
                }
                PartyRequest::FinishUpload { upload: u } => {
                    assert_eq!(u, upload);
                    finished = true;
                }
                other => panic!("unexpected request during streaming: {other:?}"),
            }
        }
        assert_eq!(streamed, payload, "every byte arrives, in order");
        assert!(finished, "the upload is finished, not left open");
    }

    /// A refused offer must drop its bytes, not stream them under the next id.
    #[test]
    fn a_refused_upload_offer_does_not_stream_into_the_next_upload() {
        let (mut fx, _) = Fx::joined();
        let channel = Uuid::new_v4();
        let big = MAX_INLINE_FILE_BYTES + 1;

        fx.send_file(channel, "refused.bin", vec![1u8; big])
            .unwrap();
        fx.respond(failed(channel, "over quota"));
        fx.send_file(channel, "wanted.bin", vec![2u8; big]).unwrap();
        let upload = Uuid::new_v4();
        fx.respond(ready(upload, PARTY_CHUNK_BYTES as u32));

        let mut streamed = Vec::new();
        for req in fx.sent() {
            if let PartyRequest::UploadChunk { upload: u, data } = req {
                assert_eq!(u, upload);
                streamed.extend(data);
            }
        }
        assert_eq!(streamed.len(), big);
        assert!(
            streamed.iter().all(|b| *b == 2),
            "the refused file's bytes were streamed under the accepted upload"
        );
        assert_eq!(
            fx.thread(channel).len(),
            1,
            "only the accepted upload stays on screen"
        );
    }

    #[test]
    fn a_tiny_upload_chunk_size_from_the_server_is_bounded() {
        let (mut fx, _) = Fx::joined();
        let size = MAX_INLINE_FILE_BYTES + 1;
        fx.send_file(Uuid::new_v4(), "f.bin", vec![9u8; size])
            .unwrap();
        fx.respond(ready(Uuid::new_v4(), 0));
        let mut chunks = 0usize;
        let mut bytes = 0usize;
        for req in fx.sent() {
            if let PartyRequest::UploadChunk { data, .. } = req {
                chunks += 1;
                bytes += data.len();
            }
        }
        assert_eq!(bytes, size, "every byte is still sent");
        assert!(
            chunks <= size / (16 * 1024) + 1,
            "{chunks} requests queued for one {size}-byte file"
        );
    }

    #[test]
    fn request_download_resolves_when_filedata_arrives() {
        let (mut fx, _) = Fx::joined();
        let data = b"downloaded file contents".to_vec();
        let hash = blob_hash(&data);

        let mut rx = fx.download(&hash);
        assert_eq!(
            fx.out.try_recv().unwrap(),
            PartyRequest::DownloadFile { hash: hash.clone() }
        );
        assert!(rx.try_recv().is_err(), "pending before the response");

        fx.respond(PartyResponse::FileData {
            hash,
            data: data.clone(),
        });
        assert_eq!(rx.try_recv().unwrap(), Ok(data));
    }

    /// Content addressing is the integrity check: mismatched bytes are refused.
    #[tokio::test]
    async fn a_download_whose_bytes_do_not_match_its_hash_is_refused() {
        let mut fx = Fx::new();
        let hash = blob_hash(b"the real file");
        let rx = fx.download(&hash);
        fx.respond(PartyResponse::FileData {
            hash,
            data: b"something else entirely".to_vec(),
        });
        let err = rx
            .await
            .expect("the waiter is completed")
            .expect_err("mismatched bytes must not be handed to the caller");
        assert!(err.contains("does not match"), "{err}");
    }

    #[test]
    fn a_large_download_is_reassembled_from_chunks() {
        let (mut fx, _) = Fx::joined();
        let channel = Uuid::new_v4();
        // Past the inline limit, which is what makes this a chunked download.
        let payload: Vec<u8> = (0..(MAX_INLINE_FILE_BYTES + 500))
            .map(|i| (i % 199) as u8)
            .collect();
        let hash = blob_hash(&payload);
        fx.known_file(channel, &hash, payload.len() as u64);

        let mut rx = fx.download(&hash);
        assert_eq!(
            fx.out.try_recv().unwrap(),
            PartyRequest::DownloadChunk {
                hash: hash.clone(),
                offset: 0
            }
        );

        // Feed it back one chunk at a time, following the offsets it asks for.
        let total = payload.len() as u64;
        let mut offset = 0usize;
        while offset < payload.len() {
            let end = (offset + PARTY_CHUNK_BYTES).min(payload.len());
            fx.respond(PartyResponse::FileChunk {
                hash: hash.clone(),
                offset: offset as u64,
                total,
                data: payload[offset..end].to_vec(),
            });
            offset = end;
            if offset < payload.len() {
                match fx.out.try_recv().unwrap() {
                    PartyRequest::DownloadChunk { offset: next, .. } => {
                        assert_eq!(next, offset as u64, "asks for exactly what it is missing");
                    }
                    other => panic!("expected the next DownloadChunk, got {other:?}"),
                }
            }
        }
        assert_eq!(rx.try_recv().unwrap(), Ok(payload));
    }

    #[test]
    fn an_out_of_order_chunk_fails_the_download() {
        let (mut fx, _) = Fx::joined();
        let hash = "abc123".to_string();
        let (otx, mut rx) = oneshot::channel();
        fx.conn_mut().chunked_downloads.insert(
            hash.clone(),
            ChunkedDownload {
                data: Vec::new(),
                tx: otx,
            },
        );
        fx.respond(PartyResponse::FileChunk {
            hash,
            offset: 999, // not where we are
            total: 4096,
            data: vec![1, 2, 3],
        });
        assert_eq!(
            rx.try_recv().unwrap(),
            Err("the server sent file data out of order".to_string())
        );
    }

    #[test]
    fn a_download_claiming_more_than_the_server_ceiling_is_refused() {
        let (mut fx, _) = Fx::joined();
        let hash = "ab".repeat(32);
        fx.known_file(Uuid::new_v4(), &hash, MAX_PARTY_FILE_BYTES);
        let mut rx = fx.download(&hash);
        fx.respond(PartyResponse::FileChunk {
            hash,
            offset: 0,
            total: MAX_PARTY_FILE_BYTES * 1000,
            data: vec![0u8; 1024],
        });
        assert!(
            matches!(rx.try_recv(), Ok(Err(_))),
            "the client kept downloading toward an impossible size"
        );
    }

    #[test]
    fn request_download_fails_when_the_server_reports_an_error() {
        let (mut fx, _) = Fx::joined();
        let mut rx = fx.download("deadbeef");
        fx.respond(PartyResponse::Error("unknown file".to_string()));
        assert_eq!(rx.try_recv().unwrap(), Err("unknown file".to_string()));
    }

    /// Errors carry no request id; an unrelated one must not fail a download.
    #[test]
    fn an_unrelated_server_error_does_not_fail_a_download() {
        let (mut fx, _) = Fx::joined();
        let mut rx = fx.download("deadbeef");
        fx.respond(PartyResponse::Error("channel name is taken".to_string()));
        assert!(rx.try_recv().is_err(), "the download is still in flight");
        assert_eq!(
            fx.conn().last_error.as_deref(),
            Some("channel name is taken")
        );
    }

    #[test]
    fn remove_server_forgets_it_and_fails_inflight_downloads() {
        let (mut fx, _) = Fx::joined();
        let mut rx = fx.download("abc123");

        assert!(fx.mgr.remove_server(fx.id), "the server is removed");
        assert!(fx.mgr.server(fx.id).is_none());
        assert!(fx.mgr.server_ids().is_empty());
        assert!(
            rx.try_recv().is_err(),
            "pending download receivers error out on leave"
        );
        assert!(!fx.mgr.remove_server(fx.id), "removing again is a no-op");
    }

    #[test]
    fn history_merges_into_channel_messages() {
        let mut fx = Fx::new();
        let me = Uuid::new_v4();
        let channel = Uuid::new_v4();
        fx.respond(PartyResponse::History(vec![
            envelope(channel, me, 1, "one"),
            envelope(channel, me, 2, "two"),
        ]));
        assert_eq!(fx.thread(channel).len(), 2);
    }

    /// PERF-05: the merge stays correct (no duplicates, ascending, a broadcast
    /// racing a page) and linear for a 50k-message channel.
    #[test]
    fn paging_a_long_channel_is_linear_and_deduplicated() {
        let mut fx = Fx::new();
        let me = Uuid::new_v4();
        let channel = Uuid::new_v4();
        let started = std::time::Instant::now();
        let total = 50_000u64;
        for start in (1..=total).step_by(MAX_HISTORY_BATCH) {
            let end = (start + MAX_HISTORY_BATCH as u64 - 1).min(total);
            // A live broadcast of the page's last envelope arrives first.
            fx.respond(PartyResponse::Message(envelope(channel, me, end, "live")));
            let page = (start..=end)
                .map(|s| envelope(channel, me, s, "x"))
                .collect();
            fx.respond(PartyResponse::History(page));
        }
        let thread = fx.thread(channel);
        assert_eq!(thread.len() as u64, total);
        assert!(thread.windows(2).all(|w| w[0].seq < w[1].seq));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "merging {total} envelopes took {:?}",
            started.elapsed()
        );
    }

    /// SRV-01: a full `Files` reply may be truncated; only then page on.
    #[test]
    fn a_full_file_listing_is_continued_page_by_page() {
        let mut fx = Fx::new();
        let entry = |i: usize| FileEntry {
            hash: format!("{i:064x}"),
            name: format!("f{i}"),
            size: 1,
            mime: "text/plain".into(),
            uploader: Uuid::nil(),
            uploader_name: "a".into(),
            location: Uuid::nil(),
            location_name: "#general".into(),
            is_dm: false,
            shared_at: 1,
            can_delete: false,
            perms: Default::default(),
        };
        let max = messenger_core::party::MAX_FILE_LISTING;
        fx.sent();

        fx.respond(PartyResponse::Files(vec![entry(0)]));
        assert!(fx.out.try_recv().is_err(), "a short listing is complete");

        fx.respond(PartyResponse::Files((0..max).map(entry).collect()));
        assert_eq!(
            fx.out.try_recv().unwrap(),
            PartyRequest::ListFilesPage { offset: max as u32 }
        );

        fx.respond(PartyResponse::FilesPage {
            files: (max..max + 3).map(entry).collect(),
            offset: max as u32,
            next: None,
        });
        assert_eq!(fx.conn().files.len(), max + 3);
        assert!(
            fx.out.try_recv().is_err(),
            "the last page asks for nothing more"
        );
    }

    #[test]
    fn disconnect_marks_status() {
        let mut fx = Fx::new();
        fx.tx.send(Incoming::Disconnected).unwrap();
        fx.mgr.poll_events();
        assert_eq!(fx.conn().status, PartyStatus::Disconnected);
    }

    #[test]
    fn server_error_response_is_surfaced_not_dropped() {
        let mut fx = Fx::new();
        fx.respond(PartyResponse::Error("channel is locked".to_string()));
        assert_eq!(
            fx.conn().last_error.as_deref(),
            Some("channel is locked"),
            "a server Error reply must surface, not be silently dropped"
        );
        fx.mgr.clear_server_error(fx.id);
        assert!(
            fx.conn().last_error.is_none(),
            "a dismissed server error must be cleared"
        );
        // Clearing an unknown server is a harmless no-op.
        fx.mgr.clear_server_error(Uuid::new_v4());
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

    /// `Joined` arrives before the channel list, so history must be seeded when
    /// the list (and members) arrive — and only once.
    #[test]
    fn history_is_requested_once_the_channel_list_arrives() {
        let mut fx = Fx::new();
        let me = Uuid::new_v4();
        let channel = Uuid::new_v4();
        let peer = Uuid::new_v4();

        // Exactly the order the server answers in.
        fx.respond(joined_as(me));
        fx.respond(general(channel));
        fx.respond(PartyResponse::Members(vec![
            member(me, "alice"),
            member(peer, "bob"),
        ]));

        let sent = fx.sent();
        assert!(
            sent.iter().any(
                |r| matches!(r, PartyRequest::FetchHistory { channel: c, .. } if *c == channel)
            ),
            "channel history must be fetched on join"
        );
        assert!(
            sent.iter()
                .any(|r| matches!(r, PartyRequest::FetchDmHistory { with, .. } if *with == peer)),
            "DM history must be fetched on join"
        );

        // A re-broadcast channel list must not refetch everything.
        fx.respond(general(channel));
        assert!(
            fx.out.try_recv().is_err(),
            "already-seeded threads must not be re-requested"
        );
    }

    fn asked_for_history_since(fx: &mut Fx, channel: Uuid, since: u64) -> bool {
        fx.sent().into_iter().any(|r| {
            matches!(r, PartyRequest::FetchHistory { channel: c, since_seq }
                if c == channel && since_seq == since)
        })
    }

    /// A later page merges into the earlier one, and a full page asks for more.
    #[test]
    fn history_pages_are_merged_and_continued() {
        let (mut fx, me) = Fx::joined();
        let channel = Uuid::new_v4();
        let batch = MAX_HISTORY_BATCH as u64;

        let page = (1..=batch)
            .map(|seq| envelope(channel, me, seq, &format!("m{seq}")))
            .collect();
        fx.respond(PartyResponse::History(page));
        assert_eq!(fx.thread(channel).len(), MAX_HISTORY_BATCH);
        assert!(
            asked_for_history_since(&mut fx, channel, batch),
            "a full page must be followed by the next one"
        );

        let tail = envelope(channel, me, batch + 1, "tail");
        fx.respond(PartyResponse::History(vec![tail]));
        assert_eq!(
            fx.thread(channel).len(),
            MAX_HISTORY_BATCH + 1,
            "a later page must not throw away the earlier one"
        );
    }

    /// A page cut short for *size* is continued; a small short page is the end.
    #[test]
    fn a_page_trimmed_for_size_is_continued() {
        let (mut fx, me) = Fx::joined();
        let channel = Uuid::new_v4();

        let long = "x".repeat(60 * 1024);
        let count = (HISTORY_PAGE_BYTES / long.len()) as u64; // ~68: well under 200
        assert!((count as usize) < MAX_HISTORY_BATCH);
        let page = (1..=count)
            .map(|seq| envelope(channel, me, seq, &long))
            .collect();
        fx.respond(PartyResponse::History(page));
        assert!(
            asked_for_history_since(&mut fx, channel, count),
            "a page trimmed for size must be continued"
        );

        let tail = envelope(channel, me, count + 1, "tail");
        fx.respond(PartyResponse::History(vec![tail]));
        assert!(
            fx.out.try_recv().is_err(),
            "a small final page must not be followed by another request"
        );
    }

    /// Seq 0 is the client's "unconfirmed" sentinel; a server must not be able
    /// to plant one and make a refusal retract the wrong row.
    #[test]
    fn a_planted_zero_sequence_envelope_is_ignored() {
        let (mut fx, me) = Fx::joined();
        let channel = Uuid::new_v4();

        fx.respond(PartyResponse::Message(envelope(channel, me, 0, "planted")));
        assert!(
            fx.thread_is_empty(channel),
            "a seq-0 envelope from the server was accepted"
        );

        fx.respond(PartyResponse::History(vec![
            envelope(channel, me, 0, "planted in history"),
            envelope(channel, me, 4, "real"),
        ]));
        let thread = fx.thread(channel);
        assert_eq!(thread.len(), 1, "the planted envelope survived history");
        assert_eq!(thread[0].seq, 4);

        // With nothing planted, our own send is the one the refusal retracts.
        fx.mgr.post(fx.id, channel, "mine".to_string()).unwrap();
        assert_eq!(fx.thread(channel).len(), 2);
        fx.respond(failed(channel, "nope"));
        let thread = fx.thread(channel);
        assert_eq!(thread.len(), 1, "the refusal retracted the wrong message");
        assert_eq!(thread[0].seq, 4, "the server's real message was removed");
    }

    /// A history page re-sorts the thread; replies must still find their rows.
    #[test]
    fn a_history_page_does_not_misdirect_replies_to_pending_sends() {
        let (mut fx, _) = Fx::joined();
        let other = Uuid::new_v4();
        let channel = Uuid::new_v4();

        fx.mgr.post(fx.id, channel, "refused".to_string()).unwrap();
        fx.mgr.post(fx.id, channel, "stored".to_string()).unwrap();
        fx.respond(PartyResponse::History(vec![
            envelope(channel, other, 1, "a"),
            envelope(channel, other, 2, "b"),
        ]));
        fx.respond(failed(channel, "nope"));
        fx.respond(posted(channel, 3));

        let texts: Vec<(u64, String)> = fx
            .thread(channel)
            .iter()
            .map(|e| match &e.payload {
                MessagePayload::Text(t) => (e.seq, t.clone()),
                other => panic!("unexpected payload {other:?}"),
            })
            .collect();
        assert_eq!(
            texts,
            vec![
                (1, "a".to_string()),
                (2, "b".to_string()),
                (3, "stored".to_string())
            ],
            "the refusal and the confirmation reached the wrong messages"
        );
    }

    /// A share shows nothing but must still hold its place in the reply queue.
    #[test]
    fn a_share_reply_is_not_matched_to_a_pending_post() {
        let (mut fx, _) = Fx::joined();
        let channel = Uuid::new_v4();
        fx.mgr
            .share_file(
                fx.id,
                "h".into(),
                Uuid::new_v4(),
                UploadTarget::Channel(channel),
            )
            .unwrap();
        fx.mgr
            .post(fx.id, channel, "after the share".to_string())
            .unwrap();
        fx.respond(posted(channel, 7));
        fx.respond(posted(channel, 8));

        let thread = fx.thread(channel);
        assert_eq!(thread.len(), 1);
        assert_eq!(thread[0].seq, 8, "the share's reply confirmed the post");
    }

    /// A post on a closed connection fails and leaves nothing on screen.
    #[test]
    fn posting_after_the_connection_closed_shows_nothing() {
        let (fx, _) = Fx::joined();
        let Fx {
            mut mgr, id, out, ..
        } = fx;
        let channel = Uuid::new_v4();
        drop(out);
        assert!(mgr.post(id, channel, "into the void".into()).is_err());
        assert!(
            mgr.server(id)
                .unwrap()
                .messages
                .get(&channel)
                .is_none_or(|t| t.is_empty()),
            "an unsent post was left on screen"
        );
    }

    #[test]
    fn a_refused_send_is_taken_back_off_the_screen() {
        let (mut fx, _) = Fx::joined();
        let channel = Uuid::new_v4();
        fx.mgr.post(fx.id, channel, "first".to_string()).unwrap();
        fx.mgr.post(fx.id, channel, "second".to_string()).unwrap();
        assert_eq!(fx.thread(channel).len(), 2);

        // The server accepts the first and refuses the second, in order.
        fx.respond(posted(channel, 7));
        fx.respond(failed(channel, "this channel is locked"));

        let msgs = fx.thread(channel);
        assert_eq!(msgs.len(), 1, "the refused message must be removed");
        assert_eq!(msgs[0].seq, 7, "the accepted one keeps its real sequence");
        assert!(fx
            .conn()
            .last_error
            .as_deref()
            .is_some_and(|e| e.contains("locked")));
    }

    #[tokio::test]
    async fn a_download_whose_bytes_match_is_delivered() {
        let mut fx = Fx::new();
        let data = b"the real file".to_vec();
        let hash = blob_hash(&data);
        let rx = fx.download(&hash);
        fx.respond(PartyResponse::FileData {
            hash,
            data: data.clone(),
        });
        assert_eq!(rx.await.unwrap().unwrap(), data);
    }
}
