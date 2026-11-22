//! Party application protocol: the client/server wire contract, carried inside
//! the established v3 encrypted tunnel (`network::session`).
//!
//! Two trust tiers share one data model (`docs/platform_spec.md`):
//! Administered (server stores plaintext; implemented) and E2EE (Phase 4).

use rsa::RsaPrivateKey;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite, ReadHalf, WriteHalf};
use uuid::Uuid;

use crate::core::{recv_packet, send_packet, AesCipher};
use crate::network::client_handshake;

/// Trust tier of a server. Only `Administered` exists today; the UIs disclose
/// "the operator can read messages" regardless of this field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TrustTier {
    /// Server stores plaintext; enables offline buffering, search, admin-read.
    #[default]
    Administered,
    /// Server stores ciphertext only; admin cannot read (Phase 4).
    E2EE,
}

/// Largest inline upload; larger files use [`PartyRequest::StartUpload`].
pub const MAX_INLINE_FILE_BYTES: usize = 4 * 1024 * 1024; // 4 MiB

/// The `Error` answering a download of a file that is unknown *or* not visible
/// (deliberately the same). Errors are uncorrelated, so the client recognises
/// a failed download by this text.
pub const UNKNOWN_FILE_ERROR: &str = "unknown file";

/// File bytes per [`PartyRequest::UploadChunk`] / [`PartyResponse::FileChunk`].
pub const PARTY_CHUNK_BYTES: usize = 256 * 1024; // 256 KiB

/// Largest file a server accepts at all, inline or chunked (separate from the
/// per-member quota).
pub const MAX_PARTY_FILE_BYTES: u64 = 100 * 1024 * 1024; // 100 MiB

/// Where a chunked upload will be posted once it completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UploadTarget {
    Channel(Uuid),
    /// A direct message to this member.
    Dm(Uuid),
}

/// A file shared in a channel or DM. Bytes are stored content-addressed by
/// `hash` (lowercase hex SHA-256); `name` is per message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMeta {
    pub hash: String,
    pub name: String,
    pub size: u64,
    pub mime: String,
}

/// An [`Envelope`]'s payload; plaintext in the Administered tier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessagePayload {
    Text(String),
    /// A server-stored file, fetched via [`PartyRequest::DownloadFile`].
    File(FileMeta),
}

/// Content address (lowercase hex SHA-256) of a blob's bytes, for dedup.
pub fn blob_hash(data: &[u8]) -> String {
    let mut hasher = BlobHasher::new();
    hasher.update(data);
    hasher.finish()
}

/// The same content address computed incrementally, so a chunked upload's hash
/// is known by completion without a 100 MiB pass under the server lock. Shares
/// its implementation with [`blob_hash`] so the two can never disagree.
#[derive(Debug, Clone, Default)]
pub struct BlobHasher {
    inner: sha2::Sha256,
}

impl BlobHasher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, data: &[u8]) {
        use sha2::Digest;
        self.inner.update(data);
    }

    pub fn finish(self) -> String {
        use sha2::Digest;
        hex::encode(self.inner.finalize())
    }
}

/// A single stored/transported message. One data model across both tiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub tier: TrustTier,
    /// Member id of the sender.
    pub sender: Uuid,
    /// Channel (or, later, DM thread) the message belongs to.
    pub channel: Uuid,
    /// Per-channel monotonic sequence number assigned by the server.
    pub seq: u64,
    /// Unix milliseconds.
    pub timestamp: u64,
    pub payload: MessagePayload,
}

/// A member's authority. Ordered, so `role >= Role::Admin` is the permission
/// test. There is exactly one [`Role::Owner`], claimed with the one-time
/// claim token or `--owner` fingerprint (joining first grants nothing); it
/// cannot be demoted and is never duplicated.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default, Hash,
)]
pub enum Role {
    /// Read-only. May read the channels they can see; may not post or upload.
    Guest,
    /// The default. Post, share files, download, create channels.
    #[default]
    Member,
    /// Manage channels, files anyone uploaded, and the roles below their own.
    Admin,
    /// The operator. Everything an admin can do, plus managing admins.
    Owner,
}

impl Role {
    /// Human label, used by both front-ends and the audit log.
    pub fn label(self) -> &'static str {
        match self {
            Role::Guest => "Guest",
            Role::Member => "Member",
            Role::Admin => "Admin",
            Role::Owner => "Owner",
        }
    }

    /// May post and upload at all (a guest is read-only everywhere).
    pub fn can_write(self) -> bool {
        self >= Role::Member
    }

    pub fn can_create_channel(self) -> bool {
        self >= Role::Member
    }

    /// Administers the server: channel access, deletions, restricted channels,
    /// roles.
    pub fn is_admin(self) -> bool {
        self >= Role::Admin
    }

    /// Whether `self` may assign `target`: only roles strictly below your own,
    /// so an admin cannot mint admins or owners.
    pub fn may_assign(self, target: Role) -> bool {
        self.is_admin() && target < self
    }
}

/// Public information about a member, as shown in the directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberInfo {
    pub id: Uuid,
    pub username: String,
    pub online: bool,
    /// Appended field: bincode is positional, so this must stay last.
    pub role: Role,
}

