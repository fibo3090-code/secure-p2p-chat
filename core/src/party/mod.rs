//! Party application protocol (Phase 1).
//!
//! Shared wire contract between the client and the Party server. It rides *on top
//! of* the established Protocol v3 encrypted tunnel (see `network::session`): the
//! handshake authenticates and encrypts the channel to the server, and these
//! messages carry the Party-level application semantics (join, directory, channel
//! messaging, offline catch-up).
//!
//! Two trust tiers share one data model (see `docs/platform_spec.md`):
//! - **Administered** (default): the server stores plaintext payloads.
//! - **E2EE** (Phase 4): payloads are ciphertext; the server never sees plaintext.
//!
//! The MVP implements the Administered tier.

use rsa::RsaPrivateKey;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite, ReadHalf, WriteHalf};
use uuid::Uuid;

use crate::core::{recv_packet, send_packet, AesCipher};
use crate::network::client_handshake;

/// Trust tier of a server / stored message. A property of the server, surfaced
/// prominently in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TrustTier {
    /// Server stores plaintext; enables offline buffering, search, admin-read.
    #[default]
    Administered,
    /// Server stores ciphertext only; admin cannot read (Phase 4).
    E2EE,
}

/// Maximum size of a file uploaded inline in a single Party request. Anything
/// larger goes through the chunked path ([`PartyRequest::StartUpload`]). Kept
/// well under [`crate::MAX_PACKET_SIZE`] to leave headroom for request framing.
pub const MAX_INLINE_FILE_BYTES: usize = 4 * 1024 * 1024; // 4 MiB

/// Bytes of file data carried by one [`PartyRequest::UploadChunk`] or
/// [`PartyResponse::FileChunk`].
///
/// Small enough that a chunk plus its framing is nowhere near
/// [`crate::MAX_PACKET_SIZE`], and small enough that a cancelled transfer wastes
/// little; large enough that a 100 MiB file is a few hundred round trips rather
/// than tens of thousands.
pub const PARTY_CHUNK_BYTES: usize = 256 * 1024; // 256 KiB

/// Largest file a community server will accept at all, inline or chunked.
///
/// Distinct from the per-member quota: the quota bounds what one member may
/// *hold*, this bounds what a single transfer may cost the server in spool
/// space and time before it is either committed or thrown away.
pub const MAX_PARTY_FILE_BYTES: u64 = 100 * 1024 * 1024; // 100 MiB

/// Where a chunked upload will be posted once it completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UploadTarget {
    Channel(Uuid),
    /// A direct message to this member.
    Dm(Uuid),
}

/// Metadata describing a file shared in a channel or DM. The file's bytes are
/// stored content-addressed by `hash` (lowercase hex SHA-256); `name` is the
/// per-message display name, so two messages may reference one blob under
/// different names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMeta {
    pub hash: String,
    pub name: String,
    pub size: u64,
    pub mime: String,
}

/// The application payload carried by an [`Envelope`]. For the Administered tier
/// this is plaintext; the E2EE tier (Phase 4) will add a ciphertext variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessagePayload {
    Text(String),
    /// A reference to a file stored on the server (Phase 2). The bytes are fetched
    /// separately via [`PartyRequest::DownloadFile`] using `FileMeta::hash`.
    File(FileMeta),
}

