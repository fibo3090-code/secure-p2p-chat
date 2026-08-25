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
//! Every optimistic append is tracked in `pending_sends` so the server's ordered
//! `MessagePosted` / `ActionFailed` reply can confirm or retract exactly that
//! message.
//!
//! History: durable channel history is requested once per channel, when the
//! channel list arrives — **not** when `Joined` arrives. The client pipelines
//! `Join`, `ListChannels`, `ListMembers`, so the server answers `Joined` first,
//! while the channel list is still empty; seeding there meant the loop had
//! nothing to iterate and no channel history was ever fetched at all. Responses
//! are paged ([`MAX_HISTORY_BATCH`]), so a full page triggers a request for the
//! next one.

use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{anyhow, bail, Result};
use messenger_core::party::{
    blob_hash, dm_thread_id, AuditEntry, ChannelInfo, ChannelKind, Envelope, FileEntry, FileMeta,
    FilePermissions, MemberInfo, MessagePayload, PartyRequest, PartyResponse, QuotaInfo, Role,
    TrustTier, UploadTarget, MAX_HISTORY_BATCH, MAX_INLINE_FILE_BYTES, MAX_PARTY_FILE_BYTES,
    PARTY_CHUNK_BYTES,
};
use messenger_core::util::{current_timestamp_millis, parse_host_port};
use rsa::RsaPrivateKey;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

/// What happened when the client tried to join a community server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartyJoinOutcome {
    /// The server's identity was already pinned (or has just been confirmed by
    /// the user), the credentials have been sent, and the join is in flight.
    /// Watch `PartyServerConn::status` for the result.
    Joining {
        server_id: Uuid,
        fingerprint: String,
    },
    /// This address has never been joined before. Nothing was sent — not the
    /// username, not the password. Show the fingerprint and SAS for an
    /// out-of-band comparison, then call again with `trust_new_identity: true`.
    NeedsVerification { fingerprint: String, sas: String },
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

/// A chunked upload the client has offered but the server has not yet accepted.
/// Once `UploadReady` arrives the bytes are streamed and this is dropped.
struct PendingUpload {
    data: Vec<u8>,
}

/// A chunked download in progress: the bytes received so far and the caller
/// waiting for the whole file.
struct ChunkedDownload {
    data: Vec<u8>,
    tx: oneshot::Sender<Result<Vec<u8>, String>>,
}

/// Per-server connection state shown in the Party tab.
pub struct PartyServerConn {
    pub address: String,
    /// The username this client joined (or is joining) with; kept so the UI can
    /// offer one-click rejoin after a disconnect or restart.
    pub username: String,
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
    /// The most recent successful governance action, for a confirmation toast.
    pub last_notice: Option<String>,
    /// Files shared here that this member may see (the Drive panel), refreshed
    /// by `ListFiles`.
    pub files: Vec<FileEntry>,
    /// This member's storage usage, refreshed by `FetchQuota`.
    pub quota: Option<QuotaInfo>,
    /// The server's audit log, newest first. Only ever populated for admins.
    pub audit: Vec<AuditEntry>,
    /// Channels (and DM threads) whose durable history has been requested since
    /// this connection joined, so a refreshed channel list does not re-fetch
    /// everything on every broadcast.
    history_requested: HashSet<Uuid>,
    /// Threads with an outgoing message appended locally but not yet
    /// acknowledged by the server, oldest first.
    ///
    /// The server answers each send with exactly one `MessagePosted` or
    /// `ActionFailed`, in order, so the head of this queue identifies which
    /// thread a reply belongs to. Without the correlation a refused post simply
    /// stayed on screen looking delivered.
    ///
    /// It stores the *thread*, not a position in that thread's vector: a
    /// history page merges into the same vector and re-sorts it, which moves
    /// every unconfirmed message. Storing indices meant a page landing between
    /// a send and its reply made the reply stamp — or delete — the wrong
    /// message. An unconfirmed send is instead the envelope still carrying
    /// `seq == 0`, and the sort keeps those in append order at the end, so the
    /// oldest pending send in a thread is its first `seq == 0` envelope.
    pending_sends: VecDeque<Uuid>,
    outgoing_tx: mpsc::UnboundedSender<PartyRequest>,
    incoming_rx: mpsc::UnboundedReceiver<Incoming>,
    /// Chunked uploads waiting for the server to accept them, in the order they
    /// were started. `UploadReady` carries an id the client has never seen, so
    /// the only correlation available is arrival order — which is exact,
    /// because the server answers one request at a time down one connection.
    pending_uploads: VecDeque<PendingUpload>,
    /// Chunked downloads in progress, keyed by content hash: the bytes so far,
    /// plus the one-shot waiting for the whole file.
    chunked_downloads: HashMap<String, ChunkedDownload>,
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