/// Channel kind: the per-channel access rule, enforced by
/// [`ChannelKind::may_read`] / [`ChannelKind::may_post`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, Hash)]
pub enum ChannelKind {
    /// Everyone who has joined the server may read and post.
    #[default]
    Public,
    /// Only members on the channel's own membership list may read or post.
    Private,
    /// Everyone may read; only admins may post (freezes a channel).
    Locked,
    /// Everyone may read; only admins may post. Used for announcements.
    Announce,
}

impl ChannelKind {
    /// Whether a member with `role` may read this channel; `is_channel_member`
    /// only matters for [`Self::Private`].
    pub fn may_read(self, role: Role, is_channel_member: bool) -> bool {
        match self {
            ChannelKind::Public | ChannelKind::Locked | ChannelKind::Announce => true,
            // Admins always see private channels, so they can moderate them.
            ChannelKind::Private => is_channel_member || role.is_admin(),
        }
    }

    /// Whether a member with `role` may post here. A guest never may.
    pub fn may_post(self, role: Role, is_channel_member: bool) -> Result<(), &'static str> {
        if !role.can_write() {
            return Err("your role on this server is read-only");
        }
        let (allowed, reason) = match self {
            ChannelKind::Public => (true, ""),
            ChannelKind::Private => (
                is_channel_member || role.is_admin(),
                "you are not a member of this channel",
            ),
            ChannelKind::Locked => (role.is_admin(), "this channel is locked"),
            ChannelKind::Announce => (
                role.is_admin(),
                "only admins may post to an announcement channel",
            ),
        };
        if allowed {
            Ok(())
        } else {
            Err(reason)
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ChannelKind::Public => "Public",
            ChannelKind::Private => "Private",
            ChannelKind::Locked => "Locked",
            ChannelKind::Announce => "Announce",
        }
    }
}

/// Public information about a channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelInfo {
    pub id: Uuid,
    pub name: String,
    pub kind: ChannelKind,
    /// Members of a [`ChannelKind::Private`] channel (empty otherwise).
    /// Appended field: keep it last.
    pub members: Vec<Uuid>,
}

/// Client → server messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartyRequest {
    /// Join the server with a chosen username and the optional server password.
    Join {
        username: String,
        password: Option<String>,
    },
    /// Request the member directory.
    ListMembers,
    /// Request the channel list.
    ListChannels,
    /// Post a text message to a channel.
    PostMessage { channel: Uuid, text: String },
    /// Fetch channel history after `since_seq` (0 = all).
    FetchHistory { channel: Uuid, since_seq: u64 },
    /// Send a direct (1:1) message to another member.
    SendDm { to: Uuid, text: String },
    /// Fetch direct-message history with another member (offline catch-up).
    FetchDmHistory { with: Uuid, since_seq: u64 },
    /// Create a new public channel.
    CreateChannel { name: String },
    /// Upload a file inline (at most [`MAX_INLINE_FILE_BYTES`]) to a channel.
    PostFile {
        channel: Uuid,
        name: String,
        mime: String,
        data: Vec<u8>,
    },
    /// Upload a file inline and send it as a direct message to another member.
    SendFileDm {
        to: Uuid,
        name: String,
        mime: String,
        data: Vec<u8>,
    },
    /// Fetch a stored file's bytes by its content hash.
    DownloadFile { hash: String },

    // --- Governance and file management (appended; keep the order stable) ------
    /// Create a channel of a specific kind. `members` seeds a
    /// [`ChannelKind::Private`] channel's membership and is ignored otherwise.
    CreateChannelOfKind {
        name: String,
        kind: ChannelKind,
        members: Vec<Uuid>,
    },
    /// Delete a channel and its history (admins only).
    DeleteChannel { channel: Uuid },
    /// Change a channel's kind and/or private membership (admins only).
    SetChannelAccess {
        channel: Uuid,
        kind: ChannelKind,
        members: Vec<Uuid>,
    },
    /// Change another member's role (admins only; never above your own).
    SetRole { member: Uuid, role: Role },
    /// List every file the caller is allowed to see, for the Drive panel.
    ListFiles,
    /// Remove a file's reference, and the blob once nothing holds it
    /// (uploader or admin).
    DeleteFile { hash: String, channel: Uuid },
    /// Fetch the server's audit log (admins only), newest first.
    FetchAuditLog { limit: u32 },
    /// Storage the caller has used and is allowed to use.
    FetchQuota,

    // --- Chunked file transfer (appended; keep the order stable) --------------
    /// Begin a chunked upload. `size` is declared up front so the server can
    /// refuse before any bytes move.
    StartUpload {
        name: String,
        mime: String,
        size: u64,
        target: UploadTarget,
    },
    /// One chunk of an in-progress upload, in order. At most
    /// [`PARTY_CHUNK_BYTES`].
    UploadChunk { upload: Uuid, data: Vec<u8> },
    /// Verify the assembled length, store the blob and post the message.
    FinishUpload { upload: Uuid },
    /// Abandon an upload and discard its spool.
    CancelUpload { upload: Uuid },
    /// Fetch one chunk of a stored file from `offset`; the reply carries the
    /// total size.
    DownloadChunk { hash: String, offset: u64 },

    // --- Per-file permissions (appended; keep the order stable) --------------
    /// Post a file you already hold elsewhere without re-uploading. Requires
    /// [`FilePermissions::share`] on the source reference.
    ShareFile {
        hash: String,
        /// The reference being re-shared, where the caller's rights come from.
        from: Uuid,
        target: UploadTarget,
    },
    /// Change what a shared file grants: `member: None` sets the default for
    /// the location, `Some(id)` one member's override. Only for rights the
    /// caller holds.
    SetFilePermissions {
        hash: String,
        location: Uuid,
        member: Option<Uuid>,
        perms: FilePermissions,
    },
    /// The file listing from `offset` on (newest first). Sent only after a
    /// `Files` reply held exactly [`MAX_FILE_LISTING`] entries, so an older
    /// server is never sent a request it cannot decode (audit SRV-01).
    ListFilesPage { offset: u32 },
}