/// Content address (lowercase hex SHA-256) of a blob's bytes. Both sides compute
/// this identically so uploads can be deduplicated by content.
pub fn blob_hash(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
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

/// A member's authority on a community server.
///
/// Ordered least- to most-privileged, so `>=` is the permission test: `role >=
/// Role::Admin` reads as "at least an admin". Every check in the server is
/// written that way.
///
/// The server has exactly one [`Role::Owner`]: the first identity to join, which
/// is the operator (they start the server, then join it). The owner cannot be
/// demoted — otherwise an admin could strip the operator of their own server —
/// and a second owner is never created.
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

    /// Whether this role may post messages and upload files at all. A guest is
    /// read-only everywhere, before any per-channel rule is consulted.
    pub fn can_write(self) -> bool {
        self >= Role::Member
    }

    /// Whether this role may create channels.
    pub fn can_create_channel(self) -> bool {
        self >= Role::Member
    }

    /// Whether this role administers the server: channel kinds and membership,
    /// deleting channels, deleting anyone's files, posting to restricted
    /// channels, and changing roles.
    pub fn is_admin(self) -> bool {
        self >= Role::Admin
    }

    /// Whether `self` may assign `target` role to somebody else.
    ///
    /// You can only grant a role strictly below your own, so an admin cannot
    /// mint another admin (or an owner) and lock the operator out of their own
    /// community. The owner is the only one who can appoint admins.
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

/// Channel kind — the per-channel access rule.
///
/// These used to be stored, persisted, and shipped to clients while being
/// enforced nowhere, then (when that was found) made to fail closed, which left
/// three of the four kinds unusable. They are now real: see
/// [`ChannelKind::may_read`] / [`ChannelKind::may_post`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, Hash)]
pub enum ChannelKind {
    /// Everyone who has joined the server may read and post.
    #[default]
    Public,
    /// Only members on the channel's own membership list may read or post.
    Private,
    /// Everyone may read; nobody may post except an admin. Used to freeze a
    /// channel without deleting its history.
    Locked,
    /// Everyone may read; only admins may post. Used for announcements.
    Announce,
}

impl ChannelKind {
    /// Whether a member with `role`, who is `is_channel_member` of this channel,
    /// may read it. `is_channel_member` is only consulted for [`Self::Private`];
    /// every other kind is server-wide.
    pub fn may_read(self, role: Role, is_channel_member: bool) -> bool {
        match self {
            ChannelKind::Public | ChannelKind::Locked | ChannelKind::Announce => true,
            // An admin can always see a private channel — they administer it,
            // and pretending otherwise only means they cannot moderate it.
            ChannelKind::Private => is_channel_member || role.is_admin(),
        }
    }

    /// Whether a member with `role` may post here. A guest never may.
    pub fn may_post(self, role: Role, is_channel_member: bool) -> Result<(), &'static str> {
        if !role.can_write() {
            return Err("your role on this server is read-only");
        }
        match self {
            ChannelKind::Public => Ok(()),
            ChannelKind::Private => {
                if is_channel_member || role.is_admin() {
                    Ok(())
                } else {
                    Err("you are not a member of this channel")
                }
            }
            ChannelKind::Locked => {
                if role.is_admin() {
                    Ok(())
                } else {
                    Err("this channel is locked")
                }
            }
            ChannelKind::Announce => {
                if role.is_admin() {
                    Ok(())
                } else {
                    Err("only admins may post to an announcement channel")
                }
            }
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
    /// Members of a [`ChannelKind::Private`] channel. Empty for every other kind,
    /// where membership is server-wide. Appended field: keep it last.
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
    /// Fetch channel history strictly after `since_seq` (offline catch-up).
    /// `since_seq = 0` returns the whole channel.
    FetchHistory { channel: Uuid, since_seq: u64 },
    /// Send a direct (1:1) message to another member.
    SendDm { to: Uuid, text: String },
    /// Fetch direct-message history with another member (offline catch-up).
    FetchDmHistory { with: Uuid, since_seq: u64 },
    /// Create a new public channel.
    CreateChannel { name: String },
    /// Upload a file inline and post it as a message to a channel. The bytes must
    /// not exceed [`MAX_INLINE_FILE_BYTES`].
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
    /// Delete a shared file: removes its reference, and the blob once nothing
    /// holds it. Allowed for the uploader or an admin.
    DeleteFile { hash: String, channel: Uuid },
    /// Fetch the server's audit log (admins only), newest first.
    FetchAuditLog { limit: u32 },
    /// Storage the caller has used and is allowed to use.
    FetchQuota,

    // --- Chunked file transfer (appended; keep the order stable) --------------
    /// Begin a chunked upload. `size` is declared up front so the server can
    /// refuse — quota, ceiling, unknown channel — before any bytes move rather
    /// than after spooling a hundred megabytes.
    StartUpload {
        name: String,
        mime: String,
        size: u64,
        target: UploadTarget,
    },
    /// One chunk of an in-progress upload, in order. At most
    /// [`PARTY_CHUNK_BYTES`].
    UploadChunk { upload: Uuid, data: Vec<u8> },
    /// All chunks sent: verify the assembled length, store the blob, and post
    /// the file message.
    FinishUpload { upload: Uuid },
    /// Abandon an upload and discard its spool.
    CancelUpload { upload: Uuid },
    /// Fetch one chunk of a stored file, starting at `offset`. The reply carries
    /// the total size, so the caller knows when it is done.
    DownloadChunk { hash: String, offset: u64 },

    // --- Per-file permissions (appended; keep the order stable) --------------
    /// Post a file you already hold somewhere else, without re-uploading it.
    /// Requires [`FilePermissions::share`] on the source reference — and the
    /// content is stored once, so this costs a reference, not a copy.
    ShareFile {
        hash: String,
        /// The reference being re-shared, which is where the caller's rights
        /// come from. One blob can sit in several places under different
        /// grants, so the source has to be named.
        from: Uuid,
        target: UploadTarget,
    },
    /// Change what a shared file grants. `member: None` sets the default for
    /// everyone who can reach the location; `Some(id)` sets one member's grant,
    /// overriding the default for them.
    ///
    /// Only ever succeeds for rights the caller holds themselves.
    SetFilePermissions {
        hash: String,
        location: Uuid,
        member: Option<Uuid>,
        perms: FilePermissions,
    },
}

/// What a member may do with one shared file.
///
/// The rights are deliberately separate: seeing that a file exists is not the
/// same as being able to fetch its bytes, and neither implies being allowed to
/// put it somewhere else. `admin` from the design note is not a flag here —
/// it is [`Role::is_admin`], which overrides all of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct FilePermissions {
    /// See it in the listing and in a message.
    pub view: bool,
    /// Fetch its bytes.
    pub download: bool,
    /// Remove this share (not other shares of the same content).
    pub delete: bool,
    /// Post it somewhere else without re-uploading — the content-addressed
    /// store makes that free, which is exactly why it needs its own right.
    pub share: bool,
}

impl Default for FilePermissions {
    /// What the audience of a channel or DM gets when a file is shared there:
    /// they can see it and fetch it, and that is all. Deleting somebody else's
    /// share and re-posting it elsewhere are both grants somebody has to make.
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

    /// Nothing.
    pub fn none() -> Self {
        Self {
            view: false,
            download: false,
            delete: false,
            share: false,
        }
    }

    /// Whether `self` covers every right in `other`.
    ///
    /// This is the "you can only delegate rights you hold" rule: a grant is
    /// refused unless the granter's own effective rights are a superset of it.
    pub fn covers(self, other: Self) -> bool {
        (!other.view || self.view)
            && (!other.download || self.download)
            && (!other.delete || self.delete)
            && (!other.share || self.share)
    }

    /// Downloading something you cannot see, or sharing something you cannot
    /// download, is not a coherent grant. Normalising here means the rule is
    /// enforced once rather than at every call site.
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

/// One shared file as shown in the Drive panel: the blob plus the provenance the
/// server records for it — who shared it, where, and when.
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
    /// Whether the *requesting* member may delete this reference. Kept as a
    /// convenience for the UI; it is `perms.delete`.
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

/// Deterministic, order-independent id for the 1:1 DM thread between two members,
/// so both participants and the server agree on the same `Envelope.channel` for
/// their direct messages. A DM `Envelope` is just a [`Message`]/[`History`] item
/// whose `channel` is this thread id rather than a real channel id.
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
    /// A *send* was refused (post, DM, or file upload). Distinct from `Error`
    /// because the client appends outgoing messages optimistically — it has to
    /// know that the refusal belongs to the message it is still showing, so it
    /// can take it back rather than leave the user believing it was delivered.
    /// Appended variant: bincode encodes the index, so this must stay last.
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
    /// The channel list changed; re-request it with
    /// [`PartyRequest::ListChannels`].
    ///
    /// A nudge rather than the list itself, because the list is now *per member*:
    /// a private channel is only visible to those in it, and the hub fans one
    /// identical frame out to every connection. Broadcasting the channels
    /// directly would hand everyone the private ones.
    DirectoryChanged,
    /// A chunked upload was accepted; send [`PartyRequest::UploadChunk`]s of at
    /// most `chunk_size` bytes, then [`PartyRequest::FinishUpload`].
    UploadReady {
        upload: Uuid,
        chunk_size: u32,
    },
    /// One chunk of a requested file. `total` is the whole file's size, so the
    /// caller knows whether to ask for more; a short final chunk is normal.
    FileChunk {
        hash: String,
        offset: u64,
        total: u64,
        data: Vec<u8>,
    },
}

/// Most envelopes the server will put in one `History` response.
///
/// A whole channel used to be returned in a single frame, so once its history
/// passed [`crate::MAX_PACKET_SIZE`] the reply could not be sent at all: the
/// send failed, the connection dropped, and the community became unjoinable
/// with no error the user could see. History is paged instead — the client asks
/// again with `since_seq` set to the last envelope it received.
pub const MAX_HISTORY_BATCH: usize = 200;

/// The bincode configuration both Party codecs use.
///
/// Byte-for-byte what `bincode::serialize`/`deserialize` produce — fixed-width
/// integers, little-endian — with one addition: **trailing bytes are rejected**.
///
/// bincode 1.x stops at the end of the value and silently ignores whatever
/// follows, so `frame` and `frame || anything` decoded to the same request. Two
/// distinct byte strings with one meaning is a malleable frame: anyone who can
/// modify bytes in flight can pad a frame without changing what it does, and any
/// reasoning that treats "the same request" and "the same bytes" as equivalent
/// is wrong. Frames arrive length-prefixed and are covered by the AEAD tag, so a
/// legitimate encoder never produces the trailing bytes this now refuses.
fn party_codec() -> impl bincode::Options {
    use bincode::Options;
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .reject_trailing_bytes()
}

impl PartyRequest {
    /// Serialize to bytes for transport inside the encrypted tunnel.
    pub fn to_bytes(&self) -> Vec<u8> {
        use bincode::Options;
        party_codec()
            .serialize(self)
            .expect("PartyRequest serialization is infallible")
    }

    /// Parse from transport bytes; returns `None` on malformed input, which
    /// includes a frame with anything appended to it.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        use bincode::Options;
        party_codec().deserialize(bytes).ok()
    }
}