    /// Append an outgoing message locally and remember where it landed, so the
    /// server's ordered reply can either stamp it with its real sequence or
    /// take it back off the screen.
    fn append_pending(&mut self, thread: Uuid, envelope: Envelope) {
        debug_assert_eq!(envelope.seq, 0, "an optimistic send must be unsequenced");
        self.messages.entry(thread).or_default().push(envelope);
        self.pending_sends.push_back(thread);
    }

    /// Position of the oldest unconfirmed send in `thread` — the first envelope
    /// the server has not yet assigned a sequence to.
    fn oldest_unconfirmed(&self, thread: Uuid) -> Option<usize> {
        self.messages.get(&thread)?.iter().position(|e| e.seq == 0)
    }

    /// The server acknowledged the oldest unconfirmed send: stamp the real
    /// sequence onto it so a later history page merges cleanly instead of
    /// duplicating it.
    fn confirm_oldest_pending(&mut self, seq: u64) {
        let Some(thread) = self.pending_sends.pop_front() else {
            return;
        };
        let Some(index) = self.oldest_unconfirmed(thread) else {
            return;
        };
        if let Some(env) = self
            .messages
            .get_mut(&thread)
            .and_then(|v| v.get_mut(index))
        {
            env.seq = seq;
        }
    }

    /// The server refused the oldest unconfirmed send: remove it. Leaving it on
    /// screen is the failure this exists to prevent — the user believes a
    /// message was delivered that the server never stored, and nothing later
    /// corrects them.
    fn retract_oldest_pending(&mut self) -> bool {
        let Some(thread) = self.pending_sends.pop_front() else {
            return false;
        };
        let Some(index) = self.oldest_unconfirmed(thread) else {
            return false;
        };
        match self.messages.get_mut(&thread) {
            Some(list) => {
                list.remove(index);
                true
            }
            None => false,
        }
    }

    /// Offer a chunked upload and hold its bytes until the server accepts.
    ///
    /// Nothing is streamed yet: the server checks the declared size against the
    /// hard ceiling and the uploader's allowance first, so a file that will be
    /// refused costs one round trip instead of a full transfer.
    fn start_chunked_upload(
        &mut self,
        name: String,
        mime: String,
        data: Vec<u8>,
        target: UploadTarget,
    ) -> Result<()> {
        let size = data.len() as u64;
        self.send(PartyRequest::StartUpload {
            name,
            mime,
            size,
            target,
        })?;
        self.pending_uploads.push_back(PendingUpload { data });
        Ok(())
    }