/// What a member may do with one shared file. Admin is not a flag here:
/// [`Role::is_admin`] overrides all of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct FilePermissions {
    /// See it in the listing and in a message.
    pub view: bool,
    /// Fetch its bytes.
    pub download: bool,
    /// Remove this share (not other shares of the same content).
    pub delete: bool,
    /// Post it elsewhere without re-uploading (free in the content-addressed
    /// store, hence its own right).
    pub share: bool,
}

impl Default for FilePermissions {
    /// A share's audience may view and download; delete and share must be
    /// granted.
    fn default() -> Self {
        Self {
            view: true,
            download: true,
            delete: false,
            share: false,
        }
    }
}

impl FilePermissions {
    /// Everything — what the uploader and any admin hold.
    pub fn all() -> Self {
        Self {
            view: true,
            download: true,
            delete: true,
            share: true,
        }
    }

    pub fn none() -> Self {
        Self {
            view: false,
            download: false,
            delete: false,
            share: false,
        }
    }

    /// Whether `self` covers every right in `other` (you can only delegate
    /// rights you hold).
    pub fn covers(self, other: Self) -> bool {
        (!other.view || self.view)
            && (!other.download || self.download)
            && (!other.delete || self.delete)
            && (!other.share || self.share)
    }

    /// Viewing is implied by any right, downloading by sharing.
    pub fn normalized(self) -> Self {
        let view = self.view || self.download || self.delete || self.share;
        Self {
            view,
            download: self.download || self.share,
            delete: self.delete,
            share: self.share,
        }
    }
}

/// One shared file in the Drive panel, with who shared it, where and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub hash: String,
    pub name: String,
    pub size: u64,
    pub mime: String,
    /// Member who shared it here.
    pub uploader: Uuid,
    pub uploader_name: String,
    /// Channel this reference lives in, or the DM thread id.
    pub location: Uuid,
    /// Display name of the location (`#general`, or the other member's name).
    pub location_name: String,
    /// True when the location is a DM thread rather than a channel.
    pub is_dm: bool,
    /// Unix milliseconds when it was shared.
    pub shared_at: u64,
    /// Whether the *requesting* member may delete this (`perms.delete`).
    pub can_delete: bool,
    /// What the requesting member may do with it. Appended field: keep it last.
    pub perms: FilePermissions,
}

/// A member's storage usage against their allowance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaInfo {
    /// Bytes of distinct content this member has uploaded and still holds.
    pub used: u64,
    /// Per-member ceiling, or `None` when unlimited (admins).
    pub limit: Option<u64>,
    /// Bytes stored server-wide across all members.
    pub server_used: u64,
    /// Server-wide ceiling.
    pub server_limit: u64,
}

/// One recorded governance action, newest first in a [`PartyResponse::AuditLog`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Unix milliseconds.
    pub at: u64,
    /// Member who performed the action.
    pub actor: Uuid,
    pub actor_name: String,
    /// Machine-readable action, e.g. `role.set`, `channel.delete`, `file.delete`.
    pub action: String,
    /// Human-readable detail, already resolved to names by the server.
    pub detail: String,
}