impl PartyResponse {
    /// Serialize to bytes for transport inside the encrypted tunnel.
    pub fn to_bytes(&self) -> Vec<u8> {
        use bincode::Options;
        party_codec()
            .serialize(self)
            .expect("PartyResponse serialization is infallible")
    }

    /// Parse from transport bytes; returns `None` on malformed input, which
    /// includes a frame with anything appended to it.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        use bincode::Options;
        party_codec().deserialize(bytes).ok()
    }
}

/// Width of the per-frame sequence number prefixed to every Party payload
/// *inside* the encryption, so it is covered by the AEAD tag.
const PARTY_SEQ_BYTES: usize = 8;

/// Monotonic per-direction frame counter. One lives on each end of each
/// direction of a Party tunnel.
///
/// Party frames used to carry nothing but the ciphertext, which meant an
/// attacker positioned on the TCP stream could replay a captured frame — a
/// `PostMessage`, or a server's `Message` broadcast — and both ends would
/// accept it as new. The P2P message loop has enforced a per-session sequence
/// against exactly this since v3; this brings the Party tunnel in line.
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

    /// Accept `seq` only if it advances the counter, rejecting replays and
    /// reordering.
    fn accept(&mut self, seq: u64) -> bool {
        if seq > self.0 {
            self.0 = seq;
            true
        } else {
            false
        }
    }
}