    /// Ask for a thread's durable history once. Returns false if it was already
    /// requested on this connection.
    fn request_history_once(&mut self, thread: Uuid, is_dm_peer: Option<Uuid>) -> bool {
        if !self.history_requested.insert(thread) {
            return false;
        }
        let req = match is_dm_peer {
            Some(with) => PartyRequest::FetchDmHistory { with, since_seq: 0 },
            None => PartyRequest::FetchHistory {
                channel: thread,
                since_seq: 0,
            },
        };
        let _ = self.send(req);
        true
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
    ///
    /// `expected_fingerprint` is the identity this address was pinned to on a
    /// previous join. It is checked **before the `Join` frame is written**,
    /// because that frame carries the user's community username and password:
    /// verifying afterwards means a server that swapped its key — a redeploy, or
    /// someone who took over the address — has already been handed the
    /// credentials it was supposed to prove it deserved. The handshake is
    /// authenticated, so a mismatch here is the whole point of pinning.
    ///
    /// When there is **no** pin — the first time this address is used —
    /// `trust_new_identity` decides what happens. `false` (the default the UI
    /// starts with) connects far enough to learn the server's fingerprint and
    /// SAS, then hangs up without sending anything and returns
    /// [`PartyJoinOutcome::NeedsVerification`]. A peer-to-peer first contact has
    /// always stopped for the SAS prompt here; a community first contact used to
    /// hand over the username and password to whatever key answered the address
    /// and pin it afterwards, which is trust-on-first-use with the "trust" step
    /// left out.
    pub async fn connect_and_join(
        &mut self,
        address: &str,
        username: &str,
        password: Option<String>,
        privkey: &RsaPrivateKey,
        expected_fingerprint: Option<&str>,
        trust_new_identity: bool,
    ) -> Result<PartyJoinOutcome> {
        use messenger_core::party::PartyClient;

        self.last_error = None;
        let (host, port) = parse_host_port(address, Some(messenger_core::PORT_DEFAULT))?;
        let stream = tokio::net::TcpStream::connect((host.as_str(), port)).await?;
        let client = PartyClient::connect(stream, privkey, Uuid::new_v4()).await?;
        let fingerprint = client.server_fingerprint().to_string();
        match expected_fingerprint.filter(|p| !p.is_empty()) {
            Some(pinned) if pinned != fingerprint => {
                // Drop the connection without sending anything. `client` owns the
                // stream, so returning here closes it.
                bail!(
                    "SECURITY: this server's identity changed since you last joined \
                     (expected {}…, got {}…). Your username and password were NOT sent. \
                     If the operator redeployed the server this may be expected — leave \
                     the saved community and rejoin to trust the new identity. \
                     Otherwise, do not proceed.",
                    &pinned[..16.min(pinned.len())],
                    &fingerprint[..16.min(fingerprint.len())]
                );
            }
            Some(_) => {} // pinned and matching: proceed
            None if !trust_new_identity => {
                // First contact with this address. Hang up before the `Join`
                // frame — it carries the credentials — and let the shell show
                // the SAS for an out-of-band check.
                let sas = client.sas().to_string();
                return Ok(PartyJoinOutcome::NeedsVerification { fingerprint, sas });
            }
            None => {} // the user has just confirmed this identity
        }
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
            username: username.to_string(),
            server_name: String::new(),
            server_fingerprint: fingerprint.clone(),
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
            pending_sends: VecDeque::new(),
            outgoing_tx,
            incoming_rx,
            pending_downloads: HashMap::new(),
            pending_uploads: VecDeque::new(),
            chunked_downloads: HashMap::new(),
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
        Ok(PartyJoinOutcome::Joining {
            server_id,
            fingerprint,
        })
    }

    /// Post a message to a channel. The message is appended locally right away
    /// (the server won't echo the sender's own broadcast).
    pub fn post(&mut self, server_id: Uuid, channel: Uuid, text: String) -> Result<()> {
        let conn = self
            .servers
            .get_mut(&server_id)
            .ok_or_else(|| anyhow!("unknown server"))?;
        let sender = conn.member_id.ok_or_else(|| anyhow!("not joined yet"))?;
        conn.append_pending(
            channel,
            Envelope {
                tier: TrustTier::Administered,
                sender,
                channel,
                seq: 0, // provisional; ordering is by arrival
                timestamp: current_timestamp_millis(),
                payload: MessagePayload::Text(text.clone()),
            },
        );
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
        conn.append_pending(
            thread,
            Envelope {
                tier: TrustTier::Administered,
                sender: me,
                channel: thread,
                seq: 0,
                timestamp: current_timestamp_millis(),
                payload: MessagePayload::Text(text.clone()),
            },
        );
        conn.send(PartyRequest::SendDm { to, text })
    }

    /// Request DM history with another member (offline catch-up). Idempotent
    /// per connection: opening the thread repeatedly does not re-fetch it.
    pub fn fetch_dm_history(&mut self, server_id: Uuid, with: Uuid) -> Result<()> {
        let conn = self
            .servers
            .get_mut(&server_id)
            .ok_or_else(|| anyhow!("unknown server"))?;
        let me = conn.member_id.ok_or_else(|| anyhow!("not joined yet"))?;
        conn.request_history_once(dm_thread_id(me, with), Some(with));
        Ok(())
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

    /// Create a channel of a specific kind. `members` seeds a private channel's
    /// membership and is ignored for every other kind.
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

    /// Ask for the files this member may see; the answer lands in
    /// `PartyServerConn::files`.
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

    /// Post a file you already hold into another channel or DM, without
    /// re-uploading it. `from` is the location you are sharing it *from*, which
    /// is where your right to do so comes from.
    pub fn share_file(
        &self,
        server_id: Uuid,
        hash: String,
        from: Uuid,
        target: UploadTarget,
    ) -> Result<()> {
        self.conn(server_id)?
            .send(PartyRequest::ShareFile { hash, from, target })
    }

    /// Change what a shared file grants — by default (`member: None`) or for one
    /// member. The server refuses anything the caller does not hold themselves.
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

    /// Ask for the audit log; the answer lands in `PartyServerConn::audit`.
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
        conn.append_pending(
            channel,
            Envelope {
                tier: TrustTier::Administered,
                sender,
                channel,
                seq: 0,
                timestamp: current_timestamp_millis(),
                payload: MessagePayload::File(meta),
            },
        );
        // Small files go inline in one request; anything larger is streamed,
        // because a single frame is bounded by MAX_PACKET_SIZE.
        if data.len() <= MAX_INLINE_FILE_BYTES {
            return conn.send(PartyRequest::PostFile {
                channel,
                name,
                mime,
                data,
            });
        }
        conn.start_chunked_upload(name, mime, data, UploadTarget::Channel(channel))
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
        conn.append_pending(
            thread,
            Envelope {
                tier: TrustTier::Administered,
                sender: me,
                channel: thread,
                seq: 0,
                timestamp: current_timestamp_millis(),
                payload: MessagePayload::File(meta),
            },
        );
        if data.len() <= MAX_INLINE_FILE_BYTES {
            return conn.send(PartyRequest::SendFileDm {
                to,
                name,
                mime,
                data,
            });
        }
        conn.start_chunked_upload(name, mime, data, UploadTarget::Dm(to))
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
        // Ask for the size the listing reports, when it knows it: a file past
        // the inline limit cannot come back in one frame, so it has to be
        // streamed. Anything unknown takes the single-frame path, which the
        // server answers for everything it can fit.
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

    /// Request a channel's durable history (offline catch-up). Idempotent per
    /// connection — `seed_history` already asks for every visible channel when
    /// the directory arrives, so this is the explicit "open this channel" path.
    pub fn fetch_history(&mut self, server_id: Uuid, channel: Uuid) -> Result<()> {
        let conn = self
            .servers
            .get_mut(&server_id)
            .ok_or_else(|| anyhow!("unknown server"))?;
        conn.request_history_once(channel, None);
        Ok(())
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

    /// Leave a community: drop the connection and forget its local state. Dropping
    /// the connection's channels makes its background task exit (and any in-flight
    /// download receivers error) — the server keeps the membership server-side, so
    /// rejoining later with the same identity resumes it. Returns whether a server
    /// was actually removed.
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

/// Build the [`FileMeta`] for an outgoing upload, computing its content hash and
/// rejecting payloads the server would refuse (`> MAX_INLINE_FILE_BYTES`) so the
/// user gets a clear error instead of a silent server-side rejection.
fn file_meta(name: &str, mime: &str, data: &[u8]) -> Result<FileMeta> {
    // Files past the inline limit are streamed rather than refused; only the
    // server's hard ceiling is a real "no".
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

/// Fail every in-flight download on a connection with `reason`, so callers awaiting
/// a download don't hang when the server reports an error or the connection drops.
fn fail_pending_downloads(conn: &mut PartyServerConn, reason: &str) {
    for (_, tx) in conn.pending_downloads.drain() {
        let _ = tx.send(Err(reason.to_string()));
    }
    // Chunked downloads wait across many round trips, so they are the ones most
    // likely to be in flight when something goes wrong.
    for (_, state) in conn.chunked_downloads.drain() {
        let _ = state.tx.send(Err(reason.to_string()));
    }
}

/// Request durable history for every thread this connection can see and has not
/// asked about yet.
///
/// Called after `Joined`, `Channels` and `Members` because any of the three can
/// be the one that completes the picture: the channel list decides which
/// channels exist, the member list decides which DM threads do, and neither is
/// known when `Joined` arrives.
fn seed_history(conn: &mut PartyServerConn) {
    if conn.member_id.is_none() {
        return; // the server refuses history until we have joined
    }
    let channels: Vec<Uuid> = conn.channels.iter().map(|c| c.id).collect();
    for ch in channels {
        conn.request_history_once(ch, None);
    }
    let Some(me) = conn.member_id else { return };
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
            // History is *not* seeded here. `Joined` arrives before `Channels`
            // (the requests are pipelined in that order), so `conn.channels` is
            // still empty — which is exactly why channel history was never
            // fetched at all. It is requested when the channel list lands.
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
            let thread = conn.messages.entry(env.channel).or_default();
            // A live broadcast can race a history page carrying the same
            // envelope; both are keyed by the server-assigned sequence.
            if env.seq == 0 || !thread.iter().any(|e| e.seq == env.seq) {
                thread.push(env);
            }
        }
        PartyResponse::History(items) => {
            let page_was_full = items.len() >= MAX_HISTORY_BATCH;
            let threads: HashSet<Uuid> = items.iter().map(|e| e.channel).collect();
            for th in threads {
                let page: Vec<Envelope> =
                    items.iter().filter(|e| e.channel == th).cloned().collect();
                let existing = conn.messages.entry(th).or_default();
                // Merge rather than replace: history is paged now, so a later
                // page must not throw away the earlier one. Server-assigned
                // sequences are the identity; unconfirmed local sends (seq 0)
                // are kept so a message in flight does not blink out.
                for env in page {
                    if !existing.iter().any(|e| e.seq == env.seq) {
                        existing.push(env);
                    }
                }
                existing.sort_by_key(|e| if e.seq == 0 { u64::MAX } else { e.seq });

                // A full page means there is probably more behind it.
                if page_was_full {
                    if let Some(last) = existing.iter().rfind(|e| e.seq > 0) {
                        let since_seq = last.seq;
                        let dm_peer = conn.member_id.and_then(|me| {
                            conn.members
                                .iter()
                                .map(|m| m.id)
                                .find(|other| dm_thread_id(me, *other) == th)
                        });
                        let _ = conn.send(match dm_peer {
                            Some(with) => PartyRequest::FetchDmHistory { with, since_seq },
                            None => PartyRequest::FetchHistory {
                                channel: th,
                                since_seq,
                            },
                        });
                    }
                }
            }
        }
        // The server stored it: stamp the real sequence onto the message we
        // already put on screen.
        PartyResponse::MessagePosted { seq, .. } => conn.confirm_oldest_pending(seq),
        // The server refused a send. Take the message back off the screen —
        // leaving it there tells the user it was delivered when it never was.
        PartyResponse::ActionFailed { reason, .. } => {
            let retracted = conn.retract_oldest_pending();
            conn.last_error = Some(if retracted {
                format!("Not sent: {reason}")
            } else {
                reason
            });
        }
        // Downloaded file bytes: hand them to the awaiting `request_download` caller
        // (keyed by content hash). Unsolicited/duplicate data has no waiter and is
        // dropped.
        PartyResponse::FileData { hash, data } => {
            if let Some(tx) = conn.pending_downloads.remove(&hash) {
                // The hash *is* the integrity check — that is the whole point of
                // content addressing, and it costs one SHA-256 to use. Matching
                // on the hash the server echoed back proves nothing about the
                // bytes it attached to it.
                let actual = blob_hash(&data);
                let _ = if actual == hash {
                    tx.send(Ok(data))
                } else {
                    tracing::error!(
                        expected = %hash,
                        actual = %actual,
                        "community server returned bytes that do not match the requested content hash"
                    );
                    tx.send(Err(
                        "The server sent a file that does not match the one that was requested. It was not saved.".to_string(),
                    ))
                };
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
        // The channel list is per member now (private channels are filtered), so
        // the server nudges instead of broadcasting a list that would be wrong
        // for somebody. Ask for our own.
        PartyResponse::DirectoryChanged => {
            let _ = conn.send(PartyRequest::ListChannels);
            let _ = conn.send(PartyRequest::ListMembers);
        }
        // The server accepted a chunked upload: stream the bytes it is now
        // waiting for. Correlated by arrival order — the id is the server's,
        // so the client has nothing else to match on, and the server answers
        // one request at a time down one connection.
        PartyResponse::UploadReady { upload, chunk_size } => {
            let Some(pending) = conn.pending_uploads.pop_front() else {
                let _ = conn.send(PartyRequest::CancelUpload { upload });
                return;
            };
            let chunk = (chunk_size as usize).clamp(1, PARTY_CHUNK_BYTES);
            for part in pending.data.chunks(chunk) {
                if conn
                    .send(PartyRequest::UploadChunk {
                        upload,
                        data: part.to_vec(),
                    })
                    .is_err()
                {
                    return; // the connection went away; nothing to clean up here
                }
            }
            let _ = conn.send(PartyRequest::FinishUpload { upload });
        }
        // One chunk of a chunked download. Ask for the next until the file is
        // whole, then hand it over — verified against the hash we asked for.
        PartyResponse::FileChunk {
            hash,
            offset,
            total,
            data,
        } => {
            let Some(state) = conn.chunked_downloads.get_mut(&hash) else {
                return; // unsolicited, or already completed/failed
            };
            // Out-of-order or duplicated chunk: the stream is no longer
            // reconstructible, so fail rather than assemble something wrong.
            if offset != state.data.len() as u64 {
                if let Some(state) = conn.chunked_downloads.remove(&hash) {
                    let _ = state
                        .tx
                        .send(Err("the server sent file data out of order".to_string()));
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
            let Some(state) = conn.chunked_downloads.remove(&hash) else {
                return;
            };
            // The hash is the integrity check, and it costs one SHA-256 to use.
            let actual = blob_hash(&state.data);
            let _ = if actual == hash {
                state.tx.send(Ok(state.data))
            } else {
                tracing::error!(expected = %hash, actual = %actual, "community server returned chunks that do not match the requested content hash");
                state.tx.send(Err(
                    "The server sent a file that does not match the one that was requested. It was not saved.".to_string(),
                ))
            };
        }
        PartyResponse::Files(files) => conn.files = files,
        PartyResponse::Quota(quota) => conn.quota = Some(quota),
        PartyResponse::AuditLog(entries) => conn.audit = entries,
        PartyResponse::Ok(message) => conn.last_notice = Some(message),
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
            username: "alice".to_string(),
            server_name: String::new(),
            server_fingerprint: "FP".to_string(),
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
            pending_sends: VecDeque::new(),
            outgoing_tx,
            incoming_rx,
            pending_downloads: HashMap::new(),
            pending_uploads: VecDeque::new(),
            chunked_downloads: HashMap::new(),
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
                members: Vec::new(),
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
                role: Role::Member,
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

    /// A file too large for one frame is offered first and streamed only once
    /// the server accepts it, so a refusal costs a round trip instead of the
    /// whole transfer.
    #[test]
    fn a_large_file_is_offered_first_then_streamed_in_chunks() {
        let (mut mgr, id, tx, mut out) = manager_with_server();
        let me = Uuid::new_v4();
        mgr.servers.get_mut(&id).unwrap().member_id = Some(me);
        let channel = Uuid::new_v4();
        let payload: Vec<u8> = (0..(MAX_INLINE_FILE_BYTES + 1234))
            .map(|i| (i % 253) as u8)
            .collect();

        mgr.send_file(
            id,
            channel,
            "big.bin".into(),
            "application/octet-stream".into(),
            payload.clone(),
        )
        .unwrap();

        // Offered, not sent: the bytes are still held client-side.
        match out.try_recv().unwrap() {
            PartyRequest::StartUpload { size, target, .. } => {
                assert_eq!(size, payload.len() as u64);
                assert_eq!(target, UploadTarget::Channel(channel));
            }
            other => panic!("expected StartUpload, got {other:?}"),
        }
        assert!(out.try_recv().is_err(), "nothing streams before acceptance");
        // …and it is already on screen, so the user sees it immediately.
        assert_eq!(mgr.servers[&id].messages[&channel].len(), 1);

        // The server accepts; the client streams and finishes.
        let upload = Uuid::new_v4();
        tx.send(Incoming::Response(PartyResponse::UploadReady {
            upload,
            chunk_size: PARTY_CHUNK_BYTES as u32,
        }))
        .unwrap();
        mgr.poll_events();

        let mut streamed = Vec::new();
        let mut finished = false;
        while let Ok(req) = out.try_recv() {
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

    /// A large download is reassembled across chunks and checked against the
    /// hash that was asked for.
    #[test]
    fn a_large_download_is_reassembled_from_chunks() {
        let (mut mgr, id, tx, mut out) = manager_with_server();
        let me = Uuid::new_v4();
        mgr.servers.get_mut(&id).unwrap().member_id = Some(me);
        let channel = Uuid::new_v4();
        // Past the inline limit, which is what makes this a chunked download.
        let payload: Vec<u8> = (0..(MAX_INLINE_FILE_BYTES + 500))
            .map(|i| (i % 199) as u8)
            .collect();
        let hash = blob_hash(&payload);

        // The client learns the size from a file message in history, which is
        // what tells it this cannot arrive in a single frame.
        mgr.servers
            .get_mut(&id)
            .unwrap()
            .messages
            .entry(channel)
            .or_default()
            .push(Envelope {
                tier: TrustTier::Administered,
                sender: me,
                channel,
                seq: 1,
                timestamp: 0,
                payload: MessagePayload::File(FileMeta {
                    hash: hash.clone(),
                    name: "big.bin".into(),
                    size: payload.len() as u64,
                    mime: "application/octet-stream".into(),
                }),
            });

        let mut rx = mgr.request_download(id, hash.clone()).unwrap();
        match out.try_recv().unwrap() {
            PartyRequest::DownloadChunk { hash: h, offset } => {
                assert_eq!(h, hash);
                assert_eq!(offset, 0);
            }
            other => panic!("expected DownloadChunk, got {other:?}"),
        }

        // Feed it back one chunk at a time, following the offsets it asks for.
        let total = payload.len() as u64;
        let mut offset = 0usize;
        while offset < payload.len() {
            let end = (offset + PARTY_CHUNK_BYTES).min(payload.len());
            tx.send(Incoming::Response(PartyResponse::FileChunk {
                hash: hash.clone(),
                offset: offset as u64,
                total,
                data: payload[offset..end].to_vec(),
            }))
            .unwrap();
            mgr.poll_events();
            offset = end;
            if offset < payload.len() {
                match out.try_recv().unwrap() {
                    PartyRequest::DownloadChunk { offset: next, .. } => {
                        assert_eq!(next, offset as u64, "asks for exactly what it is missing");
                    }
                    other => panic!("expected the next DownloadChunk, got {other:?}"),
                }
            }
        }
        assert_eq!(rx.try_recv().unwrap(), Ok(payload));
    }

    /// A chunk that does not continue where the last one stopped means the
    /// stream is no longer reconstructible — fail rather than assemble
    /// something that is not the file.
    #[test]
    fn an_out_of_order_chunk_fails_the_download() {
        let (mut mgr, id, tx, _out) = manager_with_server();
        let me = Uuid::new_v4();
        mgr.servers.get_mut(&id).unwrap().member_id = Some(me);
        let hash = "abc123".to_string();
        let (otx, mut rx) = oneshot::channel();
        mgr.servers.get_mut(&id).unwrap().chunked_downloads.insert(
            hash.clone(),
            ChunkedDownload {
                data: Vec::new(),
                tx: otx,
            },
        );

        tx.send(Incoming::Response(PartyResponse::FileChunk {
            hash: hash.clone(),
            offset: 999, // not where we are
            total: 4096,
            data: vec![1, 2, 3],
        }))
        .unwrap();
        mgr.poll_events();

        assert_eq!(
            rx.try_recv().unwrap(),
            Err("the server sent file data out of order".to_string())
        );
    }

    #[test]
    fn request_download_fails_when_the_server_reports_an_error() {
        let (mut mgr, id, tx, _out) = manager_with_server();
        mgr.servers.get_mut(&id).unwrap().member_id = Some(Uuid::new_v4());

        let mut rx = mgr.request_download(id, "deadbeef".to_string()).unwrap();

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
    fn remove_server_forgets_it_and_fails_inflight_downloads() {
        let (mut mgr, id, _tx, _out) = manager_with_server();
        mgr.servers.get_mut(&id).unwrap().member_id = Some(Uuid::new_v4());

        // An in-flight download's receiver must error (sender dropped), not hang.
        let mut rx = mgr.request_download(id, "abc123".to_string()).unwrap();

        assert!(mgr.remove_server(id), "the server is removed");
        assert!(mgr.server(id).is_none());
        assert!(mgr.server_ids().is_empty());
        assert!(
            rx.try_recv().is_err(),
            "pending download receivers error out on leave"
        );

        // Removing again is a no-op.
        assert!(!mgr.remove_server(id));
    }

    #[test]
    fn oversized_file_upload_is_rejected_without_side_effects() {
        let (mut mgr, id, _tx, mut out) = manager_with_server();
        let me = Uuid::new_v4();
        mgr.servers.get_mut(&id).unwrap().member_id = Some(me);
        let channel = Uuid::new_v4();
        // Past the *server's ceiling*, not merely past the inline limit —
        // anything between the two is streamed rather than refused.
        let too_big = vec![0u8; MAX_PARTY_FILE_BYTES as usize + 1];

        assert!(
            mgr.send_file(
                id,
                channel,
                "big.bin".to_string(),
                "application/octet-stream".to_string(),
                too_big
            )
            .is_err(),
            "a file past the server ceiling must be rejected"
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

    /// The client pipelines Join → ListChannels → ListMembers, so `Joined`
    /// arrives while the channel list is still empty. Seeding history there
    /// iterated nothing, and durable channel history was never fetched at all —
    /// every channel came up empty after a restart even though the server had
    /// it all in SQLite.
    #[test]
    fn history_is_requested_once_the_channel_list_arrives() {
        let (mut mgr, _id, tx, mut out) = manager_with_server();
        let me = Uuid::new_v4();
        let channel = Uuid::new_v4();
        let peer = Uuid::new_v4();

        // Exactly the order the server answers in.
        tx.send(Incoming::Response(PartyResponse::Joined {
            member_id: me,
            server_name: "Srv".to_string(),
            tier: TrustTier::Administered,
        }))
        .unwrap();
        tx.send(Incoming::Response(PartyResponse::Channels(vec![
            ChannelInfo {
                id: channel,
                name: "general".to_string(),
                kind: ChannelKind::Public,
                members: Vec::new(),
            },
        ])))
        .unwrap();
        tx.send(Incoming::Response(PartyResponse::Members(vec![
            MemberInfo {
                id: me,
                username: "alice".to_string(),
                online: true,
                role: Role::Member,
            },
            MemberInfo {
                id: peer,
                username: "bob".to_string(),
                online: true,
                role: Role::Member,
            },
        ])))
        .unwrap();
        mgr.poll_events();

        let mut requested_channel = false;
        let mut requested_dm = false;
        while let Ok(req) = out.try_recv() {
            match req {
                PartyRequest::FetchHistory { channel: c, .. } if c == channel => {
                    requested_channel = true
                }
                PartyRequest::FetchDmHistory { with, .. } if with == peer => requested_dm = true,
                _ => {}
            }
        }
        assert!(requested_channel, "channel history must be fetched on join");
        assert!(requested_dm, "DM history must be fetched on join");

        // Idempotent: a re-broadcast channel list (someone created a channel)
        // must not refetch everything.
        tx.send(Incoming::Response(PartyResponse::Channels(vec![
            ChannelInfo {
                id: channel,
                name: "general".to_string(),
                kind: ChannelKind::Public,
                members: Vec::new(),
            },
        ])))
        .unwrap();
        mgr.poll_events();
        assert!(
            out.try_recv().is_err(),
            "already-seeded threads must not be re-requested"
        );
    }

    /// History is paged now, so a later page must merge into the earlier one
    /// rather than replace it — and a full page asks for the next.
    #[test]
    fn history_pages_are_merged_and_continued() {
        let (mut mgr, id, tx, mut out) = manager_with_server();
        let me = Uuid::new_v4();
        let channel = Uuid::new_v4();
        mgr.servers.get_mut(&id).unwrap().member_id = Some(me);

        let page: Vec<Envelope> = (1..=MAX_HISTORY_BATCH as u64)
            .map(|seq| envelope(channel, me, seq, &format!("m{seq}")))
            .collect();
        tx.send(Incoming::Response(PartyResponse::History(page)))
            .unwrap();
        mgr.poll_events();

        assert_eq!(
            mgr.server(id).unwrap().messages[&channel].len(),
            MAX_HISTORY_BATCH
        );
        let asked_again = std::iter::from_fn(|| out.try_recv().ok()).any(|r| {
            matches!(r, PartyRequest::FetchHistory { channel: c, since_seq }
                if c == channel && since_seq == MAX_HISTORY_BATCH as u64)
        });
        assert!(asked_again, "a full page must be followed by the next one");

        // The next page appends rather than replacing.
        tx.send(Incoming::Response(PartyResponse::History(vec![envelope(
            channel,
            me,
            MAX_HISTORY_BATCH as u64 + 1,
            "tail",
        )])))
        .unwrap();
        mgr.poll_events();
        assert_eq!(
            mgr.server(id).unwrap().messages[&channel].len(),
            MAX_HISTORY_BATCH + 1,
            "a later page must not throw away the earlier one"
        );
    }

    /// A post the server refuses used to stay on screen looking delivered: the
    /// error arrived uncorrelated and nothing ever took the message back.
    #[test]
    fn a_refused_send_is_taken_back_off_the_screen() {
        let (mut mgr, id, tx, _out) = manager_with_server();
        let me = Uuid::new_v4();
        let channel = Uuid::new_v4();
        mgr.servers.get_mut(&id).unwrap().member_id = Some(me);

        mgr.post(id, channel, "first".to_string()).unwrap();
        mgr.post(id, channel, "second".to_string()).unwrap();
        assert_eq!(mgr.server(id).unwrap().messages[&channel].len(), 2);

        // The server accepts the first and refuses the second, in order.
        tx.send(Incoming::Response(PartyResponse::MessagePosted {
            channel,
            seq: 7,
        }))
        .unwrap();
        tx.send(Incoming::Response(PartyResponse::ActionFailed {
            channel,
            reason: "this channel is locked".to_string(),
        }))
        .unwrap();
        mgr.poll_events();

        let conn = mgr.server(id).unwrap();
        let msgs = &conn.messages[&channel];
        assert_eq!(msgs.len(), 1, "the refused message must be removed");
        assert_eq!(msgs[0].seq, 7, "the accepted one keeps its real sequence");
        assert!(conn
            .last_error
            .as_deref()
            .is_some_and(|e| e.contains("locked")));
    }

    /// A history page landing between a send and its reply must not misdirect
    /// that reply. The page merges into the same vector and re-sorts it, so any
    /// position recorded at send time is stale by the time the answer arrives —
    /// which used to stamp the sequence onto, or delete, somebody else's
    /// message.
    #[test]
    fn a_history_page_between_a_send_and_its_reply_does_not_misdirect_it() {
        let (mut mgr, id, tx, _out) = manager_with_server();
        let me = Uuid::new_v4();
        let peer = Uuid::new_v4();
        let channel = Uuid::new_v4();
        mgr.servers.get_mut(&id).unwrap().member_id = Some(me);

        mgr.post(id, channel, "mine".to_string()).unwrap();

        // Durable history arrives first and is merged ahead of the unconfirmed
        // send, moving it to the end of the vector.
        tx.send(Incoming::Response(PartyResponse::History(vec![
            envelope(channel, peer, 1, "older"),
            envelope(channel, peer, 2, "newer"),
        ])))
        .unwrap();
        tx.send(Incoming::Response(PartyResponse::ActionFailed {
            channel,
            reason: "this channel is locked".to_string(),
        }))
        .unwrap();
        mgr.poll_events();

        let msgs = &mgr.server(id).unwrap().messages[&channel];
        assert_eq!(msgs.len(), 2, "only the refused send is removed");
        assert!(
            msgs.iter().all(|e| e.sender == peer),
            "the peer's history survived; the local send was the one retracted"
        );
    }

    /// Content addressing is the integrity check. Matching on the hash the
    /// server echoed back proves nothing about the bytes attached to it.
    #[tokio::test]
    async fn a_download_whose_bytes_do_not_match_its_hash_is_refused() {
        let (mut mgr, id, tx, _out) = manager_with_server();
        let hash = blob_hash(b"the real file");
        let rx = mgr.request_download(id, hash.clone()).unwrap();

        tx.send(Incoming::Response(PartyResponse::FileData {
            hash,
            data: b"something else entirely".to_vec(),
        }))
        .unwrap();
        mgr.poll_events();

        let err = rx
            .await
            .expect("the waiter is completed")
            .expect_err("mismatched bytes must not be handed to the caller");
        assert!(err.contains("does not match"), "{err}");
    }

    #[tokio::test]
    async fn a_download_whose_bytes_match_is_delivered() {
        let (mut mgr, id, tx, _out) = manager_with_server();
        let data = b"the real file".to_vec();
        let hash = blob_hash(&data);
        let rx = mgr.request_download(id, hash.clone()).unwrap();

        tx.send(Incoming::Response(PartyResponse::FileData {
            hash,
            data: data.clone(),
        }))
        .unwrap();
        mgr.poll_events();

        assert_eq!(rx.await.unwrap().unwrap(), data);
    }
}