/// Deterministic, order-independent id of the DM thread between two members,
/// used as a DM `Envelope.channel`.
pub fn dm_thread_id(a: Uuid, b: Uuid) -> Uuid {
    use sha2::{Digest, Sha256};
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    let mut hasher = Sha256::new();
    hasher.update(b"party-dm-thread");
    hasher.update(lo.as_bytes());
    hasher.update(hi.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

/// Server → client messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartyResponse {
    /// Join accepted; carries the assigned member id and server context.
    Joined {
        member_id: Uuid,
        server_name: String,
        tier: TrustTier,
    },
    /// Join rejected (bad password, duplicate username, ...).
    JoinRejected {
        reason: String,
    },
    Members(Vec<MemberInfo>),
    Channels(Vec<ChannelInfo>),
    /// Acknowledges a posted message with its assigned per-channel sequence.
    MessagePosted {
        channel: Uuid,
        seq: u64,
    },
    /// A message delivered to the client (history item or live broadcast).
    Message(Envelope),
    /// A batch of history items (response to `FetchHistory`).
    History(Vec<Envelope>),
    /// A stored file's bytes (response to `DownloadFile`).
    FileData {
        hash: String,
        data: Vec<u8>,
    },
    /// A non-fatal application error.
    Error(String),
    /// A *send* (post, DM, upload) was refused. Distinct from `Error` so the
    /// client can retract the optimistically shown row. Appended variant:
    /// bincode encodes the index, so keep the order.
    ActionFailed {
        channel: Uuid,
        reason: String,
    },

    // --- Governance and file management (appended; keep the order stable) ------
    /// Every file the caller may see (response to [`PartyRequest::ListFiles`]).
    Files(Vec<FileEntry>),
    /// The caller's storage usage (response to [`PartyRequest::FetchQuota`]).
    Quota(QuotaInfo),
    /// The server's audit log, newest first (admins only).
    AuditLog(Vec<AuditEntry>),
    /// A governance action succeeded, with a message worth showing the user.
    Ok(String),
    /// The channel list changed; re-request it. A nudge, not the list: lists
    /// are per member, and the hub sends one identical frame to everyone, so
    /// broadcasting the list would leak private channels.
    DirectoryChanged,
    /// A chunked upload was accepted; send [`PartyRequest::UploadChunk`]s of at
    /// most `chunk_size` bytes, then [`PartyRequest::FinishUpload`].
    UploadReady {
        upload: Uuid,
        chunk_size: u32,
    },
    /// One chunk of a requested file; `total` is the whole size.
    FileChunk {
        hash: String,
        offset: u64,
        total: u64,
        data: Vec<u8>,
    },
    /// One page of the file listing, only in reply to
    /// [`PartyRequest::ListFilesPage`] (older clients cannot decode it). `next`
    /// is the offset to continue from, `None` at the end.
    FilesPage {
        files: Vec<FileEntry>,
        offset: u32,
        next: Option<u32>,
    },
}

/// Most entries in one `Files`/`FilesPage`. A `Files` reply holding exactly
/// this many may be truncated: the cue to page on with `ListFilesPage`.
pub const MAX_FILE_LISTING: usize = 2_000;

/// Byte budget for one file listing page — half a frame.
pub const FILE_LISTING_BYTES: usize = 4 * 1024 * 1024;

/// Most envelopes in one `History` page; the client pages on with `since_seq`.
/// Pages are also capped by [`HISTORY_PAGE_BYTES`] (see [`take_history_page`]).
pub const MAX_HISTORY_BATCH: usize = 200;

/// Bytes a Party frame adds around its payload: sequence, nonce and tag.
pub const PARTY_FRAME_OVERHEAD: usize =
    PARTY_SEQ_BYTES + crate::AES_NONCE_SIZE + crate::AES_GCM_TAG_SIZE;

/// The largest serialized request or response that fits in one frame; a
/// larger one cannot be sent, and on the server that ends the connection.
pub const MAX_PARTY_PAYLOAD_BYTES: usize = crate::MAX_PACKET_SIZE - PARTY_FRAME_OVERHEAD;

/// Byte budget for one `History` page's envelopes (half a frame). A count cap
/// alone let ~130 max-size posts make every page unsendable, which ended every
/// joining connection.
pub const HISTORY_PAGE_BYTES: usize = 4 * 1024 * 1024;

/// Stands in for an envelope too large for any page (only an older server's
/// database can hold one), so history can still be paged past it.
pub const OVERSIZED_ENVELOPE_TEXT: &str = "[this message is too large to deliver]";

/// Encoded size of `value` on the wire (bincode), for byte budgets.
pub fn encoded_len<T: Serialize + ?Sized>(value: &T) -> usize {
    bincode::serialized_size(value)
        .ok()
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(usize::MAX)
}

/// One `History` page from `envelopes` (ascending, after `since_seq`): at most
/// [`MAX_HISTORY_BATCH`] and [`HISTORY_PAGE_BYTES`] encoded, but always at
/// least one so paging never stalls. An envelope over the budget alone becomes
/// an [`OVERSIZED_ENVELOPE_TEXT`] placeholder with the same seq.
pub fn take_history_page<'a, I>(envelopes: I) -> Vec<Envelope>
where
    I: IntoIterator<Item = &'a Envelope>,
{
    let mut page = Vec::new();
    let mut used = 0usize;
    for env in envelopes.into_iter().take(MAX_HISTORY_BATCH) {
        let mut size = encoded_len(env);
        let env = if size > HISTORY_PAGE_BYTES {
            let stub = Envelope {
                payload: MessagePayload::Text(OVERSIZED_ENVELOPE_TEXT.to_string()),
                ..env.clone()
            };
            size = encoded_len(&stub);
            stub
        } else {
            env.clone()
        };
        if !page.is_empty() && used.saturating_add(size) > HISTORY_PAGE_BYTES {
            break;
        }
        used = used.saturating_add(size);
        page.push(env);
    }
    page
}

/// Whether the client should ask again from its last seq. A page cut for size
/// holds more than half the budget (envelopes are tiny next to it); a false
/// positive costs one empty reply.
pub fn history_page_may_continue(page: &[Envelope]) -> bool {
    page.len() >= MAX_HISTORY_BATCH
        || page
            .iter()
            .map(encoded_len)
            .fold(0usize, usize::saturating_add)
            > HISTORY_PAGE_BYTES / 2
}

impl PartyRequest {
    pub fn to_bytes(&self) -> Vec<u8> {
        bincode::serialize(self).expect("PartyRequest serialization is infallible")
    }

    /// Parse; `None` on malformed input, including trailing bytes
    /// ([`crate::util::decode_exact`]).
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        crate::util::decode_exact(bytes).ok()
    }
}