/// Send a serialized Party message over an established v3 tunnel: prefix the
/// next frame sequence, encrypt the result with the session `cipher` (bound to
/// `transport_aad`), and length-prefix it. Used by both the client and the
/// server for every Party request/response.
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

/// Receive and decrypt the next Party message from an established v3 tunnel.
/// Returns an error if the frame fails to authenticate/decrypt, or if its
/// sequence number does not advance (a replayed or reordered frame).
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
        .decrypt(&ciphertext, Some(transport_aad))
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

/// Client-side handle to a Party server over an established Protocol v3 tunnel.
///
/// It completes the client handshake, then exposes `send`/`recv` over the encrypted
/// channel. Responses are a single stream of [`PartyResponse`] — direct replies and
/// pushed broadcasts interleaved — that the application correlates by variant. The
/// future client UI (slice 4) drives this; the server's integration tests exercise
/// it against the real `serve_connection`.
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
        let bytes = recv_framed(
            &mut self.rd,
            &self.cipher,
            &self.transport_aad,
            &mut self.recv_seq,
        )
        .await?;
        PartyResponse::from_bytes(&bytes)
            .ok_or_else(|| anyhow::anyhow!("malformed server response"))
    }
}

impl<W> PartyWriter<W>
where
    W: AsyncWrite + Unpin + Send,
{
    /// Send a request to the server.
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
    /// Complete the client side of the v3 handshake against a Party server and
    /// return a ready client. `chat_id` is this client's advertised session id.
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

    /// The short authentication string for this tunnel (six digits + three
    /// emoji), derived from the handshake transcript exactly as it is for a
    /// peer-to-peer session. Shown to the user when a community server's
    /// identity is being trusted for the first time.
    pub fn sas(&self) -> &str {
        &self.sas
    }

    /// Split into independent read and write halves so an application can run a
    /// receive loop concurrently with sending requests.
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
        let bytes = recv_framed(
            &mut self.stream,
            &self.cipher,
            &self.transport_aad,
            &mut self.recv_seq,
        )
        .await?;
        PartyResponse::from_bytes(&bytes)
            .ok_or_else(|| anyhow::anyhow!("malformed server response"))
    }

    /// Join the server with a username and the optional server password. The
    /// `Joined`/`JoinRejected` reply arrives before any broadcast can reach this
    /// connection (the server registers a connection for broadcasts only once it
    /// has joined), so the next message is the join result.
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

    fn sample_envelope() -> Envelope {
        Envelope {
            tier: TrustTier::Administered,
            sender: Uuid::new_v4(),
            channel: Uuid::new_v4(),
            seq: 7,
            timestamp: 1_700_000_000_000,
            payload: MessagePayload::Text("hello channel".to_string()),
        }
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

    #[test]
    fn file_payload_envelope_roundtrips() {
        let env = Envelope {
            tier: TrustTier::Administered,
            sender: Uuid::new_v4(),
            channel: Uuid::new_v4(),
            seq: 7,
            timestamp: 1234,
            payload: MessagePayload::File(FileMeta {
                hash: blob_hash(b"data"),
                name: "f.bin".to_string(),
                size: 4,
                mime: "application/octet-stream".to_string(),
            }),
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
        let cipher = AesCipher::new(&[3u8; 32]).unwrap();
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
        let bytes = recv_framed(&mut b, &cipher, aad, &mut rx_seq)
            .await
            .unwrap();
        assert_eq!(PartyRequest::from_bytes(&bytes), Some(req));

        // A frame decrypted under the wrong AAD must fail to authenticate.
        send_framed(
            &mut a,
            &cipher,
            aad,
            &mut tx_seq,
            &PartyRequest::ListMembers.to_bytes(),
        )
        .await
        .unwrap();
        assert!(recv_framed(&mut b, &cipher, b"wrong-aad", &mut rx_seq)
            .await
            .is_err());
    }

    /// An attacker on the stream must not be able to make a captured frame
    /// count twice — the P2P loop has enforced this per session since v3, and
    /// the Party tunnel now does too.
    #[tokio::test]
    async fn a_replayed_party_frame_is_rejected() {
        let cipher = AesCipher::new(&[7u8; 32]).unwrap();
        let aad = b"transport|replay-test";
        let mut tx_seq = FrameSeq::new();

        // Capture the exact bytes of one frame by writing into a buffer.
        let mut captured: Vec<u8> = Vec::new();
        send_framed(
            &mut captured,
            &cipher,
            aad,
            &mut tx_seq,
            &PartyRequest::ListMembers.to_bytes(),
        )
        .await
        .unwrap();

        let mut rx_seq = FrameSeq::new();
        let mut once = captured.as_slice();
        recv_framed(&mut once, &cipher, aad, &mut rx_seq)
            .await
            .expect("the first delivery is accepted");

        // Same bytes again: authenticates fine, but the sequence has not moved.
        let mut again = captured.as_slice();
        let err = recv_framed(&mut again, &cipher, aad, &mut rx_seq)
            .await
            .expect_err("a replayed frame must be refused");
        assert!(err.to_string().contains("replayed or out-of-order"));
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

        // Minimal server: handshake, then answer one ListMembers with an empty list.
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
        // Both sides derive the same SAS from the transcript; the join screen
        // shows it before any credential is sent.
        assert!(!client.sas().is_empty());

        let (mut reader, mut writer) = client.split();
        writer.send(&PartyRequest::ListMembers).await.unwrap();
        assert!(matches!(
            reader.recv().await.unwrap(),
            PartyResponse::Members(ref m) if m.is_empty()
        ));
        server.await.unwrap();
    }

    /// A frame with anything appended must be refused.
    ///
    /// bincode 1.x stops at the end of the value and ignores the rest, so
    /// `frame` and `frame || junk` used to decode identically — two distinct
    /// byte strings with one meaning, which makes a frame malleable by anyone
    /// who can touch the bytes and quietly breaks any argument that equates
    /// "same request" with "same bytes".
    #[test]
    fn a_frame_with_trailing_bytes_is_refused() {
        let req = PartyRequest::ListChannels;
        let bytes = req.to_bytes();
        assert_eq!(
            PartyRequest::from_bytes(&bytes),
            Some(PartyRequest::ListChannels),
            "the frame itself still decodes"
        );

        let mut padded = bytes.clone();
        padded.push(0);
        assert!(
            PartyRequest::from_bytes(&padded).is_none(),
            "a padded request must not decode to the same thing"
        );

        let resp = PartyResponse::Ok("fine".to_string());
        let mut padded = resp.to_bytes();
        padded.extend_from_slice(b"junk");
        assert!(
            PartyResponse::from_bytes(&padded).is_none(),
            "a padded response must not decode to the same thing"
        );
    }

    /// The trailing-byte rejection must not change the wire format: rejecting
    /// junk is worth nothing if it also stops old peers being understood.
    #[test]
    fn the_wire_format_is_unchanged() {
        let req = PartyRequest::ListChannels;
        assert_eq!(
            req.to_bytes(),
            bincode::serialize(&req).unwrap(),
            "fixed-width little-endian, exactly as bincode::serialize writes it"
        );
        let resp = PartyResponse::Ok("fine".to_string());
        assert_eq!(resp.to_bytes(), bincode::serialize(&resp).unwrap());
    }
}