impl PartyResponse {
    pub fn to_bytes(&self) -> Vec<u8> {
        bincode::serialize(self).expect("PartyResponse serialization is infallible")
    }

    /// Parse; `None` on malformed input, including trailing bytes.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        crate::util::decode_exact(bytes).ok()
    }
}

/// Width of the per-frame sequence number, carried *inside* the encryption.
const PARTY_SEQ_BYTES: usize = 8;

/// Monotonic per-direction frame counter, so an on-path attacker cannot replay
/// a captured frame (the P2P loop has enforced this since v3).
#[derive(Debug, Default, Clone)]
pub struct FrameSeq(u64);

impl FrameSeq {
    pub fn new() -> Self {
        Self::default()
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.saturating_add(1);
        self.0
    }

    /// Accept `seq` only if it advances the counter.
    fn accept(&mut self, seq: u64) -> bool {
        if seq > self.0 {
            self.0 = seq;
            true
        } else {
            false
        }
    }
}

/// Send a Party message over a v3 tunnel: prefix the next frame sequence,
/// encrypt with the session `cipher` (bound to `transport_aad`), length-prefix.
pub async fn send_framed<S>(
    stream: &mut S,
    cipher: &AesCipher,
    transport_aad: &[u8],
    seq: &mut FrameSeq,
    payload: &[u8],
) -> anyhow::Result<()>
where
    S: AsyncWrite + Unpin,
{
    let mut framed = Vec::with_capacity(PARTY_SEQ_BYTES + payload.len());
    framed.extend_from_slice(&seq.next().to_be_bytes());
    framed.extend_from_slice(payload);
    let ciphertext = cipher.encrypt(&framed, Some(transport_aad));
    send_packet(stream, &ciphertext).await?;
    Ok(())
}

/// Receive and decrypt the next Party message, refusing a frame that fails to
/// authenticate or whose sequence does not advance (replay/reorder).
pub async fn recv_framed<S>(
    stream: &mut S,
    cipher: &AesCipher,
    transport_aad: &[u8],
    seq: &mut FrameSeq,
) -> anyhow::Result<Vec<u8>>
where
    S: AsyncRead + Unpin,
{
    let ciphertext = recv_packet(stream).await?;
    let plaintext = cipher
        // Only the other side's frames: a reflected one of ours is refused.
        .decrypt_from_peer(&ciphertext, Some(transport_aad))
        .ok_or_else(|| anyhow::anyhow!("failed to decrypt Party frame"))?;
    if plaintext.len() < PARTY_SEQ_BYTES {
        anyhow::bail!("Party frame is too short to carry a sequence number");
    }
    let (head, body) = plaintext.split_at(PARTY_SEQ_BYTES);
    let frame_seq = u64::from_be_bytes(head.try_into().expect("checked length"));
    if !seq.accept(frame_seq) {
        anyhow::bail!("replayed or out-of-order Party frame (seq {frame_seq})");
    }
    Ok(body.to_vec())
}

async fn recv_response<S: AsyncRead + Unpin>(
    stream: &mut S,
    cipher: &AesCipher,
    transport_aad: &[u8],
    seq: &mut FrameSeq,
) -> anyhow::Result<PartyResponse> {
    let bytes = recv_framed(stream, cipher, transport_aad, seq).await?;
    PartyResponse::from_bytes(&bytes).ok_or_else(|| anyhow::anyhow!("malformed server response"))
}

/// Client-side handle to a Party server over an established v3 tunnel. Replies
/// and pushed broadcasts arrive interleaved as one [`PartyResponse`] stream.
pub struct PartyClient<S> {
    stream: S,
    server_fingerprint: String,
    sas: String,
    cipher: AesCipher,
    transport_aad: Vec<u8>,
    send_seq: FrameSeq,
    recv_seq: FrameSeq,
}

/// Read half of a split [`PartyClient`].
pub struct PartyReader<R> {
    rd: R,
    cipher: AesCipher,
    transport_aad: Vec<u8>,
    recv_seq: FrameSeq,
}

/// Write half of a split [`PartyClient`].
pub struct PartyWriter<W> {
    wr: W,
    cipher: AesCipher,
    transport_aad: Vec<u8>,
    send_seq: FrameSeq,
}

impl<R> PartyReader<R>
where
    R: AsyncRead + Unpin + Send,
{
    /// Receive the next message from the server (reply or pushed broadcast).
    pub async fn recv(&mut self) -> anyhow::Result<PartyResponse> {
        recv_response(
            &mut self.rd,
            &self.cipher,
            &self.transport_aad,
            &mut self.recv_seq,
        )
        .await
    }
}

impl<W> PartyWriter<W>
where
    W: AsyncWrite + Unpin + Send,
{
    pub async fn send(&mut self, req: &PartyRequest) -> anyhow::Result<()> {
        send_framed(
            &mut self.wr,
            &self.cipher,
            &self.transport_aad,
            &mut self.send_seq,
            &req.to_bytes(),
        )
        .await
    }
}

impl<S> PartyClient<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    /// Complete the client side of the v3 handshake; `chat_id` is this
    /// client's advertised session id.
    pub async fn connect(
        mut stream: S,
        privkey: &RsaPrivateKey,
        chat_id: Uuid,
    ) -> anyhow::Result<Self> {
        let tunnel = client_handshake(&mut stream, privkey, chat_id).await?;
        let sas = crate::core::crypto::derive_sas(&tunnel.transport_aad);
        Ok(Self {
            stream,
            server_fingerprint: tunnel.peer_fingerprint,
            sas,
            cipher: tunnel.cipher,
            transport_aad: tunnel.transport_aad,
            send_seq: FrameSeq::new(),
            recv_seq: FrameSeq::new(),
        })
    }

    /// The server's handshake-verified identity fingerprint, for TOFU pinning.
    pub fn server_fingerprint(&self) -> &str {
        &self.server_fingerprint
    }

    /// The tunnel's short authentication string, derived from the transcript
    /// as for a P2P session.
    pub fn sas(&self) -> &str {
        &self.sas
    }

    /// Split into read and write halves for a concurrent receive loop.
    pub fn split(self) -> (PartyReader<ReadHalf<S>>, PartyWriter<WriteHalf<S>>) {
        let (rd, wr) = tokio::io::split(self.stream);
        (
            PartyReader {
                rd,
                cipher: self.cipher.clone(),
                transport_aad: self.transport_aad.clone(),
                recv_seq: self.recv_seq,
            },
            PartyWriter {
                wr,
                cipher: self.cipher,
                transport_aad: self.transport_aad,
                send_seq: self.send_seq,
            },
        )
    }

    /// Send a request to the server.
    pub async fn send(&mut self, req: &PartyRequest) -> anyhow::Result<()> {
        send_framed(
            &mut self.stream,
            &self.cipher,
            &self.transport_aad,
            &mut self.send_seq,
            &req.to_bytes(),
        )
        .await
    }

    /// Receive the next message from the server (a reply or a pushed broadcast).
    pub async fn recv(&mut self) -> anyhow::Result<PartyResponse> {
        recv_response(
            &mut self.stream,
            &self.cipher,
            &self.transport_aad,
            &mut self.recv_seq,
        )
        .await
    }

    /// Join with a username and optional password. The reply is the next
    /// message: broadcasts reach a connection only once it has joined.
    pub async fn join(
        &mut self,
        username: &str,
        password: Option<String>,
    ) -> anyhow::Result<PartyResponse> {
        self.send(&PartyRequest::Join {
            username: username.to_string(),
            password,
        })
        .await?;
        self.recv().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::NonceRole;

    /// A Host/Client cipher pair over one random session key.
    fn cipher_pair() -> (AesCipher, AesCipher) {
        let key: [u8; 32] = rand::random();
        (
            AesCipher::new_with_role(&key, NonceRole::Host).unwrap(),
            AesCipher::new_with_role(&key, NonceRole::Client).unwrap(),
        )
    }

    /// The exact bytes of one `ListMembers` frame sent with `cipher`.
    async fn capture_frame(cipher: &AesCipher, aad: &[u8]) -> Vec<u8> {
        let mut captured = Vec::new();
        send_framed(
            &mut captured,
            cipher,
            aad,
            &mut FrameSeq::new(),
            &PartyRequest::ListMembers.to_bytes(),
        )
        .await
        .unwrap();
        captured
    }

    fn sample_envelope() -> Envelope {
        text_envelope(7, "hello channel".to_string())
    }

    #[test]
    fn request_roundtrips() {
        let requests = vec![
            PartyRequest::Join {
                username: "alice".to_string(),
                password: Some("hunter2".to_string()),
            },
            PartyRequest::Join {
                username: "bob".to_string(),
                password: None,
            },
            PartyRequest::ListMembers,
            PartyRequest::ListFilesPage { offset: 2_000 },
            PartyRequest::ListChannels,
            PartyRequest::PostMessage {
                channel: Uuid::new_v4(),
                text: "hi all 👋".to_string(),
            },
            PartyRequest::FetchHistory {
                channel: Uuid::new_v4(),
                since_seq: 42,
            },
            PartyRequest::SendDm {
                to: Uuid::new_v4(),
                text: "psst".to_string(),
            },
            PartyRequest::FetchDmHistory {
                with: Uuid::new_v4(),
                since_seq: 3,
            },
            PartyRequest::CreateChannel {
                name: "random".to_string(),
            },
            PartyRequest::PostFile {
                channel: Uuid::new_v4(),
                name: "report.pdf".to_string(),
                mime: "application/pdf".to_string(),
                data: vec![1, 2, 3, 4],
            },
            PartyRequest::SendFileDm {
                to: Uuid::new_v4(),
                name: "note.txt".to_string(),
                mime: "text/plain".to_string(),
                data: b"hi".to_vec(),
            },
            PartyRequest::DownloadFile {
                hash: "abc123".to_string(),
            },
        ];
        for req in requests {
            let bytes = req.to_bytes();
            assert_eq!(PartyRequest::from_bytes(&bytes), Some(req));
        }
    }

    #[test]
    fn blob_hash_is_deterministic_and_content_addressed() {
        assert_eq!(blob_hash(b"hello"), blob_hash(b"hello"));
        assert_ne!(blob_hash(b"hello"), blob_hash(b"world"));
        // Known SHA-256 of "hello" (lowercase hex).
        assert_eq!(
            blob_hash(b"hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    /// The incremental and one-shot hashes must agree for every chunking, or a
    /// file's address (and dedup) would depend on how it was uploaded.
    #[test]
    fn incremental_and_one_shot_hashing_agree() {
        let payload: Vec<u8> = (0..(64 * 1024 + 12345)).map(|i| (i % 251) as u8).collect();
        let expected = blob_hash(&payload);

        for chunk_size in [1usize, 7, 1024, 64 * 1024, payload.len(), payload.len() * 2] {
            let mut hasher = BlobHasher::new();
            for chunk in payload.chunks(chunk_size.max(1)) {
                hasher.update(chunk);
            }
            assert_eq!(
                hasher.finish(),
                expected,
                "chunking at {chunk_size} changed the content address"
            );
        }

        // Empty input and the known answer hold incrementally too.
        assert_eq!(BlobHasher::new().finish(), blob_hash(b""));
        let mut hasher = BlobHasher::new();
        hasher.update(b"hel");
        hasher.update(b"lo");
        assert_eq!(
            hasher.finish(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn file_payload_envelope_roundtrips() {
        let env = Envelope {
            payload: MessagePayload::File(FileMeta {
                hash: blob_hash(b"data"),
                name: "f.bin".to_string(),
                size: 4,
                mime: "application/octet-stream".to_string(),
            }),
            ..text_envelope(7, String::new())
        };
        let bytes = bincode::serialize(&env).unwrap();
        assert_eq!(bincode::deserialize::<Envelope>(&bytes).unwrap(), env);
    }

    #[test]
    fn dm_thread_id_is_order_independent_and_distinct() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        assert_eq!(
            dm_thread_id(a, b),
            dm_thread_id(b, a),
            "thread id must be symmetric"
        );
        assert_ne!(
            dm_thread_id(a, b),
            dm_thread_id(a, c),
            "different pairs differ"
        );
    }

    #[test]
    fn response_roundtrips() {
        let responses = vec![
            PartyResponse::Joined {
                member_id: Uuid::new_v4(),
                server_name: "Study Server".to_string(),
                tier: TrustTier::Administered,
            },
            PartyResponse::JoinRejected {
                reason: "wrong password".to_string(),
            },
            PartyResponse::FilesPage {
                files: Vec::new(),
                offset: 2_000,
                next: None,
            },
            PartyResponse::Members(vec![MemberInfo {
                id: Uuid::new_v4(),
                username: "alice".to_string(),
                online: true,
                role: Role::Member,
            }]),
            PartyResponse::Channels(vec![ChannelInfo {
                id: Uuid::new_v4(),
                name: "general".to_string(),
                kind: ChannelKind::Public,
                members: Vec::new(),
            }]),
            PartyResponse::MessagePosted {
                channel: Uuid::new_v4(),
                seq: 1,
            },
            PartyResponse::Message(sample_envelope()),
            PartyResponse::History(vec![sample_envelope(), sample_envelope()]),
            PartyResponse::FileData {
                hash: blob_hash(b"bytes"),
                data: b"bytes".to_vec(),
            },
            PartyResponse::Error("boom".to_string()),
            PartyResponse::ActionFailed {
                channel: Uuid::new_v4(),
                reason: "message is too long".to_string(),
            },
        ];
        for resp in responses {
            let bytes = resp.to_bytes();
            assert_eq!(PartyResponse::from_bytes(&bytes), Some(resp));
        }
    }

    #[test]
    fn malformed_bytes_return_none() {
        assert!(PartyRequest::from_bytes(&[0xff, 0xff, 0xff, 0xff]).is_none());
        assert!(PartyResponse::from_bytes(&[]).is_none());
    }

    #[test]
    fn tier_and_channel_kind_default() {
        assert_eq!(TrustTier::default(), TrustTier::Administered);
        assert_eq!(ChannelKind::default(), ChannelKind::Public);
    }

    #[tokio::test]
    async fn framed_message_roundtrips_over_a_tunnel() {
        let (cipher, peer) = cipher_pair();
        let aad = b"transport|party-test";
        let (mut a, mut b) = tokio::io::duplex(4096);
        let mut tx_seq = FrameSeq::new();
        let mut rx_seq = FrameSeq::new();

        let req = PartyRequest::PostMessage {
            channel: Uuid::new_v4(),
            text: "hi".to_string(),
        };
        send_framed(&mut a, &cipher, aad, &mut tx_seq, &req.to_bytes())
            .await
            .unwrap();
        let bytes = recv_framed(&mut b, &peer, aad, &mut rx_seq).await.unwrap();
        assert_eq!(PartyRequest::from_bytes(&bytes), Some(req));

        // The wrong AAD must fail to authenticate.
        let list = PartyRequest::ListMembers.to_bytes();
        send_framed(&mut a, &cipher, aad, &mut tx_seq, &list)
            .await
            .unwrap();
        assert!(recv_framed(&mut b, &peer, b"wrong-aad", &mut rx_seq)
            .await
            .is_err());
    }

    /// A captured frame must not count twice.
    #[tokio::test]
    async fn a_replayed_party_frame_is_rejected() {
        let (cipher, peer) = cipher_pair();
        let aad = b"transport|replay-test";
        let captured = capture_frame(&cipher, aad).await;

        let mut rx_seq = FrameSeq::new();
        let mut once = captured.as_slice();
        recv_framed(&mut once, &peer, aad, &mut rx_seq)
            .await
            .expect("the first delivery is accepted");

        // Same bytes again: authenticates, but the sequence has not moved.
        let mut again = captured.as_slice();
        let err = recv_framed(&mut again, &peer, aad, &mut rx_seq)
            .await
            .expect_err("a replayed frame must be refused");
        assert!(err.to_string().contains("replayed or out-of-order"));
    }

    /// NET-01: both directions share a key, so an echoed frame of ours used to
    /// authenticate as the other side's.
    #[tokio::test]
    async fn a_reflected_party_frame_is_rejected() {
        let (ours, _) = cipher_pair();
        let aad = b"transport|reflect-test";
        let captured = capture_frame(&ours, aad).await;
        let mut echoed = captured.as_slice();
        assert!(recv_framed(&mut echoed, &ours, aad, &mut FrameSeq::new())
            .await
            .is_err());
    }

    #[test]
    fn frame_sequence_only_moves_forward() {
        let mut seq = FrameSeq::new();
        assert!(seq.accept(1));
        assert!(seq.accept(2));
        assert!(!seq.accept(2), "a repeat must be refused");
        assert!(!seq.accept(1), "going backwards must be refused");
        assert!(seq.accept(9), "a gap is allowed; only regression is not");
    }

    #[tokio::test]
    async fn party_client_connect_split_send_and_recv() {
        use crate::core::generate_rsa_keypair;
        use crate::network::host_handshake;
        use crate::RSA_KEY_BITS;

        let server_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let client_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let (mut server_stream, client_stream) = tokio::io::duplex(1 << 16);

        let server = tokio::spawn(async move {
            let tunnel = host_handshake(&mut server_stream, &server_priv, Uuid::new_v4())
                .await
                .unwrap();
            let mut send_seq = FrameSeq::new();
            let mut recv_seq = FrameSeq::new();
            let bytes = recv_framed(
                &mut server_stream,
                &tunnel.cipher,
                &tunnel.transport_aad,
                &mut recv_seq,
            )
            .await
            .unwrap();
            assert_eq!(
                PartyRequest::from_bytes(&bytes),
                Some(PartyRequest::ListMembers)
            );
            send_framed(
                &mut server_stream,
                &tunnel.cipher,
                &tunnel.transport_aad,
                &mut send_seq,
                &PartyResponse::Members(Vec::new()).to_bytes(),
            )
            .await
            .unwrap();
        });

        let client = PartyClient::connect(client_stream, &client_priv, Uuid::new_v4())
            .await
            .unwrap();
        assert!(!client.server_fingerprint().is_empty());
        // Shown on the join screen before any credential is sent.
        assert!(!client.sas().is_empty());

        let (mut reader, mut writer) = client.split();
        writer.send(&PartyRequest::ListMembers).await.unwrap();
        assert!(matches!(
            reader.recv().await.unwrap(),
            PartyResponse::Members(ref m) if m.is_empty()
        ));
        server.await.unwrap();
    }

    fn text_envelope(seq: u64, text: String) -> Envelope {
        Envelope {
            tier: TrustTier::Administered,
            sender: Uuid::nil(),
            channel: Uuid::nil(),
            seq,
            timestamp: 0,
            payload: MessagePayload::Text(text),
        }
    }

    /// 200 × 64 KiB of text is 12.8 MiB: a page must stop at the byte budget,
    /// make progress, and fit in a frame.
    #[test]
    fn a_history_page_is_bounded_by_bytes_as_well_as_count() {
        let all: Vec<Envelope> = (1..=MAX_HISTORY_BATCH as u64)
            .map(|seq| text_envelope(seq, "x".repeat(crate::MAX_TEXT_MESSAGE_BYTES)))
            .collect();
        let page = take_history_page(all.iter());
        assert!(!page.is_empty());
        assert!(page.len() < MAX_HISTORY_BATCH, "trimmed for size");
        let frame = PartyResponse::History(page.clone()).to_bytes();
        assert!(frame.len() <= MAX_PARTY_PAYLOAD_BYTES);
        assert!(
            history_page_may_continue(&page),
            "a trimmed page tells the client to ask again"
        );
        // Contiguous from the start, so paging resumes exactly after it.
        for (i, env) in page.iter().enumerate() {
            assert_eq!(env.seq, i as u64 + 1);
        }

        // A small final page does not ask for more.
        let small = take_history_page([text_envelope(1, "hi".to_string())].iter());
        assert_eq!(small.len(), 1);
        assert!(!history_page_may_continue(&small));
    }

    /// An envelope too large for any page becomes a placeholder at its seq.
    #[test]
    fn an_oversized_envelope_becomes_a_placeholder_rather_than_a_stuck_page() {
        let huge = text_envelope(7, "x".repeat(HISTORY_PAGE_BYTES + 1));
        let next = text_envelope(8, "after".to_string());
        let page = take_history_page([huge, next].iter());
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].seq, 7);
        assert_eq!(
            page[0].payload,
            MessagePayload::Text(OVERSIZED_ENVELOPE_TEXT.to_string())
        );
        assert_eq!(page[1].seq, 8);
    }
}
