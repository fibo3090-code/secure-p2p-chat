//! In-memory Party server state plus its durable backing store (Phase 1).
//!
//! This is the authoritative runtime model: members, channels, and durable
//! per-channel message history (which is what makes offline buffering work in the
//! Administered tier — a reconnecting member simply fetches history after the last
//! sequence it saw). The pure logic is deliberately network-free so it can be
//! unit-tested deterministically; the TCP/handshake runtime drives these methods.
//!
//! ## Durability
//!
//! Runtime state is kept in memory for fast reads and is mirrored to an embedded
//! **SQLite** database (`party.db`) under the operator's data dir. Each mutation
//! writes its delta (a single row) rather than rewriting a whole snapshot, so cost
//! scales with the change, not the history size. A server created with
//! [`PartyState::new`] has no database and is purely in-memory (used by tests and
//! by callers that manage their own persistence); [`PartyState::load`] opens the
//! database, performs a one-time import of any legacy `party_state.json` snapshot,
//! and reconstructs the in-memory model from the tables.

// Several accessors and fields here (directory/channel listing, presence toggling,
// the member fingerprint binding) are exercised by the tests now and consumed by
// the network runtime; allow them ahead of that wiring.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use messenger_core::party::{
    blob_hash, AuditEntry, ChannelInfo, ChannelKind, Envelope, FileEntry, FileMeta,
    FilePermissions, MemberInfo, MessagePayload, QuotaInfo, Role, TrustTier, UploadTarget,
    MAX_HISTORY_BATCH, MAX_INLINE_FILE_BYTES, MAX_PARTY_FILE_BYTES, PARTY_CHUNK_BYTES,
};
use messenger_core::util::{current_timestamp_millis, sanitize_filename};
use rusqlite::{params, Connection};
use serde::Deserialize;
use uuid::Uuid;

/// Filename of the embedded SQLite database under the operator's data dir.
const DB_FILE: &str = "party.db";
/// Subdirectory under the data dir holding content-addressed file blobs.
const BLOB_DIR: &str = "blobs";
/// Default ceiling on the total bytes of distinct file blobs the server stores,
/// bounding memory/disk growth from uploads. A safety cap until the Phase 3 quota
/// system lands; operators can adjust it via [`PartyState::set_max_blob_bytes`].
const MAX_TOTAL_BLOB_BYTES: u64 = 1024 * 1024 * 1024; // 1 GiB
/// Default per-member ceiling on distinct uploaded bytes (the logical quota).
///
/// The server-wide ceiling alone means the first member to fill it denies the
/// feature to everyone else, so storage is also budgeted per member. Admins are
/// exempt — they are the ones who clean up when it fills.
const MAX_MEMBER_BLOB_BYTES: u64 = 128 * 1024 * 1024; // 128 MiB
/// Most audit entries retained in memory and returned to an admin.
const MAX_AUDIT_ENTRIES: usize = 1000;
/// Most channels one server will hold. Channel creation is open to every joined
/// member and there is no admin role, so this is the only thing standing between
/// a bored member and unbounded server state.
const MAX_CHANNELS: usize = 128;
/// How many channels one member may create within [`CHANNEL_CREATE_WINDOW`].
///
/// [`MAX_CHANNELS`] bounds the server, but it bounds it *once*: any member may
/// create a public channel, so a single one of them can walk the server to its
/// ceiling in a burst — pushing a refreshed directory to every connection each
/// time, burying the real channels, and leaving the community unable to make
/// another until an admin cleans up. The server-wide cap does not distinguish
/// that from a hundred members making one channel each; this does.
///
/// Deliberately generous: making a handful of channels in a minute is something
/// a person setting a community up genuinely does.
const MAX_CHANNELS_PER_MEMBER: usize = 5;
/// The sliding window [`MAX_CHANNELS_PER_MEMBER`] is counted over.
const CHANNEL_CREATE_WINDOW: Duration = Duration::from_secs(60);

/// Filename of the legacy JSON snapshot, imported once into SQLite if present.
const LEGACY_SNAPSHOT_FILE: &str = "party_state.json";

/// Maximum length (in Unicode scalar values) of a member username. Usernames are
/// display handles shown in every client's directory, not free text, so they are
/// bounded well below the transport packet limit to keep the directory renderable
/// and prevent a member from storing/broadcasting a multi-megabyte handle.
pub const MAX_USERNAME_CHARS: usize = 32;
/// Maximum length (in Unicode scalar values) of a channel name, bounded for the
/// same reasons as [`MAX_USERNAME_CHARS`].
pub const MAX_CHANNEL_NAME_CHARS: usize = 64;
/// Maximum size (in bytes) of a channel or DM message's text payload. Mirrors the
/// P2P transport cap (`messenger_core::MAX_TEXT_MESSAGE_BYTES`) so the two message
/// paths agree, and bounds durable storage / broadcast fan-out per message.
pub const MAX_MESSAGE_TEXT_BYTES: usize = messenger_core::MAX_TEXT_MESSAGE_BYTES;

/// Run a blocking filesystem operation without stalling the async runtime.
///
/// `PartyState` is deliberately synchronous — it is a state machine, and making
/// every accessor `async` to accommodate two I/O paths would be the tail wagging
/// the dog. But two of those paths move real bytes: a blob write and a blob read
/// are each up to `MAX_PARTY_FILE_BYTES` (100 MiB), and they run inside
/// `serve_connection`'s task.
///
/// `block_in_place` tells tokio the current thread is about to block so it can
/// move the other tasks off it. It panics on a current-thread runtime, which is
/// what `#[tokio::test]` gives you by default, hence the flavor check: outside a
/// multi-threaded runtime the call is a plain function call, which is correct
/// because there are no sibling tasks to rescue.
///
/// ## What this does *not* fix
///
/// It frees the runtime **worker**; it does not free the **lock**. Every call
/// site here runs under `state.lock().await` (see `connection.rs`), so for the
/// duration of the I/O every other connection's request is still queued behind
/// the mutex. Moving the tasks off this thread does not help them, because they
/// are not waiting on the thread.
///
/// That is worth being precise about, because the comment this replaces claimed
/// otherwise — and a comment saying a problem is solved is what stops the next
/// person from solving it. Genuinely removing the stall means moving the bytes
/// outside the lock: authorise under it, read or write after releasing it. That
/// is a restructuring of the request path, not a change of thread label.
///
/// Two things narrow the exposure in the meantime. `blob_chunk_for` reads one
/// `PARTY_CHUNK_BYTES` chunk rather than the whole file, so the chunked download
/// path — the one large files take — holds the lock for 64 KiB of I/O and not
/// 100 MiB of it. The whole-blob `blob_bytes` path remains as described above.
fn blocking_io<T>(f: impl FnOnce() -> T) -> T {
    use tokio::runtime::{Handle, RuntimeFlavor};
    match Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(f)
        }
        _ => f(),
    }
}

/// Storage claimed by an upload that is being written but is not yet recorded.
#[derive(Debug, Clone, Copy)]
struct Reservation {
    member: Uuid,
    bytes: u64,
}

/// The prefix every in-progress upload's file carries in the blob directory.
///
/// Distinct from a blob's name, which is its content hash, so a sweep can tell
/// the two apart with certainty: no hash starts with this.
const STAGING_PREFIX: &str = "staging_";

/// An upload that has passed its checks and had its bytes taken out of the
/// spool, on its way to disk.
///
/// This is the value that carries an upload across the gap where the state lock
/// is *not* held. It exists in three states, in order:
///
/// 1. Created by [`PartyState::begin_upload`] under the lock, holding the bytes.
/// 2. Written by [`Self::write_staging`] with no lock held — the expensive part,
///    and the whole reason for the split.
/// 3. Consumed by [`PartyState::commit_upload`] or [`PartyState::discard_upload`]
///    under the lock again.
///
/// It must reach step 3. If it is dropped instead — a task cancelled, a
/// connection torn down between steps — `Drop` removes its own staging file so
/// the bytes are not orphaned; the reservation is released by whichever of
/// `commit_upload` / `discard_upload` runs, and by `cancel_uploads_for` when the
/// connection goes away without either.
#[derive(Debug)]
pub struct StagedUpload {
    /// Identifies both the reservation and the staging file.
    id: Uuid,
    uploader: Uuid,
    target: UploadTarget,
    name: String,
    mime: String,
    /// Taken by `write_staging`; `None` afterwards.
    data: Option<Vec<u8>>,
    /// Where blobs live, copied in so step 2 needs nothing from the state.
    blob_dir: Option<PathBuf>,
    /// Set by step 2.
    hash: Option<String>,
    size: u64,
    /// Set once the file is on disk, cleared once it has been renamed into
    /// place or deliberately removed. `Drop` acts on whatever is left.
    staging_path: Option<PathBuf>,
}

impl StagedUpload {
    /// Hash the bytes and write them to this upload's own staging file.
    ///
    /// **Call this with no lock held.** It is up to `MAX_PARTY_FILE_BYTES` of
    /// hashing and I/O, and moving it off the lock is the point of the whole
    /// three-phase shape.
    ///
    /// The staging name is per-upload, never the content hash. Two members
    /// uploading identical content therefore write to different files and
    /// neither can remove the other's: a shared name meant one member's failed
    /// commit could unlink bytes the other's commit was about to record, leaving
    /// a file message that answers "unknown file" forever.
    ///
    /// A failed write **propagates**, unlike the persistence mirrors elsewhere
    /// in this file. Those mirror state that is already correct in memory, so a
    /// failure costs durability and nothing else. These bytes are the file: a
    /// disk-backed store keeps nothing resident, so a write that failed quietly
    /// would leave the upload acknowledged, the file message broadcast to the
    /// whole channel, and every download of it answering "unknown file" for
    /// good. Refusing the upload is the only honest outcome.
    pub fn write_staging(&mut self) -> Result<(), String> {
        let Some(data) = self.data.take() else {
            return Err("upload has already been written".to_string());
        };
        if self.hash.is_none() {
            self.hash = Some(blob_hash(&data));
        }

        let Some(dir) = self.blob_dir.clone() else {
            // Memory-only store: keep the bytes for the commit to record.
            self.data = Some(data);
            return Ok(());
        };

        let path = dir.join(format!("{}{}", STAGING_PREFIX, self.id));
        let write = (|| -> std::io::Result<()> {
            use std::io::Write;
            let mut file = std::fs::File::create(&path)?;
            file.write_all(&data)?;
            // Durable before anything points at it: the rename in `commit_upload`
            // is what publishes these bytes.
            file.sync_all()?;
            Ok(())
        })();

        match write {
            Ok(()) => {
                self.staging_path = Some(path);
                Ok(())
            }
            Err(e) => {
                // Unlink the partial. A disk-full server that leaves one behind
                // on every retry makes itself progressively worse, which is the
                // opposite of what an out-of-space path should do.
                let _ = std::fs::remove_file(&path);
                tracing::error!(error = %e, path = %path.display(), "failed to stage upload");
                Err("the server could not store this file".to_string())
            }
        }
    }
}

impl Drop for StagedUpload {
    fn drop(&mut self) {
        if let Some(path) = self.staging_path.take() {
            // Only ever this upload's own staging file — never a committed blob.
            if let Err(e) = std::fs::remove_file(&path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "could not remove an abandoned staging file"
                    );
                }
            }
        }
    }
}

/// A download that has been authorised but not yet performed.
///
/// The point of the split is that deciding *whether* a member may read a blob is
/// a question about state and needs the lock; actually moving up to
/// `MAX_PARTY_FILE_BYTES` of it does not, and doing it under the lock made every
/// other member's traffic queue behind one download.
#[derive(Debug, Clone)]
pub enum BlobReadPlan {
    /// The memory-only store already holds the bytes; nothing to defer.
    Resident { bytes: Vec<u8>, total: u64 },
    /// Read `len` bytes at `offset` from `path`. `total` is the blob's full size
    /// as recorded, which is what the client is told regardless of how much of
    /// it this particular read returns.
    OnDisk {
        path: PathBuf,
        offset: u64,
        len: u64,
        total: u64,
    },
}

impl BlobReadPlan {
    /// The blob's full size, whichever shape the plan took.
    pub fn total(&self) -> u64 {
        match self {
            Self::Resident { total, .. } => *total,
            Self::OnDisk { total, .. } => *total,
        }
    }

    /// Perform the read. **Call this with no lock held** — that is the entire
    /// reason the plan exists as a separate value.
    ///
    /// A blob whose file has gone (the last reference was deleted while this was
    /// in flight) reads as `None`, which the caller reports the same way as an
    /// unknown hash. That is a race the previous shape could not have, and the
    /// honest answer to it: the file genuinely is not there any more.
    pub fn perform(self) -> Option<Vec<u8>> {
        match self {
            Self::Resident { bytes, .. } => Some(bytes),
            Self::OnDisk {
                path, offset, len, ..
            } => {
                use std::io::{Read, Seek, SeekFrom};
                let read = (|| -> std::io::Result<Vec<u8>> {
                    let mut file = std::fs::File::open(&path)?;
                    if offset > 0 {
                        file.seek(SeekFrom::Start(offset))?;
                    }
                    let mut buf = Vec::with_capacity(len.min(1 << 20) as usize);
                    file.take(len).read_to_end(&mut buf)?;
                    Ok(buf)
                })();
                match read {
                    Ok(bytes) => Some(bytes),
                    Err(e) => {
                        tracing::warn!(
                            path = %path.display(),
                            error = %e,
                            "a blob authorised for reading could not be read"
                        );
                        None
                    }
                }
            }
        }
    }
}

/// Why a join was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinError {
    WrongPassword,
    UsernameTaken,
    EmptyUsername,
    UsernameTooLong,
}

impl JoinError {
    pub fn reason(&self) -> &'static str {
        match self {
            JoinError::WrongPassword => "incorrect server password",
            JoinError::UsernameTaken => "username already taken",
            JoinError::EmptyUsername => "username must not be empty",
            JoinError::UsernameTooLong => "username is too long (max 32 characters)",
        }
    }
}

/// Compare a join attempt's password against the server's in constant time.
///
/// `Option<&str> != Option<&str>` short-circuits on the first differing byte, so
/// how long the comparison takes leaks how much of the password was right. A
/// remote attacker can measure that across many attempts and recover the secret
/// a character at a time — which is exactly what a join endpoint invites.
///
/// The length itself is not secret (it is visible from the response timing of
/// any implementation), so an early length check is fine; the *content* compare
/// is the part that must not branch.
fn password_matches(expected: Option<&str>, supplied: Option<&str>) -> bool {
    use subtle::ConstantTimeEq;
    match (expected, supplied) {
        (None, None) => true,
        (Some(expected), Some(supplied)) => {
            expected.len() == supplied.len()
                && bool::from(expected.as_bytes().ct_eq(supplied.as_bytes()))
        }
        // An open server handed a password, or a protected one handed none.
        _ => false,
    }
}

/// Reject a message whose text payload exceeds [`MAX_MESSAGE_TEXT_BYTES`], so a
/// member cannot store or broadcast an oversized message. Applies to both channel
/// posts and DMs.
fn validate_message_text(text: &str) -> Result<(), String> {
    if text.len() > MAX_MESSAGE_TEXT_BYTES {
        return Err(format!(
            "message is too long (max {MAX_MESSAGE_TEXT_BYTES} bytes)"
        ));
    }
    Ok(())
}

struct Member {
    id: Uuid,
    username: String,
    fingerprint: Option<String>,
    /// Presence is runtime-only and not persisted; members start offline on load.
    online: bool,
    /// Authority on this server. The first member to join becomes [`Role::Owner`]
    /// — that is the operator, who starts the server and then joins it.
    role: Role,
}

struct Channel {
    id: Uuid,
    name: String,
    kind: ChannelKind,
    /// Explicit membership, consulted only for [`ChannelKind::Private`].
    members: Vec<Uuid>,
    /// Durable history; index order is delivery order. `seq` is `index + 1`.
    messages: Vec<Envelope>,
}

/// One share of a file into a channel or DM thread — the provenance record the
/// Drive panel lists and `DeleteFile` removes.
///
/// This is deliberately separate from the message that references the blob.
/// Deleting a file must not rewrite history: sequence numbers are the identity
/// clients merge on, so removing an envelope would renumber everything after it
/// and desynchronise every client that had already fetched the channel. The
/// message stays where it is; the reference and the bytes are what go away.
#[derive(Clone)]
struct FileRef {
    /// Identity of this *share*, not of the content.
    ///
    /// Keying persistence on `(hash, location)` collapsed repeated shares of the
    /// same bytes into one row while the blob's refcount counted them all, so a
    /// restart reloaded fewer references than the count claimed and the last
    /// visible one could never release the blob — storage leaked with nothing
    /// left that could delete it.
    id: Uuid,
    hash: String,
    name: String,
    uploader: Uuid,
    /// Channel id, or DM thread id when `is_dm`.
    location: Uuid,
    is_dm: bool,
    shared_at: u64,
    /// What everyone who can reach this location gets by default.
    default_perms: FilePermissions,
    /// Per-member overrides of `default_perms`, for this share only.
    grants: HashMap<Uuid, FilePermissions>,
}

/// A chunked upload in flight: bytes accumulate here until `FinishUpload`.
///
/// Spooled in memory rather than to disk because [`MAX_PARTY_FILE_BYTES`] and
/// [`MAX_CONCURRENT_UPLOADS`] together bound what one connection can hold, and
/// a temp file would need the same cleanup on every disconnect path anyway.
struct PendingUpload {
    uploader: Uuid,
    name: String,
    mime: String,
    /// Size the uploader declared, checked before a byte was accepted.
    declared: u64,
    target: UploadTarget,
    data: Vec<u8>,
}

/// Uploads one connection may have in flight at once. Each one can hold up to
/// [`MAX_PARTY_FILE_BYTES`], so without a cap a single client could pin
/// arbitrary server memory by starting uploads it never finishes.
pub const MAX_CONCURRENT_UPLOADS: usize = 4;

/// One recorded governance action.
#[derive(Clone)]
struct AuditRecord {
    at: u64,
    actor: Uuid,
    action: String,
    detail: String,
}

/// A 1:1 direct-message thread, keyed by `messenger_core::party::dm_thread_id`.
struct DmThread {
    id: Uuid,
    messages: Vec<Envelope>,
}

/// A content-addressed file blob. The bytes are deduplicated by content hash and
/// reference-counted (each message that references the blob holds one count); the
/// bytes also live on disk under `<data_dir>/blobs/<hash>`.
struct BlobRecord {
    size: u64,
    mime: String,
    /// The bytes, held in memory **only** when there is no blob directory to
    /// read them back from (the in-memory `PartyState::new` mode used by tests).
    /// A disk-backed server keeps `None` here and reads on demand: otherwise the
    /// 1 GiB storage ceiling is also a 1 GiB resident-memory cost.
    data: Option<Vec<u8>>,
    refcount: u32,
}

// --- Legacy JSON snapshot shapes (read-only, for one-time import) ---------------

#[derive(Deserialize)]
struct LegacyMember {
    id: Uuid,
    username: String,
    fingerprint: Option<String>,
}

#[derive(Deserialize)]
struct LegacyChannel {
    id: Uuid,
    name: String,
    kind: ChannelKind,
    messages: Vec<Envelope>,
}

#[derive(Deserialize)]
struct LegacyDmThread {
    id: Uuid,
    messages: Vec<Envelope>,
}

#[derive(Deserialize, Default)]
struct LegacySnapshot {
    members: Vec<LegacyMember>,
    channels: Vec<LegacyChannel>,
    #[serde(default)]
    dm_threads: Vec<LegacyDmThread>,
}

/// The full server state. Server name/password/tier come from configuration, not
/// the store.
pub struct PartyState {
    name: String,
    password: Option<String>,
    tier: TrustTier,
    members: HashMap<Uuid, Member>,
    channels: Vec<Channel>,
    /// Direct-message threads, keyed by their deterministic thread id.
    dm_threads: HashMap<Uuid, DmThread>,
    /// Content-addressed file blobs, keyed by hex SHA-256 of their bytes.
    blobs: HashMap<String, BlobRecord>,
    /// Every share of a file into a channel or DM, in share order.
    file_refs: Vec<FileRef>,
    /// Governance actions, oldest first.
    audit: Vec<AuditRecord>,
    /// Chunked uploads in flight, keyed by the id handed to the uploader.
    uploads: HashMap<Uuid, PendingUpload>,
    /// Storage claimed by uploads that are being written but not yet recorded.
    ///
    /// Keyed by the staging id. Without this the ceilings bound only what is
    /// *stored*, and phase 2 writes the full payload before phase 3 re-checks
    /// them — so twenty members each finishing four 100 MiB uploads would stage
    /// 8 GiB into the blob directory and only then be refused, against a 1 GiB
    /// limit. A reservation makes the ceiling bound in-flight bytes too, which
    /// is the only reading of it that bounds peak disk.
    ///
    /// Deliberately pessimistic: the hash is not known until the bytes have been
    /// read, so a re-upload of content the server already holds reserves its
    /// size and gives it back at commit. Over-reserving briefly refuses an
    /// upload that would have fit; under-reserving lets the disk fill.
    reservations: HashMap<Uuid, Reservation>,
    /// Per-member ceiling on distinct uploaded bytes. Admins are exempt.
    max_member_blob_bytes: u64,
    /// Ceiling on the total bytes of distinct blobs the store will hold.
    max_blob_bytes: u64,
    /// When present, mutations are mirrored to this database.
    db: Option<Connection>,
    /// When present, blob bytes are mirrored to files under this directory.
    blob_dir: Option<PathBuf>,
    /// Recent channel creations per member, for the per-member burst limit.
    ///
    /// Deliberately in memory only. It is a burst limiter, not a quota: a
    /// restart forgiving it costs nothing, because [`MAX_CHANNELS`] still bounds
    /// the total and the channels already made are still there.
    channel_creations: HashMap<Uuid, Vec<Instant>>,
}

impl PartyState {
    /// Create a new in-memory Administered server with a default `general` channel
    /// and no durable backing store. Mutations are not persisted.
    pub fn new(name: impl Into<String>, password: Option<String>) -> Self {
        let general = Channel {
            id: Uuid::new_v4(),
            name: "general".to_string(),
            kind: ChannelKind::Public,
            members: Vec::new(),
            messages: Vec::new(),
        };
        Self {
            name: name.into(),
            password,
            tier: TrustTier::Administered,
            members: HashMap::new(),
            channels: vec![general],
            dm_threads: HashMap::new(),
            blobs: HashMap::new(),
            file_refs: Vec::new(),
            audit: Vec::new(),
            uploads: HashMap::new(),
            reservations: HashMap::new(),
            max_member_blob_bytes: MAX_MEMBER_BLOB_BYTES,
            max_blob_bytes: MAX_TOTAL_BLOB_BYTES,
            db: None,
            blob_dir: None,
            channel_creations: HashMap::new(),
        }
    }

    /// Override the per-member storage allowance (bytes of distinct content).
    pub fn set_max_member_blob_bytes(&mut self, bytes: u64) {
        self.max_member_blob_bytes = bytes;
    }

    /// Override the total-blob-storage ceiling (bytes of distinct content). Used
    /// to configure the quota; defaults to [`MAX_TOTAL_BLOB_BYTES`].
    pub fn set_max_blob_bytes(&mut self, bytes: u64) {
        self.max_blob_bytes = bytes;
    }

    /// Open (creating if needed) the durable state in `<data_dir>/party.db`,
    /// reconstructing the in-memory model from it. A fresh database is seeded with
    /// the default `general` channel. If a legacy `party_state.json` snapshot is
    /// present and the database is empty, it is imported once and then renamed to
    /// `party_state.json.imported` so the import is not repeated. Subsequent
    /// mutations auto-persist their delta.
    pub fn load(
        name: impl Into<String>,
        password: Option<String>,
        data_dir: &Path,
    ) -> anyhow::Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let blob_dir = data_dir.join(BLOB_DIR);
        std::fs::create_dir_all(&blob_dir)?;
        let conn = Connection::open(data_dir.join(DB_FILE))?;
        init_schema(&conn)?;

        // One-time migration from the interim JSON snapshot into SQLite.
        let legacy = data_dir.join(LEGACY_SNAPSHOT_FILE);
        if legacy.exists() && db_is_empty(&conn)? {
            import_legacy_snapshot(&conn, &legacy)?;
            if let Err(e) = std::fs::rename(&legacy, data_dir.join("party_state.json.imported")) {
                tracing::warn!(error = %e, "imported legacy snapshot but could not rename it; it will be ignored because the database is now non-empty");
            }
            tracing::info!("migrated legacy party_state.json snapshot into party.db");
        }

        let mut state = Self::new(name, password);
        state.db = Some(conn);
        state.blob_dir = Some(blob_dir.clone());

        // Sweep staging files a previous run left behind.
        //
        // `StagedUpload::drop` covers an upload abandoned while the process is
        // alive, but nothing runs when the process is killed mid-write. Those
        // files are named by a per-upload id and nothing will ever reference
        // them again, so without this they accumulate silently in a directory
        // whose accounting only ever counted recorded blobs.
        //
        // Safe because the name is unambiguous: blobs are named by content hash
        // and no hash begins with `staging_`.
        sweep_staging_files(&blob_dir);

        if state.count_channels()? > 0 {
            // The database is authoritative; rebuild the in-memory model from it.
            state.reload_from_db()?;
            // A community that predates roles reloads with nobody able to
            // administer it; give it an owner rather than leaving governance
            // permanently unreachable.
            state.ensure_owner();
        } else {
            // Fresh database: persist the seeded default `general` channel.
            let general = &state.channels[0];
            insert_channel_row(
                state.db.as_ref().unwrap(),
                general.id,
                &general.name,
                general.kind,
                &general.members,
                0,
            )?;
        }
        Ok(state)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn tier(&self) -> TrustTier {
        self.tier
    }

    /// The id of the default channel (`general`).
    pub fn default_channel(&self) -> Uuid {
        self.channels[0].id
    }

    /// Join the server: validates the password, then either reactivates the
    /// returning identity (matched by its handshake-verified fingerprint) or
    /// registers a new member with a unique, non-empty username. Returns the
    /// member id, marked online.
    pub fn join(
        &mut self,
        username: &str,
        password: Option<&str>,
        fingerprint: Option<String>,
    ) -> Result<Uuid, JoinError> {
        let username = username.trim();
        if username.is_empty() {
            return Err(JoinError::EmptyUsername);
        }
        if username.chars().count() > MAX_USERNAME_CHARS {
            return Err(JoinError::UsernameTooLong);
        }
        if !password_matches(self.password.as_deref(), password) {
            return Err(JoinError::WrongPassword);
        }

        // Returning identity: a member is never removed on disconnect (only marked
        // offline), so a reconnecting peer is recognised by its verified fingerprint
        // and reuses its existing membership — keeping its id and history — instead
        // of being locked out by its own offline entry as "username taken".
        if let Some(fp) = fingerprint.as_deref() {
            if let Some(existing) = self
                .members
                .values_mut()
                .find(|m| m.fingerprint.as_deref() == Some(fp))
            {
                existing.online = true;
                return Ok(existing.id);
            }
        }

        // New identity: the username must be free (case-insensitive).
        if self
            .members
            .values()
            .any(|m| m.username.eq_ignore_ascii_case(username))
        {
            return Err(JoinError::UsernameTaken);
        }

        let id = Uuid::new_v4();
        // The first identity through the door is the operator: they started the
        // server, so they are the one who owns it. Everyone after is a member.
        // Without this bootstrap nobody could ever administer anything, because
        // only an admin can promote an admin.
        let role = if self.members.is_empty() {
            Role::Owner
        } else {
            Role::Member
        };
        let member = Member {
            id,
            username: username.to_string(),
            fingerprint,
            online: true,
            role,
        };
        self.persist_member(&member);
        self.members.insert(id, member);
        Ok(id)
    }

    /// Make sure a populated server has somebody who can administer it.
    ///
    /// `join` only appoints an owner when the member table is empty, which is
    /// right for a fresh server and wrong for every existing one: a community
    /// that predates roles reloads with every member defaulted to `Member`, and
    /// since `set_role` requires an admin, nobody could ever be promoted. The
    /// operator would be locked out of governance on their own server,
    /// permanently, with no way back short of editing the database by hand.
    ///
    /// The choice has to be deterministic across restarts, and registration
    /// order is not recorded, so the lowest member id wins. It is recorded in
    /// the audit log, because silently handing someone ownership is exactly the
    /// sort of thing an operator should be able to see happened.
    fn ensure_owner(&mut self) {
        if self.members.is_empty() || self.members.values().any(|m| m.role.is_admin()) {
            return;
        }
        let Some(&chosen) = self.members.keys().min() else {
            return;
        };
        let name = {
            let m = self.members.get_mut(&chosen).expect("just selected");
            m.role = Role::Owner;
            m.username.clone()
        };
        if let Some(m) = self.members.get(&chosen) {
            self.persist_member(m);
        }
        tracing::info!(
            member = %chosen,
            username = %name,
            "this community had no admin (it predates roles); promoting its first member to owner"
        );
        let detail = format!("{name} became Owner (no administrator existed)");
        self.record_audit(chosen, "role.bootstrap", &detail);
    }

    /// A member's role, or [`Role::Guest`] for an unknown id (fail closed).
    pub fn role_of(&self, member: Uuid) -> Role {
        self.members.get(&member).map_or(Role::Guest, |m| m.role)
    }

    /// Change `target`'s role on `actor`'s authority.
    ///
    /// Refuses when the actor may not assign that role (you can only grant one
    /// strictly below your own), and always refuses to change the owner's — an
    /// admin who could demote the owner would own the server.
    pub fn set_role(&mut self, actor: Uuid, target: Uuid, role: Role) -> Result<String, String> {
        let actor_role = self.role_of(actor);
        if !actor_role.is_admin() {
            return Err("only admins may change roles".to_string());
        }
        if actor == target {
            return Err("you cannot change your own role".to_string());
        }
        let Some(current) = self.members.get(&target).map(|m| m.role) else {
            return Err("unknown member".to_string());
        };
        if current == Role::Owner {
            return Err("the owner's role cannot be changed".to_string());
        }
        if role == Role::Owner {
            return Err("a server has exactly one owner".to_string());
        }
        if !actor_role.may_assign(role) {
            return Err(format!(
                "you may only assign roles below your own ({})",
                actor_role.label()
            ));
        }
        // Demoting a peer admin needs the same authority as appointing one.
        if current >= actor_role {
            return Err("you cannot change the role of someone at or above your own".to_string());
        }
        let name = {
            let m = self.members.get_mut(&target).expect("checked above");
            m.role = role;
            m.username.clone()
        };
        if let Some(m) = self.members.get(&target) {
            self.persist_member(m);
        }
        let detail = format!("{name} is now {}", role.label());
        self.record_audit(actor, "role.set", &detail);
        Ok(detail)
    }

    /// Mark a member's presence. Used when a connection drops or reconnects.
    /// Presence is runtime-only and intentionally not persisted.
    pub fn set_online(&mut self, member: Uuid, online: bool) {
        if let Some(m) = self.members.get_mut(&member) {
            m.online = online;
        }
    }

    pub fn is_member(&self, member: Uuid) -> bool {
        self.members.contains_key(&member)
    }

    /// Record a governance action. Kept bounded so a long-lived server does not
    /// accumulate an unbounded log in memory.
    fn record_audit(&mut self, actor: Uuid, action: &str, detail: &str) {
        let record = AuditRecord {
            at: current_timestamp_millis(),
            actor,
            action: action.to_string(),
            detail: detail.to_string(),
        };
        self.persist_audit(&record);
        self.audit.push(record);
        if self.audit.len() > MAX_AUDIT_ENTRIES {
            let overflow = self.audit.len() - MAX_AUDIT_ENTRIES;
            self.audit.drain(..overflow);
        }
    }

    /// The audit log, newest first, for an admin. Non-admins get nothing: the log
    /// records who did what to whom, which is exactly the sort of thing an
    /// ordinary member should not be able to enumerate.
    pub fn audit_log(&self, member: Uuid, limit: usize) -> Result<Vec<AuditEntry>, String> {
        if !self.role_of(member).is_admin() {
            return Err("only admins may read the audit log".to_string());
        }
        Ok(self
            .audit
            .iter()
            .rev()
            .take(limit)
            .map(|r| AuditEntry {
                at: r.at,
                actor: r.actor,
                actor_name: self.username_of(r.actor),
                action: r.action.clone(),
                detail: r.detail.clone(),
            })
            .collect())
    }

    /// Display name for a member id, falling back for one who no longer exists.
    fn username_of(&self, id: Uuid) -> String {
        self.members
            .get(&id)
            .map(|m| m.username.clone())
            .unwrap_or_else(|| "unknown".to_string())
    }

    /// The member directory, sorted by username for stable output.
    pub fn members(&self) -> Vec<MemberInfo> {
        let mut list: Vec<MemberInfo> = self
            .members
            .values()
            .map(|m| MemberInfo {
                id: m.id,
                username: m.username.clone(),
                online: m.online,
                role: m.role,
            })
            .collect();
        list.sort_by(|a, b| a.username.cmp(&b.username));
        list
    }

    /// The channel list as `member` may see it: channels they cannot read are
    /// omitted entirely, so a private channel does not even advertise its
    /// existence to somebody who is not in it.
    pub fn channels_for(&self, member: Uuid) -> Vec<ChannelInfo> {
        let role = self.role_of(member);
        self.channels
            .iter()
            .filter(|c| c.kind.may_read(role, c.members.contains(&member)))
            .map(|c| ChannelInfo {
                id: c.id,
                name: c.name.clone(),
                kind: c.kind,
                members: c.members.clone(),
            })
            .collect()
    }

    /// Whether every joined member may read `channel` — true for the kinds whose
    /// access is server-wide, false for `Private`, which has its own list.
    ///
    /// This decides how a post to it may be fanned out: a channel everyone can
    /// read may be broadcast, and one that is not must be delivered only to the
    /// members who may see it.
    pub fn channel_is_open_to_all(&self, channel: Uuid) -> bool {
        !matches!(
            self.channel(channel).map(|c| c.kind),
            Some(ChannelKind::Private) | None
        )
    }

    /// The members allowed to read `channel`. Used to fan a restricted channel's
    /// messages out by member instead of broadcasting them to every connection.
    pub fn members_who_can_read(&self, channel: Uuid) -> Vec<Uuid> {
        self.members
            .keys()
            .copied()
            .filter(|m| self.member_can_read_channel(*m, channel))
            .collect()
    }

    /// Every channel, regardless of access. Used by persistence and tests.
    pub fn channels(&self) -> Vec<ChannelInfo> {
        self.channels
            .iter()
            .map(|c| ChannelInfo {
                id: c.id,
                name: c.name.clone(),
                kind: c.kind,
                members: c.members.clone(),
            })
            .collect()
    }

    /// Create a new public channel with a unique, non-empty name. Returns the new
    /// channel's info. Errors on empty or duplicate (case-insensitive) names.
    pub fn create_channel(&mut self, name: &str) -> Result<ChannelInfo, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("channel name must not be empty".to_string());
        }
        if name.chars().count() > MAX_CHANNEL_NAME_CHARS {
            return Err(format!(
                "channel name is too long (max {MAX_CHANNEL_NAME_CHARS} characters)"
            ));
        }
        if self
            .channels
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(name))
        {
            return Err("a channel with that name already exists".to_string());
        }
        // Any joined member may create a channel and there is no admin role, so
        // without a ceiling one member can mint channels without bound — each
        // one held in memory, persisted, and broadcast as a refreshed channel
        // list to every other connection.
        if self.channels.len() >= MAX_CHANNELS {
            return Err(format!(
                "this server already has the maximum of {MAX_CHANNELS} channels"
            ));
        }
        let position = self.channels.len();
        let channel = Channel {
            id: Uuid::new_v4(),
            name: name.to_string(),
            kind: ChannelKind::Public,
            members: Vec::new(),
            messages: Vec::new(),
        };
        let info = ChannelInfo {
            id: channel.id,
            name: channel.name.clone(),
            kind: channel.kind,
            members: Vec::new(),
        };
        self.persist_channel(&channel, position);
        self.channels.push(channel);
        Ok(info)
    }

    /// Refuse a member who has created [`MAX_CHANNELS_PER_MEMBER`] channels
    /// inside [`CHANNEL_CREATE_WINDOW`].
    ///
    /// Admins are limited too. The point is not distrust of admins — it is that
    /// a stolen or careless admin session is exactly the one that can reach the
    /// restricted channel kinds, and a burst limit costs a legitimate admin
    /// nothing they would notice.
    fn check_channel_rate(&mut self, creator: Uuid, now: Instant) -> Result<(), String> {
        // Members who have gone quiet stop being tracked, so this map does not
        // outgrow the membership it is keyed by.
        self.channel_creations.retain(|_, seen| {
            seen.iter()
                .any(|t| now.duration_since(*t) < CHANNEL_CREATE_WINDOW)
        });
        let recent = self
            .channel_creations
            .get(&creator)
            .map(|seen| {
                seen.iter()
                    .filter(|t| now.duration_since(**t) < CHANNEL_CREATE_WINDOW)
                    .count()
            })
            .unwrap_or(0);
        if recent >= MAX_CHANNELS_PER_MEMBER {
            return Err(format!(
                "you have created {MAX_CHANNELS_PER_MEMBER} channels in the last \
                 {} seconds — wait a moment before creating another",
                CHANNEL_CREATE_WINDOW.as_secs()
            ));
        }
        Ok(())
    }

    /// Record a successful creation against `creator`'s allowance.
    fn record_channel_creation(&mut self, creator: Uuid, now: Instant) {
        let seen = self.channel_creations.entry(creator).or_default();
        seen.retain(|t| now.duration_since(*t) < CHANNEL_CREATE_WINDOW);
        seen.push(now);
    }

    /// Create a channel of a given kind on `creator`'s authority.
    ///
    /// Anyone who may write can make a `Public` channel; the restricted kinds
    /// (`Private`, `Locked`, `Announce`) are administrative, because each of them
    /// is a way to control what other members may see or say. The creator of a
    /// private channel is always in it — otherwise they could lock themselves out
    /// of the channel they just made.
    pub fn create_channel_of_kind(
        &mut self,
        creator: Uuid,
        name: &str,
        kind: ChannelKind,
        members: Vec<Uuid>,
    ) -> Result<ChannelInfo, String> {
        self.create_channel_of_kind_at(creator, name, kind, members, Instant::now())
    }

    /// [`Self::create_channel_of_kind`] against a caller-supplied clock, so the
    /// per-member rate limit is testable without sleeping for a minute.
    pub fn create_channel_of_kind_at(
        &mut self,
        creator: Uuid,
        name: &str,
        kind: ChannelKind,
        mut members: Vec<Uuid>,
        now: Instant,
    ) -> Result<ChannelInfo, String> {
        let role = self.role_of(creator);
        if !role.can_create_channel() {
            return Err("your role on this server is read-only".to_string());
        }
        if kind != ChannelKind::Public && !role.is_admin() {
            return Err(format!(
                "only admins may create a {} channel",
                kind.label().to_lowercase()
            ));
        }
        // Checked before the channel is made, and recorded only once it is, so a
        // rejected name (duplicate, too long, over the server cap) costs the
        // member nothing against their allowance.
        self.check_channel_rate(creator, now)?;
        let info = self.create_channel(name)?;
        self.record_channel_creation(creator, now);
        if kind != ChannelKind::Public {
            if kind == ChannelKind::Private {
                members.retain(|m| self.members.contains_key(m));
                if !members.contains(&creator) {
                    members.push(creator);
                }
            } else {
                members.clear();
            }
            let position = self
                .channels
                .iter()
                .position(|c| c.id == info.id)
                .expect("just created");
            {
                let c = &mut self.channels[position];
                c.kind = kind;
                c.members = members.clone();
            }
            let c = &self.channels[position];
            self.persist_channel(c, position);
        }
        let detail = format!("created {} channel #{}", kind.label().to_lowercase(), name);
        self.record_audit(creator, "channel.create", &detail);
        Ok(ChannelInfo {
            kind,
            members,
            ..info
        })
    }

    /// Change a channel's kind and private membership (admins only).
    pub fn set_channel_access(
        &mut self,
        actor: Uuid,
        channel: Uuid,
        kind: ChannelKind,
        mut members: Vec<Uuid>,
    ) -> Result<String, String> {
        if !self.role_of(actor).is_admin() {
            return Err("only admins may change channel access".to_string());
        }
        if kind == ChannelKind::Private {
            members.retain(|m| self.members.contains_key(m));
            if !members.contains(&actor) {
                members.push(actor);
            }
        } else {
            members.clear();
        }
        let Some(position) = self.channels.iter().position(|c| c.id == channel) else {
            return Err("unknown channel".to_string());
        };
        let name = {
            let c = &mut self.channels[position];
            c.kind = kind;
            c.members = members;
            c.name.clone()
        };
        let c = &self.channels[position];
        self.persist_channel(c, position);
        let detail = format!("#{name} is now {}", kind.label().to_lowercase());
        self.record_audit(actor, "channel.access", &detail);
        Ok(detail)
    }

    /// Delete a channel and its history (admins only).
    ///
    /// The last channel cannot be deleted: `default_channel` indexes
    /// `channels[0]`, and a server with no channels has nowhere to post.
    pub fn delete_channel(&mut self, actor: Uuid, channel: Uuid) -> Result<String, String> {
        if !self.role_of(actor).is_admin() {
            return Err("only admins may delete channels".to_string());
        }
        if self.channels.len() <= 1 {
            return Err("a server must keep at least one channel".to_string());
        }
        let Some(position) = self.channels.iter().position(|c| c.id == channel) else {
            return Err("unknown channel".to_string());
        };
        let removed = self.channels.remove(position);
        // Release every file shared here before the references disappear with
        // the channel, or their bytes would be stranded with nothing holding a
        // count and nothing able to reach them.
        let stranded: Vec<String> = self
            .file_refs
            .iter()
            .filter(|r| !r.is_dm && r.location == channel)
            .map(|r| r.hash.clone())
            .collect();
        self.file_refs.retain(|r| r.is_dm || r.location != channel);
        for hash in stranded {
            self.release_blob(&hash);
        }
        self.delete_channel_rows(channel);
        // Positions are an index, so everything after the hole has to move up.
        for (i, c) in self.channels.iter().enumerate().skip(position) {
            self.persist_channel(c, i);
        }
        let detail = format!("deleted channel #{}", removed.name);
        self.record_audit(actor, "channel.delete", &detail);
        Ok(detail)
    }

    fn channel_mut(&mut self, id: Uuid) -> Option<&mut Channel> {
        self.channels.iter_mut().find(|c| c.id == id)
    }

    fn channel(&self, id: Uuid) -> Option<&Channel> {
        self.channels.iter().find(|c| c.id == id)
    }

    /// Whether a member may *read* `channel` (its history, and the files it
    /// references).
    ///
    /// [`ChannelKind`] was stored, persisted, shipped to clients, and enforced
    /// nowhere: `Private` channels were readable by every joined member, and
    /// posting to a `Locked` or `Announce` channel worked. There is no
    /// per-channel membership list to consult yet, so the honest behaviour is to
    /// fail **closed** — a kind whose access rule cannot be evaluated denies
    /// rather than allows. `create_channel` only ever makes `Public` channels,
    /// so this changes nothing today; it means the type stops promising an
    /// access control it does not have, and the first per-channel ACL cannot
    /// ship silently open.
    fn member_can_read_channel(&self, member: Uuid, channel: Uuid) -> bool {
        let role = self.role_of(member);
        match self.channel(channel) {
            Some(c) => c.kind.may_read(role, c.members.contains(&member)),
            None => false,
        }
    }

    /// Whether `member` may *post* to `channel`, per their role and the channel's
    /// kind (see [`ChannelKind::may_post`]).
    fn member_can_post_to_channel(&self, member: Uuid, channel: Uuid) -> Result<(), String> {
        let role = self.role_of(member);
        match self.channel(channel) {
            Some(c) => c
                .kind
                .may_post(role, c.members.contains(&member))
                .map_err(str::to_string),
            None => Err("unknown channel".to_string()),
        }
    }

    /// Append a text message from `sender` to `channel`, assigning the next
    /// per-channel sequence number. The message is stored durably (offline
    /// buffering) and the assigned envelope is returned for broadcast.
    ///
    /// Errors if the sender is not a member or the channel does not exist.
    pub fn post_message(
        &mut self,
        sender: Uuid,
        channel: Uuid,
        text: String,
    ) -> Result<Envelope, String> {
        if !self.is_member(sender) {
            return Err("sender is not a member of this server".to_string());
        }
        validate_message_text(&text)?;
        self.member_can_post_to_channel(sender, channel)?;
        let tier = self.tier;
        let chan = self
            .channel_mut(channel)
            .ok_or_else(|| "unknown channel".to_string())?;
        let seq = chan.messages.len() as u64 + 1;
        let envelope = Envelope {
            tier,
            sender,
            channel,
            seq,
            timestamp: current_timestamp_millis(),
            payload: MessagePayload::Text(text),
        };
        chan.messages.push(envelope.clone());
        self.persist_message(&envelope);
        Ok(envelope)
    }

    /// Channel history strictly after `since_seq` (offline catch-up), capped at
    /// [`MAX_HISTORY_BATCH`] envelopes per call so the reply always fits in one
    /// frame. The caller asks again with the last `seq` it received to page
    /// through the rest.
    ///
    /// Returning a whole channel at once meant that once its history exceeded
    /// `MAX_PACKET_SIZE` the response could not be sent at all: the send failed,
    /// the connection dropped, and the community became unjoinable with nothing
    /// on screen to explain it.
    ///
    /// Channels the member may not read (see [`Self::member_can_read_channel`])
    /// yield `[]` — the same as an unknown channel, so the reply does not reveal
    /// that the channel exists.
    pub fn history_since(&self, member: Uuid, channel: Uuid, since_seq: u64) -> Vec<Envelope> {
        if !self.is_member(member) || !self.member_can_read_channel(member, channel) {
            return Vec::new();
        }
        match self.channel(channel) {
            Some(c) => c
                .messages
                .iter()
                .filter(|m| m.seq > since_seq)
                .take(MAX_HISTORY_BATCH)
                .cloned()
                .collect(),
            None => Vec::new(),
        }
    }

    /// Send a direct message from `from` to `to`. Both must be members. The message
    /// is stored durably in their (deterministic) 1:1 thread and the assigned
    /// envelope is returned (its `channel` is the DM thread id) for delivery to both
    /// participants. Errors if either party is not a member.
    pub fn post_dm(&mut self, from: Uuid, to: Uuid, text: String) -> Result<Envelope, String> {
        if !self.is_member(from) {
            return Err("sender is not a member of this server".to_string());
        }
        if !self.is_member(to) {
            return Err("recipient is not a member of this server".to_string());
        }
        validate_message_text(&text)?;
        let thread_id = messenger_core::party::dm_thread_id(from, to);
        let tier = self.tier;
        let thread = self
            .dm_threads
            .entry(thread_id)
            .or_insert_with(|| DmThread {
                id: thread_id,
                messages: Vec::new(),
            });
        let seq = thread.messages.len() as u64 + 1;
        let envelope = Envelope {
            tier,
            sender: from,
            channel: thread_id,
            seq,
            timestamp: current_timestamp_millis(),
            payload: MessagePayload::Text(text),
        };
        thread.messages.push(envelope.clone());
        self.persist_dm(thread_id, &envelope);
        Ok(envelope)
    }

    /// Direct-message history for a thread strictly after `since_seq`, paged the
    /// same way as [`Self::history_since`] and for the same reason.
    pub fn dm_history(&self, thread_id: Uuid, since_seq: u64) -> Vec<Envelope> {
        match self.dm_threads.get(&thread_id) {
            Some(t) => t
                .messages
                .iter()
                .filter(|m| m.seq > since_seq)
                .take(MAX_HISTORY_BATCH)
                .cloned()
                .collect(),
            None => Vec::new(),
        }
    }

    // --- File sharing (Phase 2, slice 1) ----------------------------------------

    /// This member's storage allowance, or `None` when they are exempt.
    fn member_blob_limit(&self, member: Uuid) -> Option<u64> {
        if self.role_of(member).is_admin() {
            None
        } else {
            Some(self.max_member_blob_bytes)
        }
    }

    /// Distinct bytes this member currently holds.
    ///
    /// Counted over *distinct* blobs, so sharing one file into three channels
    /// costs its size once — the same rule the server-wide ceiling uses. Freeing
    /// it therefore requires deleting every reference, which is what makes the
    /// number match what the store actually holds on their behalf.
    fn member_blob_bytes(&self, member: Uuid) -> u64 {
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut total = 0u64;
        for r in self.file_refs.iter().filter(|r| r.uploader == member) {
            if seen.insert(r.hash.as_str()) {
                total = total
                    .saturating_add(self.blobs.get(&r.hash).map(|b| b.size).unwrap_or_default());
            }
        }
        // Plus anything of theirs currently being staged. Without this a member
        // can start several uploads that each fit under the allowance and only
        // discover at commit that together they do not — after every byte has
        // already been written.
        for r in self.reservations.values().filter(|r| r.member == member) {
            total = total.saturating_add(r.bytes);
        }
        total
    }

    /// Storage usage for the Drive panel's quota readout.
    pub fn quota_for(&self, member: Uuid) -> QuotaInfo {
        QuotaInfo {
            used: self.member_blob_bytes(member),
            limit: self.member_blob_limit(member),
            server_used: self.blobs.values().map(|r| r.size).sum(),
            server_limit: self.max_blob_bytes,
        }
    }

    /// Record that `uploader` shared `meta` into `location`.
    fn record_file_ref(&mut self, uploader: Uuid, meta: &FileMeta, location: Uuid, is_dm: bool) {
        let entry = FileRef {
            id: Uuid::new_v4(),
            default_perms: FilePermissions::default(),
            grants: HashMap::new(),
            hash: meta.hash.clone(),
            name: meta.name.clone(),
            uploader,
            location,
            is_dm,
            shared_at: current_timestamp_millis(),
        };
        self.persist_file_ref(&entry);
        self.file_refs.push(entry);
    }

    /// What `member` may do with the share `r`.
    ///
    /// Resolution order: an admin holds everything; anyone who cannot reach the
    /// location holds nothing; within a location they can reach, the uploader
    /// holds everything, an explicit grant overrides the location default, and
    /// otherwise the default applies. A guest never holds a write right,
    /// whatever they were granted.
    fn effective_perms(&self, member: Uuid, r: &FileRef) -> FilePermissions {
        if self.role_of(member).is_admin() {
            return FilePermissions::all();
        }
        // Reachability gates everything below it, uploaders included. Having
        // once shared a file into a channel does not survive being shut out of
        // that channel — otherwise making a channel private would leave its
        // files reachable by exactly the people just excluded from it.
        if !self.can_reach_location(member, r) {
            return FilePermissions::none();
        }
        if r.uploader == member {
            return FilePermissions::all();
        }
        // A guest is read-only everywhere, and that outranks any grant: the
        // rest of the server refuses their writes, so a `delete` grant here
        // would be the one place it did not.
        let granted = r
            .grants
            .get(&member)
            .copied()
            .unwrap_or(r.default_perms)
            .normalized();
        if self.role_of(member).can_write() {
            granted
        } else {
            FilePermissions {
                delete: false,
                share: false,
                ..granted
            }
        }
    }

    /// Whether `member` can reach the place this file was shared — the channel
    /// they may read, or their own DM thread.
    fn can_reach_location(&self, member: Uuid, r: &FileRef) -> bool {
        if r.is_dm {
            self.members
                .keys()
                .any(|o| messenger_core::party::dm_thread_id(member, *o) == r.location)
        } else {
            self.member_can_read_channel(member, r.location)
        }
    }

    /// Re-share a file the caller already holds into another channel or DM,
    /// without moving the bytes again.
    ///
    /// `from` names the reference the caller's rights come from: one blob can
    /// sit in several places under different grants, so "may I share this?" is
    /// only answerable against a specific share. The new reference starts at the
    /// default grant, not at the source's — re-sharing passes on the file, not
    /// the sharer's authority over it.
    pub fn share_file(
        &mut self,
        actor: Uuid,
        hash: &str,
        from: Uuid,
        target: UploadTarget,
    ) -> Result<Envelope, String> {
        let Some(source) = self
            .file_refs
            .iter()
            .find(|r| r.hash == hash && r.location == from)
            .cloned()
        else {
            return Err("that file is not shared there".to_string());
        };
        if !self.effective_perms(actor, &source).share {
            return Err("you do not have permission to share this file".to_string());
        }
        let (location, is_dm) = match target {
            UploadTarget::Channel(channel) => {
                self.member_can_post_to_channel(actor, channel)?;
                (channel, false)
            }
            UploadTarget::Dm(to) => {
                if !self.is_member(to) {
                    return Err("recipient is not a member of this server".to_string());
                }
                if !self.role_of(actor).can_write() {
                    return Err("your role on this server is read-only".to_string());
                }
                (messenger_core::party::dm_thread_id(actor, to), true)
            }
        };
        if self
            .file_refs
            .iter()
            .any(|r| r.hash == hash && r.location == location)
        {
            return Err("that file is already shared there".to_string());
        }

        // The bytes exist already, so this is a reference and a message — it
        // costs the sharer a refcount, and it counts against *their* quota,
        // because they are now one of the people holding it here.
        let meta = {
            let Some(rec) = self.blobs.get_mut(hash) else {
                return Err("that file is no longer stored here".to_string());
            };
            rec.refcount += 1;
            let refcount = rec.refcount;
            let (size, mime) = (rec.size, rec.mime.clone());
            self.persist_blob_refcount(hash, refcount);
            FileMeta {
                hash: hash.to_string(),
                name: source.name.clone(),
                size,
                mime,
            }
        };
        self.record_file_ref(actor, &meta, location, is_dm);

        let tier = self.tier;
        let envelope = if is_dm {
            let thread = self.dm_threads.entry(location).or_insert_with(|| DmThread {
                id: location,
                messages: Vec::new(),
            });
            let seq = thread.messages.len() as u64 + 1;
            let env = Envelope {
                tier,
                sender: actor,
                channel: location,
                seq,
                timestamp: current_timestamp_millis(),
                payload: MessagePayload::File(meta),
            };
            thread.messages.push(env.clone());
            self.persist_dm(location, &env);
            env
        } else {
            let chan = self.channel_mut(location).expect("checked above");
            let seq = chan.messages.len() as u64 + 1;
            let env = Envelope {
                tier,
                sender: actor,
                channel: location,
                seq,
                timestamp: current_timestamp_millis(),
                payload: MessagePayload::File(meta),
            };
            chan.messages.push(env.clone());
            self.persist_message(&env);
            env
        };
        let detail = format!("shared {} again", source.name);
        self.record_audit(actor, "file.share", &detail);
        Ok(envelope)
    }

    /// Change what a share grants, either by default or for one member.
    ///
    /// Refused unless the caller's own effective rights cover what they are
    /// handing out — the "you can only delegate rights you hold" rule. Only the
    /// uploader or an admin may change a share's permissions at all; holding
    /// `share` lets you spread a file, not re-write who else may do what.
    pub fn set_file_permissions(
        &mut self,
        actor: Uuid,
        hash: &str,
        location: Uuid,
        member: Option<Uuid>,
        perms: FilePermissions,
    ) -> Result<String, String> {
        let Some(index) = self
            .file_refs
            .iter()
            .position(|r| r.hash == hash && r.location == location)
        else {
            return Err("that file is not shared here".to_string());
        };
        let entry = self.file_refs[index].clone();
        let is_admin = self.role_of(actor).is_admin();
        if entry.uploader != actor && !is_admin {
            return Err(
                "only the member who shared a file, or an admin, may change what it grants"
                    .to_string(),
            );
        }
        let perms = perms.normalized();
        if !self.effective_perms(actor, &entry).covers(perms) {
            return Err("you cannot grant a right you do not hold yourself".to_string());
        }
        if let Some(target) = member {
            if !self.is_member(target) {
                return Err("unknown member".to_string());
            }
        }

        let name = entry.name.clone();
        let detail = {
            let r = &mut self.file_refs[index];
            match member {
                Some(target) => {
                    r.grants.insert(target, perms);
                    format!("changed what {name} grants {}", self.member_label(target))
                }
                None => {
                    r.default_perms = perms;
                    format!("changed what {name} grants by default")
                }
            }
        };
        let updated = self.file_refs[index].clone();
        self.persist_file_ref(&updated);
        self.record_audit(actor, "file.permissions", &detail);
        Ok(detail)
    }

    /// A member's display name, for audit detail.
    fn member_label(&self, id: Uuid) -> String {
        self.username_of(id)
    }

    /// Drop one reference to a blob, deleting the bytes when the last one goes.
    ///
    /// This is the half of reference counting that never existed: uploads
    /// incremented the count and nothing ever decremented it, so storage only
    /// ever grew and the count was decorative.
    fn release_blob(&mut self, hash: &str) {
        let Some(rec) = self.blobs.get_mut(hash) else {
            return;
        };
        rec.refcount = rec.refcount.saturating_sub(1);
        if rec.refcount > 0 {
            let refcount = rec.refcount;
            self.persist_blob_refcount(hash, refcount);
            return;
        }
        self.blobs.remove(hash);
        self.delete_blob_row(hash);
        if let Some(dir) = &self.blob_dir {
            let path = dir.join(hash);
            if let Err(e) = blocking_io(|| std::fs::remove_file(&path)) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::error!(error = %e, path = %path.display(), "failed to delete blob bytes");
                }
            }
        }
    }

    /// Delete one share of a file. The uploader may remove their own; an admin
    /// may remove anyone's.
    ///
    /// The *message* referencing the file stays in history: sequence numbers are
    /// what clients merge on, so removing an envelope would renumber the channel
    /// and desynchronise everyone who had already fetched it. What goes away is
    /// the reference and — once nothing else holds it — the bytes, after which
    /// the file card reports that the file was deleted.
    pub fn delete_file(
        &mut self,
        actor: Uuid,
        hash: &str,
        location: Uuid,
    ) -> Result<String, String> {
        let Some(index) = self
            .file_refs
            .iter()
            .position(|r| r.hash == hash && r.location == location)
        else {
            return Err("that file is not shared here".to_string());
        };
        let entry = self.file_refs[index].clone();
        // One check now covers who may delete *and* whether they can see where
        // it was shared: `effective_perms` returns nothing at all for a
        // location the member cannot reach.
        if !self.effective_perms(actor, &entry).delete {
            return Err("you do not have permission to delete this file".to_string());
        }
        self.file_refs.remove(index);
        self.delete_file_ref_row(entry.id);
        self.release_blob(hash);
        let detail = format!("deleted file {}", entry.name);
        self.record_audit(actor, "file.delete", &detail);
        Ok(detail)
    }

    /// Every file `member` may see, newest first — the Drive panel's contents.
    ///
    /// Access follows the same rule as downloading: files shared in channels they
    /// can read, and in their own DM threads.
    pub fn files_for(&self, member: Uuid) -> Vec<FileEntry> {
        let mut out: Vec<FileEntry> = self
            .file_refs
            .iter()
            .filter(|r| self.effective_perms(member, r).view)
            .map(|r| {
                let (location_name, is_dm) = if r.is_dm {
                    (self.dm_location_name(member, r.location), true)
                } else {
                    (
                        self.channel(r.location)
                            .map(|c| format!("#{}", c.name))
                            .unwrap_or_else(|| "#unknown".to_string()),
                        false,
                    )
                };
                FileEntry {
                    hash: r.hash.clone(),
                    name: r.name.clone(),
                    size: self.blobs.get(&r.hash).map(|b| b.size).unwrap_or_default(),
                    mime: self
                        .blobs
                        .get(&r.hash)
                        .map(|b| b.mime.clone())
                        .unwrap_or_default(),
                    uploader: r.uploader,
                    uploader_name: self.username_of(r.uploader),
                    location: r.location,
                    location_name,
                    is_dm,
                    shared_at: r.shared_at,
                    can_delete: self.effective_perms(member, r).delete,
                    perms: self.effective_perms(member, r),
                }
            })
            .collect();
        out.sort_by_key(|f| std::cmp::Reverse(f.shared_at));
        out
    }

    /// Label a DM thread from `viewer`'s side: the other participant's name.
    fn dm_location_name(&self, viewer: Uuid, thread: Uuid) -> String {
        for other in self.members.keys() {
            if messenger_core::party::dm_thread_id(viewer, *other) == thread {
                return format!("DM with {}", self.username_of(*other));
            }
        }
        "Direct message".to_string()
    }

    // --- Chunked upload (Phase 2, slice 2) --------------------------------------

    /// Accept a chunked upload, or refuse it before a single byte moves.
    ///
    /// Everything checkable up front is checked here — the declared size against
    /// the hard ceiling and the uploader's remaining allowance, and the target
    /// against the permission rules — because the alternative is spooling a
    /// hundred megabytes and then saying no.
    /// Begin a chunked upload, if this member is allowed one.
    ///
    /// The concurrency cap counts the uploads *this state* is holding for the
    /// member, not the ones the connection believes it has open. Those two
    /// numbers used to be allowed to disagree: `FinishUpload` cleared the
    /// connection's list before calling `finish_upload`, which then returned
    /// `Err` on the incomplete path without dropping the spool. The connection
    /// then reported zero in flight while the server still held up to
    /// `MAX_PARTY_FILE_BYTES` per abandoned upload, so the cap could be walked
    /// past indefinitely — declare 100 MiB, send 99, finish, repeat. Deriving
    /// the count from the spools themselves makes the cap true regardless of
    /// what any caller remembers.
    pub fn start_upload(
        &mut self,
        uploader: Uuid,
        name: String,
        mime: String,
        size: u64,
        target: UploadTarget,
    ) -> Result<Uuid, String> {
        if !self.is_member(uploader) {
            return Err("sender is not a member of this server".to_string());
        }
        if self.uploads_in_flight(uploader) >= MAX_CONCURRENT_UPLOADS {
            return Err(format!(
                "you already have {MAX_CONCURRENT_UPLOADS} uploads in progress"
            ));
        }
        if size == 0 {
            return Err("file is empty".to_string());
        }
        if size > MAX_PARTY_FILE_BYTES {
            return Err(format!(
                "file is larger than the {} limit for this server",
                messenger_core::util::format_size(MAX_PARTY_FILE_BYTES)
            ));
        }
        match target {
            UploadTarget::Channel(channel) => self.member_can_post_to_channel(uploader, channel)?,
            UploadTarget::Dm(to) => {
                if !self.role_of(uploader).can_write() {
                    return Err("your role on this server is read-only".to_string());
                }
                if !self.is_member(to) {
                    return Err("recipient is not a member of this server".to_string());
                }
            }
        }
        // The quota check is provisional: content the server already holds costs
        // nothing, but that is only knowable once the bytes have arrived, so the
        // pessimistic answer is the one to give before accepting them.
        if let Some(limit) = self.member_blob_limit(uploader) {
            let used = self.member_blob_bytes(uploader);
            if used.saturating_add(size) > limit {
                return Err(format!(
                    "this would exceed your {} file storage allowance ({} of it already used)",
                    messenger_core::util::format_size(limit),
                    messenger_core::util::format_size(used)
                ));
            }
        }
        let id = Uuid::new_v4();
        self.uploads.insert(
            id,
            PendingUpload {
                uploader,
                name,
                mime,
                declared: size,
                target,
                data: Vec::with_capacity(size.min(PARTY_CHUNK_BYTES as u64 * 4) as usize),
            },
        );
        Ok(id)
    }

    /// Append one chunk. Refuses anything that would take the upload past the
    /// size it declared — checking after the write would still have put the
    /// excess in memory, which is the whole thing being bounded.
    pub fn upload_chunk(
        &mut self,
        uploader: Uuid,
        upload: Uuid,
        data: &[u8],
    ) -> Result<(), String> {
        if data.len() > PARTY_CHUNK_BYTES {
            return Err("chunk is too large".to_string());
        }
        let Some(pending) = self.uploads.get_mut(&upload) else {
            return Err("no such upload".to_string());
        };
        if pending.uploader != uploader {
            return Err("no such upload".to_string());
        }
        let would_be = pending.data.len() as u64 + data.len() as u64;
        if would_be > pending.declared {
            let name = pending.name.clone();
            self.uploads.remove(&upload);
            return Err(format!("{name} sent more data than it declared"));
        }
        pending.data.extend_from_slice(data);
        Ok(())
    }

    /// Complete an upload: store the assembled bytes and post the file message.
    /// Returns the envelope to deliver, and whether it was a DM.
    /// Complete an upload.
    ///
    /// The spool is dropped on **every** path out of here, success or failure.
    /// It used to survive the incomplete case, which was the leak: a member
    /// declaring 100 MiB, sending 99 and finishing got an error back and left
    /// the 99 MiB behind, with nothing that would ever reclaim it short of a
    /// restart. Finishing short is a client that broke its own protocol; the
    /// bytes are not worth keeping on the chance it comes back for them.
    pub fn finish_upload(
        &mut self,
        uploader: Uuid,
        upload: Uuid,
    ) -> Result<(Envelope, UploadTarget), String> {
        let mut staged = self.begin_upload(uploader, upload)?;
        if let Err(reason) = staged.write_staging() {
            self.discard_upload(staged);
            return Err(reason);
        }
        self.commit_upload(staged)
    }

    /// Phase 1 of an upload, under the lock: check it, claim the storage, and
    /// take the bytes.
    ///
    /// Everything that needs to consult state happens here — the upload exists,
    /// it is this member's, it is complete, they may still post where it is
    /// going, and there is room for it. What does *not* happen here is any I/O:
    /// the returned [`StagedUpload`] is written by the caller once it has let go
    /// of the lock, and handed back to [`Self::commit_upload`].
    pub fn begin_upload(&mut self, uploader: Uuid, upload: Uuid) -> Result<StagedUpload, String> {
        let Some(pending) = self.uploads.get(&upload) else {
            return Err("no such upload".to_string());
        };
        if pending.uploader != uploader {
            // Not this member's upload: say nothing about it and, crucially,
            // do not remove it — it belongs to someone else.
            return Err("no such upload".to_string());
        }
        if (pending.data.len() as u64) != pending.declared {
            let short = pending.declared - pending.data.len() as u64;
            self.uploads.remove(&upload);
            return Err(format!(
                "upload is incomplete — {} still missing",
                messenger_core::util::format_size(short)
            ));
        }

        // Check where it is going *before* taking the bytes, so a refusal leaves
        // the spool exactly as it was.
        let target = pending.target;
        let size = pending.data.len() as u64;
        self.check_may_post_file(uploader, target)?;
        self.check_storage_room(uploader, size)?;

        let pending = self.uploads.remove(&upload).expect("checked above");
        // No pre-computed hash: hashing up to `MAX_PARTY_FILE_BYTES` here would
        // put exactly the work this split exists to move back under the lock.
        // The reservation is therefore pessimistic — see `stage`.
        self.stage(
            uploader,
            target,
            pending.name,
            pending.mime,
            pending.data,
            None,
        )
    }

    /// Phase 1 for an **inline** upload — one that arrived whole in a single
    /// request rather than in chunks.
    ///
    /// Inline uploads are capped at `MAX_INLINE_FILE_BYTES` (4 MiB), so writing
    /// one under the lock is 25x cheaper than a chunked upload's 100 MiB. It is
    /// still not free, and more to the point a rule with an exception in it is a
    /// rule that gets forgotten: "no blob write happens under the state lock" is
    /// worth being true without qualification. Both paths therefore stage,
    /// commit and reclaim identically — one fewer shape to reason about, and one
    /// fewer place for the ceiling checks and the re-check to drift apart.
    pub fn begin_inline_upload(
        &mut self,
        uploader: Uuid,
        target: UploadTarget,
        name: String,
        mime: String,
        data: Vec<u8>,
    ) -> Result<StagedUpload, String> {
        let data = Self::check_inline_size(data)?;
        self.check_may_post_file(uploader, target)?;
        // Inline uploads are capped at 4 MiB and the bytes are already in hand,
        // so hashing here costs single-digit milliseconds — cheap enough to buy
        // an exact answer to "do we already hold this?", which is what keeps a
        // re-upload of stored content free at a full ceiling. The chunked path
        // cannot afford the same trick at 100 MiB; see `begin_upload`.
        let hash = blob_hash(&data);
        self.stage(uploader, target, name, mime, data, Some(hash))
    }

    /// Build a [`StagedUpload`], reserving storage for it unless the content is
    /// one the store already holds.
    ///
    /// `known_hash` is `Some` only when the caller could afford to compute it
    /// under the lock. When it is `None` the reservation is necessarily
    /// pessimistic: a duplicate reserves its size and gives it back at commit,
    /// so a re-upload of existing content can be refused at a full ceiling where
    /// it would previously have deduplicated. That is the price of not hashing
    /// 100 MiB with the lock held, and it is the right way round — the refusal
    /// is immediate and truthful, and `ShareFile` re-shares content the server
    /// already holds without going near this path at all.
    fn stage(
        &mut self,
        uploader: Uuid,
        target: UploadTarget,
        name: String,
        mime: String,
        data: Vec<u8>,
        known_hash: Option<String>,
    ) -> Result<StagedUpload, String> {
        let size = data.len() as u64;
        // Content already held costs no new storage, so it needs no reservation
        // and no ceiling check — the same rule commit applies once the hash is
        // known for certain.
        let already_held = known_hash
            .as_deref()
            .is_some_and(|h| self.blobs.contains_key(h));
        let id = Uuid::new_v4();
        if !already_held {
            self.check_storage_room(uploader, size)?;
            self.reservations.insert(
                id,
                Reservation {
                    member: uploader,
                    bytes: size,
                },
            );
        }
        Ok(StagedUpload {
            id,
            uploader,
            target,
            name,
            mime,
            data: Some(data),
            blob_dir: self.blob_dir.clone(),
            hash: known_hash,
            size,
            staging_path: None,
        })
    }

    /// Phase 3, under the lock: re-check, publish the bytes, post the message.
    ///
    /// The re-check is the point. Between phase 1 and here the lock was released
    /// for as long as a 100 MiB write takes, and in that window the uploader can
    /// have been demoted or removed, the channel deleted, or a DM recipient
    /// removed from the server. Checking only on the way in would append a
    /// message that the current state forbids.
    pub fn commit_upload(
        &mut self,
        mut staged: StagedUpload,
    ) -> Result<(Envelope, UploadTarget), String> {
        // Release the claim first, whatever happens next: from here on the bytes
        // are either recorded (and counted as stored) or discarded.
        self.reservations.remove(&staged.id);

        let hash = match staged.hash.clone() {
            Some(hash) => hash,
            None => return Err("upload was never written".to_string()),
        };
        let uploader = staged.uploader;
        let target = staged.target;

        self.check_may_post_file(uploader, target)?;

        // Content the server already holds costs nothing more, so the ceiling is
        // only re-checked for genuinely new content — and now against the real
        // hash rather than the pessimistic reservation.
        if !self.blobs.contains_key(&hash) {
            self.check_storage_room(uploader, staged.size)?;
        }

        let meta = self.publish_staged_blob(&mut staged, &hash)?;
        let envelope = match target {
            UploadTarget::Channel(channel) => self.append_file_message(uploader, channel, meta)?,
            UploadTarget::Dm(to) => self.append_file_dm(uploader, to, meta)?,
        };
        Ok((envelope, target))
    }

    /// Give up on a staged upload: release its claim and remove its file.
    ///
    /// `Drop` removes the file on its own, so this is about the reservation —
    /// but calling it explicitly is what keeps the two in step on the paths that
    /// know they are giving up.
    pub fn discard_upload(&mut self, staged: StagedUpload) {
        self.reservations.remove(&staged.id);
        drop(staged); // its `Drop` unlinks the staging file
    }

    /// Move a staged upload's bytes into the blob store under their content hash.
    ///
    /// A rename, not a copy: the bytes were already written and fsynced in phase
    /// 2, and renaming within one directory is atomic, so a blob is either
    /// absent or complete — never a half-written file that a download would
    /// serve as truncated.
    fn publish_staged_blob(
        &mut self,
        staged: &mut StagedUpload,
        hash: &str,
    ) -> Result<FileMeta, String> {
        let size = staged.size;
        let mime = staged.mime.clone();

        if let Some(rec) = self.blobs.get_mut(hash) {
            // Already held: this upload's copy is redundant. Take the staging
            // file back out (via `Drop`) and just count the new reference.
            rec.refcount += 1;
            let refcount = rec.refcount;
            self.persist_blob_refcount(hash, refcount);
        } else {
            match (self.blob_dir.as_ref(), staged.staging_path.take()) {
                (Some(dir), Some(path)) => {
                    let dest = dir.join(hash);
                    if let Err(e) = std::fs::rename(&path, &dest) {
                        tracing::error!(
                            error = %e,
                            from = %path.display(),
                            to = %dest.display(),
                            "failed to publish a staged blob"
                        );
                        // Put it back so `Drop` still cleans up after us.
                        staged.staging_path = Some(path);
                        return Err("the server could not store this file".to_string());
                    }
                }
                // Memory-only store: there is no file, the bytes are resident.
                (None, _) => {}
                (Some(_), None) => {
                    return Err("upload was never written".to_string());
                }
            }
            self.persist_blob_row(hash, size, &mime, 1);
            let resident = if self.blob_dir.is_some() {
                None
            } else {
                staged.data.take()
            };
            self.blobs.insert(
                hash.to_string(),
                BlobRecord {
                    size,
                    mime: mime.clone(),
                    data: resident,
                    refcount: 1,
                },
            );
        }

        Ok(FileMeta {
            hash: hash.to_string(),
            // The display name is member-supplied: reduce it to a safe filename
            // so no client ever receives a name that could escape its download
            // directory.
            name: sanitize_filename(&staged.name),
            size,
            mime,
        })
    }

    /// May this member put a file in this place, right now?
    ///
    /// One function so the check on the way in and the re-check on the way out
    /// cannot drift apart — which is how the DM recipient came to be checked
    /// only on the way in, letting a member removed from the server during a
    /// 100 MiB upload still receive the message at the end of it.
    fn check_may_post_file(&self, uploader: Uuid, target: UploadTarget) -> Result<(), String> {
        if !self.is_member(uploader) {
            return Err("sender is not a member of this server".to_string());
        }
        match target {
            UploadTarget::Channel(channel) => self.member_can_post_to_channel(uploader, channel),
            UploadTarget::Dm(to) => {
                if !self.role_of(uploader).can_write() {
                    return Err("your role on this server is read-only".to_string());
                }
                if !self.is_member(to) {
                    return Err("recipient is not a member of this server".to_string());
                }
                Ok(())
            }
        }
    }

    /// Is there room for `size` more bytes, server-wide and for this member?
    ///
    /// Counts reservations alongside stored blobs, so uploads still being
    /// written are budgeted for rather than discovered afterwards.
    fn check_storage_room(&self, uploader: Uuid, size: u64) -> Result<(), String> {
        let stored = self.total_committed_and_reserved_bytes();
        if stored.saturating_add(size) > self.max_blob_bytes {
            return Err("server file storage is full".to_string());
        }
        if let Some(limit) = self.member_blob_limit(uploader) {
            let used = self.member_blob_bytes(uploader);
            if used.saturating_add(size) > limit {
                return Err(format!(
                    "this would exceed your {} file storage allowance ({} of it already used)",
                    messenger_core::util::format_size(limit),
                    messenger_core::util::format_size(used)
                ));
            }
        }
        Ok(())
    }

    /// Bytes the store holds or has promised to hold.
    fn total_committed_and_reserved_bytes(&self) -> u64 {
        let stored: u64 = self.blobs.values().map(|r| r.size).sum();
        let reserved: u64 = self.reservations.values().map(|r| r.bytes).sum();
        stored.saturating_add(reserved)
    }

    /// Discard an in-flight upload. Called on an explicit cancel and on
    /// disconnect, so a client that vanishes mid-transfer does not pin its spool
    /// for the lifetime of the process.
    pub fn cancel_upload(&mut self, uploader: Uuid, upload: Uuid) {
        if self
            .uploads
            .get(&upload)
            .is_some_and(|p| p.uploader == uploader)
        {
            self.uploads.remove(&upload);
        }
    }

    /// Drop every upload belonging to `member` (their connection went away).
    ///
    /// Reservations go too. A connection torn down between phase 1 and phase 3
    /// leaves a claim nothing else would release — its `StagedUpload` cleans up
    /// its own file when dropped, but it cannot reach the state to give the
    /// storage back.
    pub fn cancel_uploads_for(&mut self, member: Uuid) {
        self.uploads.retain(|_, p| p.uploader != member);
        self.reservations.retain(|_, r| r.member != member);
    }

    /// How many uploads this member currently has spooled here.
    pub fn uploads_in_flight(&self, member: Uuid) -> usize {
        self.uploads
            .values()
            .filter(|p| p.uploader == member)
            .count()
    }

    /// Bytes currently held in upload spools, across every member.
    ///
    /// Not a limit — [`MAX_CONCURRENT_UPLOADS`] and [`MAX_PARTY_FILE_BYTES`]
    /// are what bound this — but the number a test needs in order to assert
    /// that an abandoned upload was actually reclaimed rather than merely
    /// forgotten by its connection.
    pub fn staged_upload_bytes(&self) -> u64 {
        self.uploads.values().map(|p| p.data.len() as u64).sum()
    }

    /// One chunk of a stored blob, plus the file's total size, for a member who
    /// is allowed to see it. `None` when unknown or not permitted — the same
    /// answer either way, so the endpoint never reveals a file's existence.
    ///
    /// Reads **under the lock**. The server's own request path does not use
    /// this: it goes through [`Self::plan_blob_read`] so the bytes move after
    /// the lock is released. Kept for in-process callers and tests, and
    /// delegating rather than duplicating so there is exactly one place that
    /// decides what a read is allowed to see.
    pub fn blob_chunk_for(&self, member: Uuid, hash: &str, offset: u64) -> Option<(Vec<u8>, u64)> {
        let plan = self.plan_blob_read(member, hash, Some(offset))?;
        let total = plan.total();
        plan.perform().map(|bytes| (bytes, total))
    }

    /// Validate an inline upload's size, returning the data on success.
    fn check_inline_size(data: Vec<u8>) -> Result<Vec<u8>, String> {
        if data.is_empty() {
            return Err("file is empty".to_string());
        }
        if data.len() > MAX_INLINE_FILE_BYTES {
            return Err(format!(
                "file exceeds the {} MiB inline limit",
                MAX_INLINE_FILE_BYTES / (1024 * 1024)
            ));
        }
        Ok(data)
    }

    /// Store a file and post it as a message to `channel`. Returns the assigned
    /// envelope (its payload is `File`) for broadcast, like [`Self::post_message`].
    pub fn post_file(
        &mut self,
        sender: Uuid,
        channel: Uuid,
        name: String,
        mime: String,
        data: Vec<u8>,
    ) -> Result<Envelope, String> {
        self.post_inline_file(sender, UploadTarget::Channel(channel), name, mime, data)
    }

    /// Run all three upload phases back to back, with no lock to release
    /// between them.
    ///
    /// The convenience form of the staged path, for in-process callers and
    /// tests. It writes under whatever lock the caller holds, which is why the
    /// server's request path does **not** use it — but it is the same three
    /// functions in the same order, so there is still exactly one place that
    /// decides what may be stored and exactly one that writes it. The previous
    /// inline path was a second implementation of both, and a second
    /// implementation is what quietly stops matching.
    fn post_inline_file(
        &mut self,
        sender: Uuid,
        target: UploadTarget,
        name: String,
        mime: String,
        data: Vec<u8>,
    ) -> Result<Envelope, String> {
        let mut staged = self.begin_inline_upload(sender, target, name, mime, data)?;
        if let Err(reason) = staged.write_staging() {
            self.discard_upload(staged);
            return Err(reason);
        }
        self.commit_upload(staged).map(|(env, _)| env)
    }

    /// Record a file reference and append the file message to a channel.
    ///
    /// Called by `commit_upload` for both the inline and the chunked path,
    /// so every file message is appended in exactly one way.
    fn append_file_message(
        &mut self,
        sender: Uuid,
        channel: Uuid,
        meta: FileMeta,
    ) -> Result<Envelope, String> {
        let tier = self.tier;
        self.record_file_ref(sender, &meta, channel, false);
        let chan = self
            .channel_mut(channel)
            .ok_or_else(|| "channel no longer exists".to_string())?;
        let seq = chan.messages.len() as u64 + 1;
        let envelope = Envelope {
            tier,
            sender,
            channel,
            seq,
            timestamp: current_timestamp_millis(),
            payload: MessagePayload::File(meta),
        };
        chan.messages.push(envelope.clone());
        self.persist_message(&envelope);
        Ok(envelope)
    }

    /// Store a file and send it as a direct message to `to`. Returns the assigned
    /// envelope (payload `File`) for delivery, like [`Self::post_dm`].
    pub fn post_file_dm(
        &mut self,
        from: Uuid,
        to: Uuid,
        name: String,
        mime: String,
        data: Vec<u8>,
    ) -> Result<Envelope, String> {
        self.post_inline_file(from, UploadTarget::Dm(to), name, mime, data)
    }

    /// Record a file reference and append the file message to a DM thread.
    /// The DM twin of [`Self::append_file_message`].
    fn append_file_dm(&mut self, from: Uuid, to: Uuid, meta: FileMeta) -> Result<Envelope, String> {
        let thread_id = messenger_core::party::dm_thread_id(from, to);
        let tier = self.tier;
        self.record_file_ref(from, &meta, thread_id, true);
        let thread = self
            .dm_threads
            .entry(thread_id)
            .or_insert_with(|| DmThread {
                id: thread_id,
                messages: Vec::new(),
            });
        let seq = thread.messages.len() as u64 + 1;
        let envelope = Envelope {
            tier,
            sender: from,
            channel: thread_id,
            seq,
            timestamp: current_timestamp_millis(),
            payload: MessagePayload::File(meta),
        };
        thread.messages.push(envelope.clone());
        self.persist_dm(thread_id, &envelope);
        Ok(envelope)
    }

    /// The bytes of a stored blob by content hash, or `None` if unknown.
    ///
    /// When the store is disk-backed the bytes are read on demand rather than
    /// kept resident: holding every blob in memory made the storage ceiling
    /// (1 GiB by default) a *memory* ceiling too, which is how a small VPS gets
    /// OOM-killed by a community that is merely using the feature.
    pub fn blob_bytes(&self, hash: &str) -> Option<Vec<u8>> {
        let record = self.blobs.get(hash)?;
        if let Some(resident) = &record.data {
            return Some(resident.clone());
        }
        let dir = self.blob_dir.as_ref()?;
        match blocking_io(|| std::fs::read(dir.join(hash))) {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                tracing::error!(hash, error = %e, "blob is recorded but its file could not be read");
                None
            }
        }
    }

    /// Whether `member` may download the blob `hash`. A file is accessible only if
    /// it is referenced by a message in a channel the member may **read** or in a
    /// DM thread the member is a party to. Without this, any joined member who
    /// learned a content hash could fetch a file shared privately in a DM between
    /// two *other* members — content-addressed storage is shared, but the
    /// download endpoint must still enforce who can see what.
    /// Decided over the reference table rather than by re-scanning every message
    /// in every channel: the references *are* the record of where a file was
    /// shared, and a deleted reference has to stop granting access immediately.
    fn member_can_access_blob(&self, member: Uuid, hash: &str) -> bool {
        // Any *one* share that grants download is enough: the same bytes can sit
        // in several places under different grants, and being allowed to fetch
        // them anywhere means being allowed to fetch them.
        self.file_refs
            .iter()
            .filter(|r| r.hash == hash)
            .any(|r| self.effective_perms(member, r).download)
    }

    /// The bytes of a stored blob, but only when `member` is permitted to see it.
    /// Returns `None` both when the blob is unknown and when access is denied, so
    /// the endpoint never reveals the existence of a file the member can't access.
    ///
    /// Reads **under the lock**, like [`Self::blob_chunk_for`], and for the same
    /// reason is not what the server's request path uses. Delegates to
    /// [`Self::plan_blob_read`] so both share one access decision.
    pub fn blob_bytes_for(&self, member: Uuid, hash: &str) -> Option<Vec<u8>> {
        self.plan_blob_read(member, hash, None)?.perform()
    }

    /// Decide a download *without performing it*.
    ///
    /// Access control is a question about state, so it is answered here, under
    /// the lock. Moving the bytes is not: a whole-blob read is up to
    /// `MAX_PARTY_FILE_BYTES` (100 MiB) of disk I/O, and doing it here means
    /// every other member's message waits behind one member's download. The
    /// caller performs the plan once it has let go of the lock — see
    /// `BlobReadPlan::perform`.
    ///
    /// Authorisation is therefore point-in-time, which was already the contract
    /// (see `member_can_access_blob`) and is now a wider window: a member whose
    /// access is revoked mid-read still receives the rest of what they had
    /// already been cleared for. Content addressing is what makes that safe to
    /// state — the path named here always holds the same bytes, so a late read
    /// cannot return something *else*. Revocation takes effect at the next
    /// request.
    ///
    /// `offset` selects a chunked read (`PARTY_CHUNK_BYTES` from there);
    /// `None` asks for the whole blob.
    pub fn plan_blob_read(
        &self,
        member: Uuid,
        hash: &str,
        offset: Option<u64>,
    ) -> Option<BlobReadPlan> {
        if !self.member_can_access_blob(member, hash) {
            return None;
        }
        let record = self.blobs.get(hash)?;
        let total = record.size;

        // Memory-only store (no blob directory): the bytes are already here, so
        // there is nothing to defer. Tests run this way.
        if let Some(resident) = &record.data {
            let total = resident.len() as u64;
            let bytes = match offset {
                None => resident.clone(),
                Some(at) if at >= total => Vec::new(),
                Some(at) => {
                    let start = at as usize;
                    let end = (start + PARTY_CHUNK_BYTES).min(resident.len());
                    resident[start..end].to_vec()
                }
            };
            return Some(BlobReadPlan::Resident { bytes, total });
        }

        let dir = self.blob_dir.as_ref()?;
        Some(BlobReadPlan::OnDisk {
            path: dir.join(hash),
            offset: offset.unwrap_or(0),
            // A whole-blob read is bounded by the blob's own recorded size; a
            // chunked one by the chunk size. Either way the length is decided
            // here, so the read cannot be talked into being larger than the
            // file the record describes.
            len: match offset {
                None => total,
                Some(at) => (total.saturating_sub(at)).min(PARTY_CHUNK_BYTES as u64),
            },
            total,
        })
    }

    // --- Durable mirroring (best-effort: failures are logged, not propagated, so a
    // transient disk error never drops a live request) --------------------------

    fn persist_member(&self, m: &Member) {
        let Some(conn) = &self.db else { return };
        if let Err(e) = insert_member_row(conn, m.id, &m.username, m.fingerprint.as_deref(), m.role)
        {
            tracing::error!(error = %e, "failed to persist party member");
        }
    }

    fn persist_file_ref(&self, r: &FileRef) {
        let Some(conn) = &self.db else { return };
        if let Err(e) = conn.execute(
            "INSERT OR REPLACE INTO file_refs
                (id, hash, name, uploader, location, is_dm, shared_at, default_perms, grants)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                r.id.to_string(),
                r.hash,
                r.name,
                r.uploader.to_string(),
                r.location.to_string(),
                r.is_dm as i64,
                r.shared_at as i64,
                serde_json::to_string(&r.default_perms).expect("FilePermissions serializes"),
                serde_json::to_string(
                    &r.grants
                        .iter()
                        .map(|(k, v)| (k.to_string(), *v))
                        .collect::<HashMap<String, FilePermissions>>()
                )
                .expect("grants serialize")
            ],
        ) {
            tracing::error!(error = %e, "failed to persist file reference");
        }
    }

    /// Delete exactly one share. Deleting by `(hash, location)` would drop every
    /// share of that content in that place while only one refcount was released.
    fn delete_file_ref_row(&self, id: Uuid) {
        let Some(conn) = &self.db else { return };
        if let Err(e) = conn.execute(
            "DELETE FROM file_refs WHERE id = ?1",
            params![id.to_string()],
        ) {
            tracing::error!(error = %e, "failed to delete file reference");
        }
    }

    fn delete_blob_row(&self, hash: &str) {
        let Some(conn) = &self.db else { return };
        if let Err(e) = conn.execute("DELETE FROM blobs WHERE hash = ?1", params![hash]) {
            tracing::error!(error = %e, "failed to delete blob row");
        }
    }

    fn delete_channel_rows(&self, channel: Uuid) {
        let Some(conn) = &self.db else { return };
        let id = channel.to_string();
        for (sql, what) in [
            ("DELETE FROM messages WHERE channel_id = ?1", "messages"),
            ("DELETE FROM file_refs WHERE location = ?1", "file refs"),
            ("DELETE FROM channels WHERE id = ?1", "channel"),
        ] {
            if let Err(e) = conn.execute(sql, params![id]) {
                tracing::error!(error = %e, what, "failed to delete channel rows");
            }
        }
    }

    fn persist_audit(&self, r: &AuditRecord) {
        let Some(conn) = &self.db else { return };
        if let Err(e) = conn.execute(
            "INSERT INTO audit (at, actor, action, detail) VALUES (?1, ?2, ?3, ?4)",
            params![r.at as i64, r.actor.to_string(), r.action, r.detail],
        ) {
            tracing::error!(error = %e, "failed to persist audit entry");
        }
    }

    fn persist_channel(&self, c: &Channel, position: usize) {
        let Some(conn) = &self.db else { return };
        if let Err(e) = insert_channel_row(conn, c.id, &c.name, c.kind, &c.members, position) {
            tracing::error!(error = %e, "failed to persist party channel");
        }
    }

    fn persist_message(&self, e: &Envelope) {
        let Some(conn) = &self.db else { return };
        if let Err(e) = insert_message_row(conn, e) {
            tracing::error!(error = %e, "failed to persist party message");
        }
    }

    fn persist_dm(&self, thread_id: Uuid, e: &Envelope) {
        let Some(conn) = &self.db else { return };
        if let Err(e) = insert_dm_message_row(conn, thread_id, e) {
            tracing::error!(error = %e, "failed to persist party direct message");
        }
    }

    fn persist_blob_row(&self, hash: &str, size: u64, mime: &str, refcount: u32) {
        let Some(conn) = &self.db else { return };
        if let Err(e) = insert_blob_row(conn, hash, size, mime, refcount) {
            tracing::error!(error = %e, "failed to persist file blob row");
        }
    }

    fn persist_blob_refcount(&self, hash: &str, refcount: u32) {
        let Some(conn) = &self.db else { return };
        if let Err(e) = conn.execute(
            "UPDATE blobs SET refcount = ?1 WHERE hash = ?2",
            params![refcount, hash],
        ) {
            tracing::error!(error = %e, "failed to update file blob refcount");
        }
    }

    /// Rebuild the in-memory model from the database. Presence resets to offline.
    fn reload_from_db(&mut self) -> anyhow::Result<()> {
        let conn = self
            .db
            .as_ref()
            .expect("reload_from_db requires an open database");

        let mut members = HashMap::new();
        {
            let mut stmt = conn.prepare("SELECT id, username, fingerprint, role FROM members")?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?;
            for row in rows {
                let (id, username, fingerprint, role) = row?;
                let id = Uuid::parse_str(&id)?;
                members.insert(
                    id,
                    Member {
                        id,
                        username,
                        fingerprint,
                        online: false,
                        // A role that will not parse falls back to the least
                        // privileged value rather than failing the load: a
                        // damaged row must not hand out authority.
                        role: serde_json::from_str(&role).unwrap_or(Role::Guest),
                    },
                );
            }
        }

        let mut channels = Vec::new();
        {
            let mut stmt =
                conn.prepare("SELECT id, name, kind, members FROM channels ORDER BY position ASC")?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?;
            let raw: Vec<(String, String, String, String)> = rows.collect::<Result<_, _>>()?;
            for (id, name, kind, members) in raw {
                let id = Uuid::parse_str(&id)?;
                let kind: ChannelKind = serde_json::from_str(&kind)?;
                let members: Vec<Uuid> = serde_json::from_str::<Vec<String>>(&members)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|s| Uuid::parse_str(s).ok())
                    .collect();
                let messages = load_messages(conn, "messages", "channel_id", id)?;
                channels.push(Channel {
                    id,
                    name,
                    kind,
                    members,
                    messages,
                });
            }
        }

        let mut dm_threads = HashMap::new();
        {
            let mut stmt = conn.prepare("SELECT id FROM dm_threads")?;
            let ids: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<_, _>>()?;
            for id in ids {
                let id = Uuid::parse_str(&id)?;
                let messages = load_messages(conn, "dm_messages", "thread_id", id)?;
                dm_threads.insert(id, DmThread { id, messages });
            }
        }

        let mut blobs = HashMap::new();
        {
            let mut stmt = conn.prepare("SELECT hash, size, mime, refcount FROM blobs")?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })?;
            let raw: Vec<(String, i64, String, i64)> = rows.collect::<Result<_, _>>()?;
            let blob_dir = self
                .blob_dir
                .as_ref()
                .expect("reload_from_db requires a blob directory");
            for (hash, size, mime, refcount) in raw {
                // Presence check only: the bytes are read on demand by
                // `blob_bytes`, so a restart no longer pulls the entire blob
                // store into memory.
                if !blob_dir.join(&hash).is_file() {
                    tracing::error!(hash = %hash, "blob bytes missing on disk; skipping");
                    continue;
                }
                blobs.insert(
                    hash,
                    BlobRecord {
                        size: size as u64,
                        mime,
                        data: None,
                        refcount: refcount as u32,
                    },
                );
            }
        }

        let mut file_refs = Vec::new();
        {
            let mut stmt = conn.prepare(
                "SELECT id, hash, name, uploader, location, is_dm, shared_at,
                        default_perms, grants
                 FROM file_refs ORDER BY shared_at ASC",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, String>(8)?,
                ))
            })?;
            for row in rows {
                let (id, hash, name, uploader, location, is_dm, shared_at, default_perms, grants) =
                    row?;
                // A reference to a blob that is gone grants access to nothing and
                // would only inflate the owner's quota, so drop it.
                if !blobs.contains_key(&hash) {
                    continue;
                }
                file_refs.push(FileRef {
                    id: Uuid::parse_str(&id)?,
                    // A row that will not parse falls back to the default
                    // grant rather than failing the load: a damaged value
                    // must not hand out rights nobody granted.
                    default_perms: serde_json::from_str(&default_perms).unwrap_or_default(),
                    grants: serde_json::from_str::<HashMap<String, FilePermissions>>(&grants)
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|(k, v)| Uuid::parse_str(&k).ok().map(|id| (id, v)))
                        .collect(),
                    hash,
                    name,
                    uploader: Uuid::parse_str(&uploader)?,
                    location: Uuid::parse_str(&location)?,
                    is_dm: is_dm != 0,
                    shared_at: shared_at as u64,
                });
            }
        }

        let mut audit = Vec::new();
        {
            let mut stmt = conn
                .prepare("SELECT at, actor, action, detail FROM audit ORDER BY id DESC LIMIT ?1")?;
            let rows = stmt.query_map(params![MAX_AUDIT_ENTRIES as i64], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?;
            for row in rows {
                let (at, actor, action, detail) = row?;
                audit.push(AuditRecord {
                    at: at as u64,
                    actor: Uuid::parse_str(&actor)?,
                    action,
                    detail,
                });
            }
            // Read newest-first for the LIMIT; stored oldest-first.
            audit.reverse();
        }

        self.members = members;
        self.channels = channels;
        self.dm_threads = dm_threads;
        self.blobs = blobs;
        self.file_refs = file_refs;
        self.audit = audit;
        Ok(())
    }
}

// --- Free-standing SQLite helpers ----------------------------------------------

/// Create the schema if it does not yet exist.
fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS members (
             id          TEXT PRIMARY KEY,
             username    TEXT NOT NULL,
             fingerprint TEXT
         );
         CREATE TABLE IF NOT EXISTS channels (
             id       TEXT PRIMARY KEY,
             name     TEXT NOT NULL,
             kind     TEXT NOT NULL,
             position INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS messages (
             channel_id TEXT NOT NULL,
             seq        INTEGER NOT NULL,
             envelope   TEXT NOT NULL,
             PRIMARY KEY (channel_id, seq)
         );
         CREATE TABLE IF NOT EXISTS dm_threads (
             id TEXT PRIMARY KEY
         );
         CREATE TABLE IF NOT EXISTS dm_messages (
             thread_id TEXT NOT NULL,
             seq       INTEGER NOT NULL,
             envelope  TEXT NOT NULL,
             PRIMARY KEY (thread_id, seq)
         );
         CREATE TABLE IF NOT EXISTS blobs (
             hash     TEXT PRIMARY KEY,
             size     INTEGER NOT NULL,
             mime     TEXT NOT NULL,
             refcount INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS file_refs (
             id            TEXT PRIMARY KEY,
             hash          TEXT NOT NULL,
             name          TEXT NOT NULL,
             uploader      TEXT NOT NULL,
             location      TEXT NOT NULL,
             is_dm         INTEGER NOT NULL,
             shared_at     INTEGER NOT NULL,
             -- No DEFAULT needed here: every insert writes both. The ALTER
             -- below carries one, because that is what existing rows need.
             default_perms TEXT NOT NULL,
             grants        TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS audit (
             id     INTEGER PRIMARY KEY AUTOINCREMENT,
             at     INTEGER NOT NULL,
             actor  TEXT NOT NULL,
             action TEXT NOT NULL,
             detail TEXT NOT NULL
         );",
    )?;
    // Added after the first release: `ALTER TABLE` is how an existing party.db
    // gains them, and "duplicate column" is the expected answer on a database
    // that already has them.
    for sql in [
        "ALTER TABLE members ADD COLUMN role TEXT NOT NULL DEFAULT '\"Member\"'",
        "ALTER TABLE channels ADD COLUMN members TEXT NOT NULL DEFAULT '[]'",
        // `file_refs` was first keyed by (hash, location), which collapsed
        // repeated shares of the same content. Give existing rows a per-share
        // id; `randomblob` is evaluated per row, so each gets a distinct one,
        // and 32 undashed hex characters parse as a UUID.
        "ALTER TABLE file_refs ADD COLUMN id TEXT",
        // Per-file permissions. Existing shares keep the previous behaviour,
        // which is exactly `FilePermissions::default()` — visible and
        // downloadable to whoever can reach the location, nothing more.
        "ALTER TABLE file_refs ADD COLUMN default_perms TEXT NOT NULL          DEFAULT '{\"view\":true,\"download\":true,\"delete\":false,\"share\":false}'",
        "ALTER TABLE file_refs ADD COLUMN grants TEXT NOT NULL DEFAULT '{}'",
    ] {
        if let Err(e) = conn.execute(sql, []) {
            let msg = e.to_string();
            if !msg.contains("duplicate column name") {
                return Err(e);
            }
        }
    }
    conn.execute(
        "UPDATE file_refs SET id = lower(hex(randomblob(16))) WHERE id IS NULL OR id = ''",
        [],
    )?;
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS file_refs_id ON file_refs (id)",
        [],
    )?;
    Ok(())
}

fn insert_blob_row(
    conn: &Connection,
    hash: &str,
    size: u64,
    mime: &str,
    refcount: u32,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO blobs (hash, size, mime, refcount) VALUES (?1, ?2, ?3, ?4)",
        params![hash, size as i64, mime, refcount],
    )?;
    Ok(())
}

/// True when no members, channels, or DM threads exist yet (a pristine database).
fn db_is_empty(conn: &Connection) -> rusqlite::Result<bool> {
    Ok(table_count(conn, CountedTable::Members)? == 0
        && table_count(conn, CountedTable::Channels)? == 0
        && table_count(conn, CountedTable::DmThreads)? == 0)
}

/// The tables `table_count` will count. SQLite cannot bind an identifier as a
/// parameter, so the name has to be interpolated — which means the only real
/// defence is that the set of possible names is closed.
///
/// It was a `&str` with a comment promising callers would only pass literals. A
/// comment is not a defence: the day someone counts rows for a caller-supplied
/// name, the promise is silently broken and the compiler says nothing. As an
/// enum the query cannot be given a name this module did not write.
#[derive(Debug, Clone, Copy)]
enum CountedTable {
    Members,
    Channels,
    DmThreads,
}

impl CountedTable {
    /// The literal that goes into the SQL. Every arm is a fixed string.
    fn as_sql(self) -> &'static str {
        match self {
            CountedTable::Members => "members",
            CountedTable::Channels => "channels",
            CountedTable::DmThreads => "dm_threads",
        }
    }
}

fn table_count(conn: &Connection, table: CountedTable) -> rusqlite::Result<i64> {
    conn.query_row(
        &format!("SELECT COUNT(*) FROM {}", table.as_sql()),
        [],
        |r| r.get(0),
    )
}

impl PartyState {
    fn count_channels(&self) -> rusqlite::Result<i64> {
        match &self.db {
            Some(conn) => table_count(conn, CountedTable::Channels),
            None => Ok(0),
        }
    }
}

fn insert_member_row(
    conn: &Connection,
    id: Uuid,
    username: &str,
    fingerprint: Option<&str>,
    role: Role,
) -> rusqlite::Result<()> {
    let role = serde_json::to_string(&role).expect("Role serializes");
    conn.execute(
        "INSERT OR REPLACE INTO members (id, username, fingerprint, role) VALUES (?1, ?2, ?3, ?4)",
        params![id.to_string(), username, fingerprint, role],
    )?;
    Ok(())
}

fn insert_channel_row(
    conn: &Connection,
    id: Uuid,
    name: &str,
    kind: ChannelKind,
    members: &[Uuid],
    position: usize,
) -> rusqlite::Result<()> {
    let kind = serde_json::to_string(&kind).expect("ChannelKind serializes");
    let members = serde_json::to_string(&members.iter().map(|m| m.to_string()).collect::<Vec<_>>())
        .expect("member list serializes");
    conn.execute(
        "INSERT OR REPLACE INTO channels (id, name, kind, members, position)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id.to_string(), name, kind, members, position as i64],
    )?;
    Ok(())
}

fn insert_message_row(conn: &Connection, e: &Envelope) -> rusqlite::Result<()> {
    let json = serde_json::to_string(e).expect("Envelope serializes");
    conn.execute(
        "INSERT OR REPLACE INTO messages (channel_id, seq, envelope) VALUES (?1, ?2, ?3)",
        params![e.channel.to_string(), e.seq as i64, json],
    )?;
    Ok(())
}

fn insert_dm_message_row(conn: &Connection, thread_id: Uuid, e: &Envelope) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO dm_threads (id) VALUES (?1)",
        params![thread_id.to_string()],
    )?;
    let json = serde_json::to_string(e).expect("Envelope serializes");
    conn.execute(
        "INSERT OR REPLACE INTO dm_messages (thread_id, seq, envelope) VALUES (?1, ?2, ?3)",
        params![thread_id.to_string(), e.seq as i64, json],
    )?;
    Ok(())
}

/// Load and deserialize an ordered envelope history from `table` where the keying
/// column `key_col` equals `key`. Both `table` and `key_col` are module constants.
/// Remove staging files a previous run left behind. Best effort: a file that
/// cannot be removed is logged and skipped, because failing a server start over
/// one orphaned temporary is a worse outcome than leaving it there.
fn sweep_staging_files(blob_dir: &Path) {
    let entries = match std::fs::read_dir(blob_dir) {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(error = %e, dir = %blob_dir.display(), "could not scan for staging files");
            return;
        }
    };
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(STAGING_PREFIX) {
            continue;
        }
        match std::fs::remove_file(entry.path()) {
            Ok(()) => removed += 1,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    path = %entry.path().display(),
                    "could not remove an orphaned staging file"
                );
            }
        }
    }
    if removed > 0 {
        tracing::info!(
            count = removed,
            "removed staging files left by a previous run"
        );
    }
}

fn load_messages(
    conn: &Connection,
    table: &str,
    key_col: &str,
    key: Uuid,
) -> anyhow::Result<Vec<Envelope>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT envelope FROM {table} WHERE {key_col} = ?1 ORDER BY seq ASC"
    ))?;
    let rows: Vec<String> = stmt
        .query_map(params![key.to_string()], |r| r.get::<_, String>(0))?
        .collect::<Result<_, _>>()?;
    let mut out = Vec::with_capacity(rows.len());
    for json in rows {
        out.push(serde_json::from_str(&json)?);
    }
    Ok(out)
}

/// One-time import of a legacy JSON snapshot into the (empty) database, in a single
/// transaction so it is all-or-nothing.
fn import_legacy_snapshot(conn: &Connection, path: &Path) -> anyhow::Result<()> {
    let json = std::fs::read_to_string(path)?;
    let snap: LegacySnapshot = serde_json::from_str(&json)?;

    let tx = conn.unchecked_transaction()?;
    for m in &snap.members {
        insert_member_row(
            &tx,
            m.id,
            &m.username,
            m.fingerprint.as_deref(),
            Role::Member,
        )?;
    }
    for (position, c) in snap.channels.iter().enumerate() {
        insert_channel_row(&tx, c.id, &c.name, c.kind, &[], position)?;
        for e in &c.messages {
            insert_message_row(&tx, e)?;
        }
    }
    for t in &snap.dm_threads {
        for e in &t.messages {
            insert_dm_message_row(&tx, t.id, e)?;
        }
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_server_join_succeeds_and_lists_member() {
        let mut state = PartyState::new("Open", None);
        let id = state.join("alice", None, None).expect("join");
        assert!(state.is_member(id));
        let members = state.members();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].username, "alice");
        assert!(members[0].online);
    }

    /// The comparison must accept exactly the right password and nothing else —
    /// including the cases the `Option` equality it replaced got right, so
    /// switching to a constant-time compare changed only the timing.
    #[test]
    fn password_matches_covers_every_combination() {
        assert!(
            password_matches(None, None),
            "open server, no password given"
        );
        assert!(
            !password_matches(None, Some("anything")),
            "an open server must not accept a password as if it were configured"
        );
        assert!(
            !password_matches(Some("s3cret"), None),
            "a protected server must not accept a missing password"
        );
        assert!(password_matches(Some("s3cret"), Some("s3cret")));
        assert!(!password_matches(Some("s3cret"), Some("s3crey")));
        assert!(
            !password_matches(Some("s3cret"), Some("s3cre")),
            "a correct prefix is not a match"
        );
        assert!(
            !password_matches(Some("s3cret"), Some("s3cretx")),
            "a correct prefix plus extra is not a match"
        );
        assert!(password_matches(Some(""), Some("")));
    }

    #[test]
    fn password_protected_join_enforced() {
        let mut state = PartyState::new("Locked", Some("s3cret".to_string()));
        assert_eq!(
            state.join("alice", None, None),
            Err(JoinError::WrongPassword)
        );
        assert_eq!(
            state.join("alice", Some("nope"), None),
            Err(JoinError::WrongPassword)
        );
        assert!(state.join("alice", Some("s3cret"), None).is_ok());
    }

    #[test]
    fn duplicate_and_empty_usernames_rejected() {
        let mut state = PartyState::new("Open", None);
        state.join("alice", None, None).unwrap();
        assert_eq!(
            state.join("Alice", None, None),
            Err(JoinError::UsernameTaken),
            "username uniqueness is case-insensitive"
        );
        assert_eq!(state.join("   ", None, None), Err(JoinError::EmptyUsername));
    }

    #[test]
    fn oversized_username_rejected_at_the_boundary() {
        let mut state = PartyState::new("Open", None);
        // Exactly at the cap is accepted (counted in Unicode scalar values, so the
        // trailing surrounding whitespace is trimmed first).
        let at_cap = "a".repeat(MAX_USERNAME_CHARS);
        assert!(
            state.join(&at_cap, None, None).is_ok(),
            "{MAX_USERNAME_CHARS} chars is allowed"
        );

        // One past the cap is rejected before the member is ever stored.
        let too_long = "b".repeat(MAX_USERNAME_CHARS + 1);
        assert_eq!(
            state.join(&too_long, None, None),
            Err(JoinError::UsernameTooLong)
        );
        assert_eq!(
            state.members().len(),
            1,
            "the oversized username is not registered"
        );
    }

    #[test]
    fn oversized_channel_name_rejected() {
        let mut state = PartyState::new("Open", None);
        let at_cap = "c".repeat(MAX_CHANNEL_NAME_CHARS);
        assert!(state.create_channel(&at_cap).is_ok());

        let too_long = "d".repeat(MAX_CHANNEL_NAME_CHARS + 1);
        let err = state.create_channel(&too_long).unwrap_err();
        assert!(err.contains("too long"), "unexpected error: {err}");
    }

    #[test]
    fn uploaded_file_names_are_sanitized_against_path_traversal() {
        let mut state = PartyState::new("Open", None);
        let alice = state.join("alice", None, None).unwrap();
        let chan = state.default_channel();

        // A member-chosen name must never be able to escape a client's download
        // directory when later joined onto it.
        let env = state
            .post_file(
                alice,
                chan,
                "..\\..\\Startup\\evil.exe".to_string(),
                "application/octet-stream".to_string(),
                b"payload".to_vec(),
            )
            .unwrap();
        match &env.payload {
            MessagePayload::File(f) => {
                assert!(
                    !f.name.contains("..") && !f.name.contains('\\') && !f.name.contains('/'),
                    "stored name must be traversal-safe, got {:?}",
                    f.name
                );
            }
            other => panic!("expected a File payload, got {other:?}"),
        }
    }

    #[test]
    fn oversized_message_text_rejected_in_channels_and_dms() {
        let mut state = PartyState::new("Open", None);
        let alice = state.join("alice", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let chan = state.default_channel();

        let too_long = "x".repeat(MAX_MESSAGE_TEXT_BYTES + 1);
        assert!(
            state.post_message(alice, chan, too_long.clone()).is_err(),
            "an oversized channel message must be rejected"
        );
        assert!(
            state.post_dm(alice, bob, too_long).is_err(),
            "an oversized DM must be rejected"
        );
        // Nothing oversized was persisted to the channel.
        assert!(state.history_since(alice, chan, 0).is_empty());

        // A message exactly at the cap is still accepted.
        let at_cap = "y".repeat(MAX_MESSAGE_TEXT_BYTES);
        assert!(state.post_message(alice, chan, at_cap).is_ok());
    }

    #[test]
    fn reconnecting_with_same_fingerprint_reuses_the_member() {
        let mut state = PartyState::new("Open", None);
        let id1 = state
            .join("alice", None, Some("FP-alice".to_string()))
            .unwrap();

        // Simulate a dropped connection.
        state.set_online(id1, false);

        // The same identity reconnects under the same username: it must reuse the
        // existing membership (same id, back online), not be locked out by its own
        // offline ghost entry as "username taken".
        let id2 = state
            .join("alice", None, Some("FP-alice".to_string()))
            .expect("returning member must be allowed to rejoin");
        assert_eq!(id1, id2, "a returning identity keeps its member id");
        assert_eq!(state.members().len(), 1, "no duplicate member is created");
        assert!(
            state.members().iter().find(|m| m.id == id1).unwrap().online,
            "the reused member is marked online again"
        );
    }

    #[test]
    fn a_different_identity_cannot_take_an_existing_username() {
        let mut state = PartyState::new("Open", None);
        state
            .join("alice", None, Some("FP-alice".to_string()))
            .unwrap();
        // A different fingerprint claiming the same username is still rejected
        // (prevents impersonation of an established member).
        assert_eq!(
            state.join("alice", None, Some("FP-mallory".to_string())),
            Err(JoinError::UsernameTaken)
        );
    }

    #[test]
    fn posting_assigns_monotonic_per_channel_seq() {
        let mut state = PartyState::new("Open", None);
        let alice = state.join("alice", None, None).unwrap();
        let chan = state.default_channel();

        let m1 = state
            .post_message(alice, chan, "first".to_string())
            .unwrap();
        let m2 = state
            .post_message(alice, chan, "second".to_string())
            .unwrap();
        assert_eq!(m1.seq, 1);
        assert_eq!(m2.seq, 2);
        assert_eq!(m2.sender, alice);
        assert_eq!(m2.payload, MessagePayload::Text("second".to_string()));
    }

    #[test]
    fn posting_rejects_non_members_and_unknown_channels() {
        let mut state = PartyState::new("Open", None);
        let stranger = Uuid::new_v4();
        let chan = state.default_channel();
        assert!(state.post_message(stranger, chan, "x".to_string()).is_err());

        let alice = state.join("alice", None, None).unwrap();
        assert!(state
            .post_message(alice, Uuid::new_v4(), "x".to_string())
            .is_err());
    }

    #[test]
    fn history_since_supports_offline_catch_up() {
        let mut state = PartyState::new("Open", None);
        let alice = state.join("alice", None, None).unwrap();
        let chan = state.default_channel();
        for i in 0..5 {
            state.post_message(alice, chan, format!("msg {i}")).unwrap();
        }

        // Whole channel.
        assert_eq!(state.history_since(alice, chan, 0).len(), 5);
        // A member who last saw seq=3 gets only 4 and 5.
        let missed = state.history_since(alice, chan, 3);
        assert_eq!(missed.len(), 2);
        assert_eq!(missed[0].seq, 4);
        assert_eq!(missed[1].seq, 5);
        // Caught up: nothing new.
        assert!(state.history_since(alice, chan, 5).is_empty());
        // Unknown channel.
        assert!(state.history_since(alice, Uuid::new_v4(), 0).is_empty());
    }

    #[test]
    fn presence_can_be_toggled() {
        let mut state = PartyState::new("Open", None);
        let alice = state.join("alice", None, None).unwrap();
        state.set_online(alice, false);
        assert!(!state.members()[0].online);
        state.set_online(alice, true);
        assert!(state.members()[0].online);
    }

    #[test]
    fn durable_state_persists_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let channel;
        {
            let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
            channel = state.default_channel();
            let alice = state
                .join("alice", None, Some("FINGERPRINT".to_string()))
                .unwrap();
            state
                .post_message(alice, channel, "persisted msg".to_string())
                .unwrap();
        } // dropped; everything was written through to the database on each mutation

        let reloaded = PartyState::load("Srv", None, dir.path()).unwrap();
        // Member directory survived; presence resets to offline after reload.
        let members = reloaded.members();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].username, "alice");
        assert!(!members[0].online, "presence must reset to offline on load");
        // Channel identity + history survived.
        assert_eq!(reloaded.default_channel(), channel);
        let member = reloaded.members()[0].id;
        let history = reloaded.history_since(member, channel, 0);
        assert_eq!(history.len(), 1);
        assert_eq!(
            history[0].payload,
            MessagePayload::Text("persisted msg".to_string())
        );
    }

    #[test]
    fn load_without_existing_snapshot_starts_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let state = PartyState::load("Srv", None, dir.path()).unwrap();
        assert!(state.members().is_empty());
        assert_eq!(state.channels().len(), 1); // default `general`
    }

    #[test]
    fn direct_messages_share_one_order_independent_thread() {
        let mut state = PartyState::new("Open", None);
        let alice = state.join("alice", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let thread = messenger_core::party::dm_thread_id(alice, bob);

        let m1 = state.post_dm(alice, bob, "hi bob".to_string()).unwrap();
        assert_eq!(m1.channel, thread);
        assert_eq!(m1.sender, alice);
        assert_eq!(m1.seq, 1);
        let m2 = state.post_dm(bob, alice, "hey alice".to_string()).unwrap();
        assert_eq!(m2.channel, thread);
        assert_eq!(m2.seq, 2);

        assert_eq!(state.dm_history(thread, 0).len(), 2);
        assert_eq!(state.dm_history(thread, 1).len(), 1);
        assert!(state.dm_history(Uuid::new_v4(), 0).is_empty());
    }

    #[test]
    fn create_channel_adds_unique_named_channels() {
        let mut state = PartyState::new("Open", None);
        assert_eq!(state.channels().len(), 1); // general
        let info = state.create_channel("random").unwrap();
        assert_eq!(info.name, "random");
        assert_eq!(state.channels().len(), 2);
        // Duplicate (case-insensitive) and empty names are rejected.
        assert!(state.create_channel("Random").is_err());
        assert!(state.create_channel("   ").is_err());
        assert_eq!(state.channels().len(), 2);
    }

    #[test]
    fn dm_rejects_non_members() {
        let mut state = PartyState::new("Open", None);
        let alice = state.join("alice", None, None).unwrap();
        let stranger = Uuid::new_v4();
        assert!(state.post_dm(alice, stranger, "x".to_string()).is_err());
        assert!(state.post_dm(stranger, alice, "x".to_string()).is_err());
    }

    #[test]
    fn dm_history_persists_across_reload() {
        let dir = tempfile::tempdir().unwrap();
        let thread;
        {
            let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
            let alice = s.join("alice", None, None).unwrap();
            let bob = s.join("bob", None, None).unwrap();
            thread = messenger_core::party::dm_thread_id(alice, bob);
            s.post_dm(alice, bob, "secret".to_string()).unwrap();
        }
        let s = PartyState::load("Srv", None, dir.path()).unwrap();
        assert_eq!(s.dm_history(thread, 0).len(), 1);
    }

    /// A blob whose bytes cannot be written must fail the upload outright.
    ///
    /// The failure this guards is silent and permanent: the write error used to
    /// be logged and swallowed, so the blob was recorded, the file message was
    /// posted and broadcast to the whole channel, and every download of it
    /// answered "unknown file" from then on — with the sender told it worked.
    #[test]
    fn an_unwritable_blob_store_refuses_the_upload_instead_of_posting_a_dead_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
        let alice = state.join("alice", None, None).unwrap();
        let channel = state.default_channel();

        // Replace the blob directory with a regular file, so writing any blob
        // path inside it fails at the filesystem level.
        let blob_dir = dir.path().join(BLOB_DIR);
        std::fs::remove_dir_all(&blob_dir).unwrap();
        std::fs::write(&blob_dir, b"not a directory").unwrap();

        let err = state
            .post_file(
                alice,
                channel,
                "report.pdf".to_string(),
                "application/pdf".to_string(),
                b"payload".to_vec(),
            )
            .expect_err("an upload whose bytes cannot be stored must fail");
        assert!(
            err.contains("could not store"),
            "expected a storage error, got: {err}"
        );

        // Nothing was recorded: no message in the channel, and no blob to serve.
        assert!(state.history_since(alice, channel, 0).is_empty());
        assert!(state.blob_bytes(&blob_hash(b"payload")).is_none());
    }

    #[test]
    fn created_channels_and_their_order_persist_across_reload() {
        let dir = tempfile::tempdir().unwrap();
        let (general, random_id);
        {
            let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
            general = s.default_channel();
            let alice = s.join("alice", None, None).unwrap();
            random_id = s.create_channel("random").unwrap().id;
            s.post_message(alice, random_id, "in random".to_string())
                .unwrap();
        }
        let s = PartyState::load("Srv", None, dir.path()).unwrap();
        let channels = s.channels();
        assert_eq!(channels.len(), 2);
        // Order is preserved: general first, then the created channel.
        assert_eq!(s.default_channel(), general);
        assert_eq!(channels[0].id, general);
        assert_eq!(channels[1].id, random_id);
        // History of the created channel survived.
        let member = s.members()[0].id;
        let history = s.history_since(member, random_id, 0);
        assert_eq!(history.len(), 1);
        assert_eq!(
            history[0].payload,
            MessagePayload::Text("in random".to_string())
        );
    }

    #[test]
    fn legacy_json_snapshot_is_imported_once() {
        let dir = tempfile::tempdir().unwrap();

        // Hand-write a legacy snapshot in the old on-disk shape.
        let member_id = Uuid::new_v4();
        let channel_id = Uuid::new_v4();
        let snapshot = serde_json::json!({
            "members": [
                { "id": member_id, "username": "legacy_user", "fingerprint": "FP" }
            ],
            "channels": [
                {
                    "id": channel_id,
                    "name": "general",
                    "kind": "Public",
                    "messages": [
                        {
                            "tier": "Administered",
                            "sender": member_id,
                            "channel": channel_id,
                            "seq": 1,
                            "timestamp": 1700000000000u64,
                            "payload": { "Text": "hello from the past" }
                        }
                    ]
                }
            ],
            "dm_threads": []
        });
        std::fs::write(
            dir.path().join("party_state.json"),
            serde_json::to_string(&snapshot).unwrap(),
        )
        .unwrap();

        // First load imports the snapshot into SQLite and renames the file.
        let state = PartyState::load("Srv", None, dir.path()).unwrap();
        assert_eq!(state.members().len(), 1);
        assert_eq!(state.members()[0].username, "legacy_user");
        assert_eq!(state.default_channel(), channel_id);
        let member = state.members()[0].id;
        let history = state.history_since(member, channel_id, 0);
        assert_eq!(history.len(), 1);
        assert_eq!(
            history[0].payload,
            MessagePayload::Text("hello from the past".to_string())
        );
        assert!(
            !dir.path().join("party_state.json").exists(),
            "legacy snapshot should be renamed after import"
        );
        assert!(dir.path().join("party_state.json.imported").exists());

        // A second load must not re-import (the database is now authoritative).
        drop(state);
        let reloaded = PartyState::load("Srv", None, dir.path()).unwrap();
        assert_eq!(reloaded.members().len(), 1);
        let member = reloaded.members()[0].id;
        assert_eq!(reloaded.history_since(member, channel_id, 0).len(), 1);
    }

    fn file_payload(env: &Envelope) -> &FileMeta {
        match &env.payload {
            MessagePayload::File(f) => f,
            other => panic!("expected a File payload, got {other:?}"),
        }
    }

    #[test]
    fn post_file_stores_blob_and_posts_a_file_message() {
        let mut state = PartyState::new("Open", None);
        let alice = state.join("alice", None, None).unwrap();
        let chan = state.default_channel();

        let env = state
            .post_file(
                alice,
                chan,
                "hi.txt".into(),
                "text/plain".into(),
                b"hello".to_vec(),
            )
            .expect("post_file");
        assert_eq!(env.seq, 1);
        let meta = file_payload(&env);
        assert_eq!(meta.name, "hi.txt");
        assert_eq!(meta.size, 5);
        assert_eq!(meta.hash, blob_hash(b"hello"));

        // The bytes are retrievable by hash, and the message is in channel history.
        assert_eq!(state.blob_bytes(&meta.hash), Some(b"hello".to_vec()));
        assert_eq!(state.history_since(alice, chan, 0).len(), 1);
    }

    #[test]
    fn identical_uploads_are_deduplicated_by_content() {
        let mut state = PartyState::new("Open", None);
        let alice = state.join("alice", None, None).unwrap();
        let chan = state.default_channel();

        let a = state
            .post_file(
                alice,
                chan,
                "a.bin".into(),
                "application/octet-stream".into(),
                b"same".to_vec(),
            )
            .unwrap();
        let b = state
            .post_file(
                alice,
                chan,
                "b.bin".into(),
                "application/octet-stream".into(),
                b"same".to_vec(),
            )
            .unwrap();

        // Two messages, two names, but one shared content-addressed blob.
        assert_eq!(file_payload(&a).hash, file_payload(&b).hash);
        assert_eq!(state.blobs.len(), 1);
        assert_eq!(state.blobs[&file_payload(&a).hash].refcount, 2);
        assert_eq!(state.history_since(alice, chan, 0).len(), 2);
    }

    #[test]
    fn oversized_and_empty_uploads_are_rejected() {
        let mut state = PartyState::new("Open", None);
        let alice = state.join("alice", None, None).unwrap();
        let chan = state.default_channel();

        assert!(state
            .post_file(alice, chan, "empty".into(), "text/plain".into(), Vec::new())
            .is_err());
        let too_big = vec![0u8; MAX_INLINE_FILE_BYTES + 1];
        assert!(state
            .post_file(
                alice,
                chan,
                "big".into(),
                "application/octet-stream".into(),
                too_big
            )
            .is_err());
        // Nothing was stored.
        assert!(state.blobs.is_empty());
        assert!(state.history_since(alice, chan, 0).is_empty());
    }

    #[test]
    fn post_file_rejects_non_members_and_unknown_channels() {
        let mut state = PartyState::new("Open", None);
        let stranger = Uuid::new_v4();
        let chan = state.default_channel();
        assert!(state
            .post_file(
                stranger,
                chan,
                "x".into(),
                "text/plain".into(),
                b"x".to_vec()
            )
            .is_err());

        let alice = state.join("alice", None, None).unwrap();
        assert!(state
            .post_file(
                alice,
                Uuid::new_v4(),
                "x".into(),
                "text/plain".into(),
                b"x".to_vec()
            )
            .is_err());
    }

    #[test]
    fn file_dm_is_stored_in_the_thread() {
        let mut state = PartyState::new("Open", None);
        let alice = state.join("alice", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let thread = messenger_core::party::dm_thread_id(alice, bob);

        let env = state
            .post_file_dm(
                alice,
                bob,
                "secret.txt".into(),
                "text/plain".into(),
                b"psst".to_vec(),
            )
            .expect("post_file_dm");
        assert_eq!(env.channel, thread);
        assert_eq!(file_payload(&env).name, "secret.txt");
        assert_eq!(state.dm_history(thread, 0).len(), 1);
        assert_eq!(
            state.blob_bytes(&file_payload(&env).hash),
            Some(b"psst".to_vec())
        );
    }

    #[test]
    fn a_new_blob_past_the_storage_ceiling_is_rejected() {
        let mut state = PartyState::new("Open", None);
        let alice = state.join("alice", None, None).unwrap();
        let chan = state.default_channel();
        state.set_max_blob_bytes(8);

        // First upload fits under the 8-byte ceiling.
        state
            .post_file(
                alice,
                chan,
                "a.bin".into(),
                "application/octet-stream".into(),
                vec![1u8; 6],
            )
            .expect("a blob under the ceiling is accepted");

        // A distinct second blob would push the total to 12 bytes: rejected,
        // stored nowhere, and no message is posted for it.
        let err = state
            .post_file(
                alice,
                chan,
                "b.bin".into(),
                "application/octet-stream".into(),
                vec![2u8; 6],
            )
            .expect_err("a blob past the ceiling must be rejected");
        assert!(err.contains("storage"), "error should say why: {err}");
        assert_eq!(state.blobs.len(), 1, "the rejected blob must not be stored");
        assert_eq!(
            state.history_since(alice, chan, 0).len(),
            1,
            "no message is posted for a rejected upload"
        );

        // Re-uploading already-stored content adds no bytes, so deduplicated
        // uploads still succeed at the ceiling.
        state
            .post_file(
                alice,
                chan,
                "a-again.bin".into(),
                "application/octet-stream".into(),
                vec![1u8; 6],
            )
            .expect("re-upload of stored content adds no bytes and stays allowed");
    }

    #[test]
    fn download_is_denied_for_a_dm_file_a_member_cannot_see() {
        let mut state = PartyState::new("Open", None);
        let alice = state.join("alice", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let mallory = state.join("mallory", None, None).unwrap();

        // Alice sends Bob a private file over a DM.
        let env = state
            .post_file_dm(
                alice,
                bob,
                "secret.pdf".into(),
                "application/pdf".into(),
                b"top secret".to_vec(),
            )
            .unwrap();
        let hash = file_payload(&env).hash.clone();

        // Both participants can download it...
        assert_eq!(
            state.blob_bytes_for(alice, &hash),
            Some(b"top secret".to_vec())
        );
        assert_eq!(
            state.blob_bytes_for(bob, &hash),
            Some(b"top secret".to_vec())
        );
        // ...but a third member cannot, even if they somehow learn the hash.
        assert_eq!(
            state.blob_bytes_for(mallory, &hash),
            None,
            "a non-participant must not be able to download a private DM file"
        );
    }

    #[test]
    fn download_of_a_channel_file_is_allowed_for_any_member() {
        let mut state = PartyState::new("Open", None);
        let alice = state.join("alice", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let chan = state.default_channel();

        let env = state
            .post_file(
                alice,
                chan,
                "pic.png".into(),
                "image/png".into(),
                b"pixels".to_vec(),
            )
            .unwrap();
        let hash = file_payload(&env).hash.clone();

        // A public-channel file is downloadable by any member, not just the poster.
        assert_eq!(state.blob_bytes_for(bob, &hash), Some(b"pixels".to_vec()));
        // An unknown hash is denied.
        assert_eq!(state.blob_bytes_for(bob, "deadbeef"), None);
    }

    #[test]
    fn blobs_and_file_messages_persist_across_reload() {
        let dir = tempfile::tempdir().unwrap();
        let (chan, hash);
        {
            let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
            chan = s.default_channel();
            let alice = s.join("alice", None, None).unwrap();
            let env = s
                .post_file(
                    alice,
                    chan,
                    "doc.txt".into(),
                    "text/plain".into(),
                    b"durable".to_vec(),
                )
                .unwrap();
            hash = file_payload(&env).hash.clone();
        }
        let s = PartyState::load("Srv", None, dir.path()).unwrap();
        // The blob bytes and the file message both survived.
        assert_eq!(s.blob_bytes(&hash), Some(b"durable".to_vec()));
        let member = s.members()[0].id;
        let history = s.history_since(member, chan, 0);
        assert_eq!(history.len(), 1);
        assert_eq!(file_payload(&history[0]).name, "doc.txt");
        assert_eq!(s.blobs[&hash].refcount, 1);
    }

    /// A disk-backed server must not hold every blob's bytes in memory: the
    /// storage ceiling would double as a resident-memory ceiling.
    #[test]
    fn disk_backed_blobs_are_not_held_in_memory() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
        let alice = s.join("alice", None, None).unwrap();
        let chan = s.default_channel();
        let env = s
            .post_file(
                alice,
                chan,
                "doc.txt".into(),
                "text/plain".into(),
                b"durable".to_vec(),
            )
            .unwrap();
        let hash = file_payload(&env).hash.clone();

        assert!(
            s.blobs[&hash].data.is_none(),
            "a disk-backed blob must not keep its bytes resident"
        );
        // …and is still served correctly, read back from disk on demand.
        assert_eq!(s.blob_bytes(&hash), Some(b"durable".to_vec()));
    }

    /// `ChannelKind` was stored, shipped to clients, and enforced nowhere; then
    /// it was made to fail closed, which left three of the four kinds unusable.
    /// Each kind now has a real rule, and an ordinary member is held to it.
    #[test]
    fn non_public_channels_are_enforced_rather_than_decorative() {
        let mut state = PartyState::new("Open", None);
        // The first member is the owner, so join a second one to test the
        // ordinary-member path.
        let _owner = state.join("owner", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let public = state.default_channel();

        for (kind, label) in [
            (ChannelKind::Private, "private"),
            (ChannelKind::Locked, "locked"),
            (ChannelKind::Announce, "announce"),
        ] {
            let id = Uuid::new_v4();
            state.channels.push(Channel {
                id,
                name: label.to_string(),
                kind,
                members: Vec::new(),
                messages: Vec::new(),
            });
            assert!(
                state.post_message(bob, id, "hi".to_string()).is_err(),
                "{label}: an ordinary member must not be able to post"
            );
            assert!(
                state
                    .post_file(bob, id, "f".into(), "text/plain".into(), b"x".to_vec())
                    .is_err(),
                "{label}: an ordinary member must not be able to upload"
            );
        }

        // A private channel they are not in is not readable, and says nothing
        // about whether it exists.
        let private = state
            .channels
            .iter()
            .find(|c| c.kind == ChannelKind::Private)
            .unwrap()
            .id;
        assert!(state.history_since(bob, private, 0).is_empty());
        assert!(
            !state.channels_for(bob).iter().any(|c| c.id == private),
            "a private channel must not even be listed to a non-member"
        );
        // The public channel is unaffected.
        assert!(state.post_message(bob, public, "hi".to_string()).is_ok());
        assert_eq!(state.history_since(bob, public, 0).len(), 1);
    }

    /// The kinds are restrictions on ordinary members, not on the people who
    /// administer the server — otherwise nobody could post an announcement to an
    /// announcement channel.
    #[test]
    fn admins_can_post_where_ordinary_members_cannot() {
        let mut state = PartyState::new("Open", None);
        let owner = state.join("owner", None, None).unwrap();
        assert_eq!(state.role_of(owner), Role::Owner);

        let announce = state
            .create_channel_of_kind(owner, "news", ChannelKind::Announce, vec![])
            .unwrap();
        assert!(state
            .post_message(owner, announce.id, "release day".to_string())
            .is_ok());

        let bob = state.join("bob", None, None).unwrap();
        assert_eq!(state.role_of(bob), Role::Member);
        assert!(state
            .post_message(bob, announce.id, "me too".to_string())
            .is_err());
        // But bob can still read it — that is the point of an announce channel.
        assert_eq!(state.history_since(bob, announce.id, 0).len(), 1);
    }

    /// A private channel is readable and postable by the members on its list.
    #[test]
    fn private_channel_membership_grants_access() {
        let mut state = PartyState::new("Open", None);
        let owner = state.join("owner", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let carol = state.join("carol", None, None).unwrap();

        let private = state
            .create_channel_of_kind(owner, "secret", ChannelKind::Private, vec![bob])
            .unwrap();
        // The creator is always added, so they cannot lock themselves out.
        assert!(private.members.contains(&owner));
        assert!(private.members.contains(&bob));

        assert!(state
            .post_message(bob, private.id, "in the club".to_string())
            .is_ok());
        assert_eq!(state.history_since(bob, private.id, 0).len(), 1);

        // Carol is not on the list: she cannot post, read, or even see it.
        assert!(state
            .post_message(carol, private.id, "let me in".to_string())
            .is_err());
        assert!(state.history_since(carol, private.id, 0).is_empty());
        assert!(!state.channels_for(carol).iter().any(|c| c.id == private.id));
    }

    /// Only admins may create the restricted kinds: each of them is a way to
    /// control what other members can see or say.
    #[test]
    fn ordinary_members_may_only_create_public_channels() {
        let mut state = PartyState::new("Open", None);
        let _owner = state.join("owner", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();

        assert!(state
            .create_channel_of_kind(bob, "chat", ChannelKind::Public, vec![])
            .is_ok());
        for kind in [
            ChannelKind::Private,
            ChannelKind::Locked,
            ChannelKind::Announce,
        ] {
            assert!(
                state
                    .create_channel_of_kind(bob, "nope", kind, vec![])
                    .is_err(),
                "{kind:?} is administrative"
            );
        }
    }

    /// A file posted in a channel the member cannot read must not be reachable
    /// by content hash either — the download endpoint is where access is
    /// enforced, because blob storage is shared across the whole server.
    #[test]
    fn blobs_in_unreadable_channels_are_not_downloadable() {
        let mut state = PartyState::new("Open", None);
        // The first member owns the server, and an admin can read every channel,
        // so the member under test has to be an ordinary one.
        let _owner = state.join("owner", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let chan = state.default_channel();
        let env = state
            .post_file(
                bob,
                chan,
                "secret.txt".into(),
                "text/plain".into(),
                b"classified".to_vec(),
            )
            .unwrap();
        let hash = file_payload(&env).hash.clone();
        assert!(state.blob_bytes_for(bob, &hash).is_some());

        // Flip the channel to Private with bob off the list: he can no longer
        // reach the file, even though he knows its hash and uploaded it.
        {
            let c = state.channel_mut(chan).unwrap();
            c.kind = ChannelKind::Private;
            c.members = Vec::new();
        }
        assert!(
            state.blob_bytes_for(bob, &hash).is_none(),
            "a file in an unreadable channel must not be downloadable by hash"
        );
    }

    /// The operator starts the server and then joins it, so the first identity
    /// through the door owns it. Without this bootstrap nobody could administer
    /// anything, because only an admin can appoint one.
    #[test]
    fn the_first_member_owns_the_server_and_the_rest_do_not() {
        let mut state = PartyState::new("Srv", None);
        let owner = state.join("owner", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        assert_eq!(state.role_of(owner), Role::Owner);
        assert_eq!(state.role_of(bob), Role::Member);
        assert_eq!(
            state.role_of(Uuid::new_v4()),
            Role::Guest,
            "unknown ids fail closed"
        );
    }

    /// An admin must not be able to mint a peer admin or unseat the operator —
    /// either would let them take the community from the person who runs it.
    #[test]
    fn role_changes_cannot_escalate_past_the_actor() {
        let mut state = PartyState::new("Srv", None);
        let owner = state.join("owner", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let carol = state.join("carol", None, None).unwrap();

        // The owner may appoint an admin.
        state.set_role(owner, bob, Role::Admin).unwrap();
        assert_eq!(state.role_of(bob), Role::Admin);

        // That admin may demote an ordinary member to guest…
        state.set_role(bob, carol, Role::Guest).unwrap();
        assert_eq!(state.role_of(carol), Role::Guest);
        // …but not appoint another admin, not take the owner's seat, and not
        // touch the owner.
        assert!(state.set_role(bob, carol, Role::Admin).is_err());
        assert!(state.set_role(bob, carol, Role::Owner).is_err());
        assert!(state.set_role(bob, owner, Role::Guest).is_err());
        assert_eq!(state.role_of(owner), Role::Owner);

        // An ordinary member may not change roles at all, including their own.
        assert!(state.set_role(carol, carol, Role::Admin).is_err());
        assert!(state.set_role(owner, owner, Role::Admin).is_err());
    }

    /// A guest is read-only everywhere, before any per-channel rule applies.
    #[test]
    fn a_guest_cannot_write_anywhere() {
        let mut state = PartyState::new("Srv", None);
        let owner = state.join("owner", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        state.set_role(owner, bob, Role::Guest).unwrap();
        let chan = state.default_channel();

        assert!(state.post_message(bob, chan, "hello".to_string()).is_err());
        assert!(state
            .post_file(bob, chan, "f".into(), "text/plain".into(), b"x".to_vec())
            .is_err());
        assert!(state
            .create_channel_of_kind(bob, "mine", ChannelKind::Public, vec![])
            .is_err());
        // Reading is still allowed.
        state
            .post_message(owner, chan, "welcome".to_string())
            .unwrap();
        assert_eq!(state.history_since(bob, chan, 0).len(), 1);
    }

    /// Reference counting only ever counted up: uploads incremented it and
    /// nothing decremented it, so deleting was impossible and storage only grew.
    #[test]
    fn deleting_the_last_reference_reclaims_the_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
        let owner = state.join("owner", None, None).unwrap();
        let a = state.default_channel();
        let b = state
            .create_channel_of_kind(owner, "second", ChannelKind::Public, vec![])
            .unwrap()
            .id;

        // The same content shared twice is one blob with two references.
        let env = state
            .post_file(
                owner,
                a,
                "d.txt".into(),
                "text/plain".into(),
                b"same".to_vec(),
            )
            .unwrap();
        let hash = file_payload(&env).hash.clone();
        state
            .post_file(
                owner,
                b,
                "d.txt".into(),
                "text/plain".into(),
                b"same".to_vec(),
            )
            .unwrap();
        assert_eq!(state.blobs[&hash].refcount, 2);
        assert_eq!(state.quota_for(owner).used, 4, "dedup: counted once");

        // Dropping one reference keeps the bytes for the other.
        state.delete_file(owner, &hash, a).unwrap();
        assert_eq!(state.blobs[&hash].refcount, 1);
        assert!(state.blob_bytes(&hash).is_some());

        // Dropping the last one reclaims them, on disk as well as in memory.
        state.delete_file(owner, &hash, b).unwrap();
        assert!(!state.blobs.contains_key(&hash));
        assert!(!dir.path().join(BLOB_DIR).join(&hash).exists());
        assert_eq!(state.quota_for(owner).used, 0);
    }

    /// Deleting is the uploader's right or an admin's, not everyone's.
    #[test]
    fn only_the_uploader_or_an_admin_may_delete_a_file() {
        let mut state = PartyState::new("Srv", None);
        let owner = state.join("owner", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let carol = state.join("carol", None, None).unwrap();
        let chan = state.default_channel();

        let env = state
            .post_file(
                bob,
                chan,
                "b.txt".into(),
                "text/plain".into(),
                b"bobs".to_vec(),
            )
            .unwrap();
        let hash = file_payload(&env).hash.clone();

        assert!(
            state.delete_file(carol, &hash, chan).is_err(),
            "a bystander"
        );
        assert!(state.delete_file(owner, &hash, chan).is_ok(), "an admin");

        // And the uploader can delete their own.
        let env = state
            .post_file(
                bob,
                chan,
                "c.txt".into(),
                "text/plain".into(),
                b"more".to_vec(),
            )
            .unwrap();
        let hash = file_payload(&env).hash.clone();
        assert!(state.delete_file(bob, &hash, chan).is_ok());
    }

    /// A deleted file stops being downloadable immediately, while the message
    /// that referenced it stays in history — removing the envelope would
    /// renumber the channel and desynchronise every client that had it.
    #[test]
    fn deleting_a_file_revokes_access_but_keeps_history_intact() {
        let mut state = PartyState::new("Srv", None);
        let owner = state.join("owner", None, None).unwrap();
        let chan = state.default_channel();
        state
            .post_message(owner, chan, "before".to_string())
            .unwrap();
        let env = state
            .post_file(
                owner,
                chan,
                "x.txt".into(),
                "text/plain".into(),
                b"bytes".to_vec(),
            )
            .unwrap();
        state
            .post_message(owner, chan, "after".to_string())
            .unwrap();
        let hash = file_payload(&env).hash.clone();

        state.delete_file(owner, &hash, chan).unwrap();
        assert!(state.blob_bytes_for(owner, &hash).is_none());

        let history = state.history_since(owner, chan, 0);
        assert_eq!(history.len(), 3, "history keeps its shape");
        assert_eq!(
            history.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![1, 2, 3],
            "and its sequence numbers"
        );
    }

    /// The server-wide ceiling alone lets the first member to reach it deny the
    /// feature to everyone else, so storage is budgeted per member as well.
    #[test]
    fn a_member_cannot_exceed_their_own_storage_allowance() {
        let mut state = PartyState::new("Srv", None);
        let owner = state.join("owner", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        state.set_max_member_blob_bytes(10);
        let chan = state.default_channel();

        assert!(state
            .post_file(
                bob,
                chan,
                "a".into(),
                "text/plain".into(),
                b"12345678".to_vec()
            )
            .is_ok());
        // Distinct content, so it is a new blob rather than a deduplicated
        // re-share, and it would take him past the ceiling.
        let err = state
            .post_file(
                bob,
                chan,
                "b".into(),
                "text/plain".into(),
                b"abcdefgh".to_vec(),
            )
            .expect_err("this would take bob past his allowance");
        assert!(err.contains("allowance"), "got: {err}");
        assert_eq!(state.quota_for(bob).used, 8);
        assert_eq!(state.quota_for(bob).limit, Some(10));

        // Admins are exempt: they are who clears space when it fills.
        assert!(state
            .post_file(
                owner,
                chan,
                "c".into(),
                "text/plain".into(),
                b"abcdefghij!".to_vec()
            )
            .is_ok());
        assert_eq!(state.quota_for(owner).limit, None);

        // Freeing a reference frees the allowance again.
        let hash = blob_hash(b"12345678");
        state.delete_file(bob, &hash, chan).unwrap();
        assert_eq!(state.quota_for(bob).used, 0);
        assert!(state
            .post_file(
                bob,
                chan,
                "d".into(),
                "text/plain".into(),
                b"abcdefgh".to_vec()
            )
            .is_ok());
    }

    /// The Drive listing shows a member their own files and the ones shared
    /// where they can see them — and nothing else.
    #[test]
    fn the_file_listing_respects_who_may_see_what() {
        let mut state = PartyState::new("Srv", None);
        let owner = state.join("owner", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let public = state.default_channel();

        state
            .post_file(
                bob,
                public,
                "shared.txt".into(),
                "text/plain".into(),
                b"pub".to_vec(),
            )
            .unwrap();
        // A DM between owner and bob.
        state
            .post_file_dm(
                owner,
                bob,
                "dm.txt".into(),
                "text/plain".into(),
                b"dm".to_vec(),
            )
            .unwrap();
        // A private channel bob is not in.
        let secret = state
            .create_channel_of_kind(owner, "secret", ChannelKind::Private, vec![])
            .unwrap()
            .id;
        state
            .post_file(
                owner,
                secret,
                "hidden.txt".into(),
                "text/plain".into(),
                b"sec".to_vec(),
            )
            .unwrap();

        let listing = state.files_for(bob);
        let names: Vec<&str> = listing.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"shared.txt"));
        assert!(names.contains(&"dm.txt"), "his own DM thread");
        assert!(
            !names.contains(&"hidden.txt"),
            "a private channel he is not in"
        );

        // He may delete only what he uploaded.
        let shared = listing.iter().find(|f| f.name == "shared.txt").unwrap();
        assert!(shared.can_delete);
        assert_eq!(shared.uploader_name, "bob");
        let dm = listing.iter().find(|f| f.name == "dm.txt").unwrap();
        assert!(!dm.can_delete, "the owner uploaded it");
        assert!(dm.is_dm);
    }

    /// Governance actions are recorded, and the log is admin-only: it says who
    /// did what to whom, which is not an ordinary member's to enumerate.
    #[test]
    fn governance_actions_are_audited_and_the_log_is_admin_only() {
        let mut state = PartyState::new("Srv", None);
        let owner = state.join("owner", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();

        state.set_role(owner, bob, Role::Admin).unwrap();
        state
            .create_channel_of_kind(owner, "news", ChannelKind::Announce, vec![])
            .unwrap();

        let log = state.audit_log(owner, 50).unwrap();
        assert!(log.len() >= 2);
        // Newest first.
        assert_eq!(log[0].action, "channel.create");
        assert_eq!(log[1].action, "role.set");
        assert_eq!(log[1].actor_name, "owner");
        assert!(log[1].detail.contains("bob"));

        // Bob is an admin now, so he may read it too.
        assert!(state.audit_log(bob, 50).is_ok());
        state.set_role(owner, bob, Role::Member).unwrap();
        assert!(state.audit_log(bob, 50).is_err());
    }

    /// A community that predates roles must not reload with nobody able to
    /// administer it.
    ///
    /// `join` only appoints an owner when the member table is empty, and the
    /// migration defaults existing rows to `Member`. Without a bootstrap the
    /// operator would be permanently locked out of governance on their own
    /// server, because `set_role` requires an admin that does not exist.
    #[test]
    fn an_upgraded_server_with_no_admin_gets_an_owner() {
        let dir = tempfile::tempdir().unwrap();
        let (alice, bob);
        {
            let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
            alice = s.join("alice", None, None).unwrap();
            bob = s.join("bob", None, None).unwrap();
            // Simulate the pre-roles state: everybody an ordinary member.
            for id in [alice, bob] {
                let m = s.members.get_mut(&id).unwrap();
                m.role = Role::Member;
                let snapshot = s.members.get(&id).unwrap();
                s.persist_member(snapshot);
            }
            assert!(!s.members.values().any(|m| m.role.is_admin()));
        }

        let s = PartyState::load("Srv", None, dir.path()).unwrap();
        let owners: Vec<Uuid> = s
            .members
            .values()
            .filter(|m| m.role == Role::Owner)
            .map(|m| m.id)
            .collect();
        assert_eq!(owners.len(), 1, "exactly one owner is appointed");
        // Deterministic, so a restart does not shuffle ownership around.
        assert_eq!(owners[0], alice.min(bob));
        // And it is recorded, because silently handing somebody ownership is
        // the sort of thing an operator should be able to see happened.
        let log = s.audit_log(owners[0], 10).unwrap();
        assert!(log.iter().any(|e| e.action == "role.bootstrap"));

        // A server that already has an admin is left alone.
        let again = PartyState::load("Srv", None, dir.path()).unwrap();
        assert_eq!(
            again
                .members
                .values()
                .filter(|m| m.role == Role::Owner)
                .count(),
            1,
            "the bootstrap does not run twice"
        );
    }

    /// The same content shared twice in one place is two shares, and both have
    /// to survive a restart — otherwise the blob's reference count outlives the
    /// references, and the last visible one cannot release it.
    #[test]
    fn repeated_shares_of_one_file_survive_a_reload_individually() {
        let dir = tempfile::tempdir().unwrap();
        let (owner, channel, hash);
        {
            let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
            owner = s.join("owner", None, None).unwrap();
            channel = s.default_channel();
            let env = s
                .post_file(
                    owner,
                    channel,
                    "twice.txt".into(),
                    "text/plain".into(),
                    b"same bytes".to_vec(),
                )
                .unwrap();
            hash = file_payload(&env).hash.clone();
            s.post_file(
                owner,
                channel,
                "twice.txt".into(),
                "text/plain".into(),
                b"same bytes".to_vec(),
            )
            .unwrap();
            assert_eq!(s.blobs[&hash].refcount, 2);
            assert_eq!(s.file_refs.len(), 2);
        }

        let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
        assert_eq!(s.file_refs.len(), 2, "both shares came back");
        assert_eq!(s.blobs[&hash].refcount, 2);

        // Releasing them one at a time reclaims the bytes exactly once.
        s.delete_file(owner, &hash, channel).unwrap();
        assert!(
            s.blob_bytes(&hash).is_some(),
            "one reference still holds it"
        );
        s.delete_file(owner, &hash, channel).unwrap();
        assert!(!s.blobs.contains_key(&hash));
        assert!(!dir.path().join(BLOB_DIR).join(&hash).exists());
    }

    /// Roles, channel kinds, private membership, file provenance and the audit
    /// log all have to survive a restart, or the server forgets who runs it.
    #[test]
    fn governance_state_survives_a_reload() {
        let dir = tempfile::tempdir().unwrap();
        let (owner, bob, private_id, hash);
        {
            let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
            owner = s.join("owner", None, None).unwrap();
            bob = s.join("bob", None, None).unwrap();
            s.set_role(owner, bob, Role::Admin).unwrap();
            private_id = s
                .create_channel_of_kind(owner, "secret", ChannelKind::Private, vec![bob])
                .unwrap()
                .id;
            let env = s
                .post_file(
                    owner,
                    private_id,
                    "p.txt".into(),
                    "text/plain".into(),
                    b"private bytes".to_vec(),
                )
                .unwrap();
            hash = file_payload(&env).hash.clone();
        }

        let s = PartyState::load("Srv", None, dir.path()).unwrap();
        assert_eq!(s.role_of(owner), Role::Owner);
        assert_eq!(s.role_of(bob), Role::Admin);
        let chan = s
            .channels()
            .into_iter()
            .find(|c| c.id == private_id)
            .unwrap();
        assert_eq!(chan.kind, ChannelKind::Private);
        assert!(chan.members.contains(&bob));
        assert!(chan.members.contains(&owner));
        // Provenance came back with it, so the Drive panel and quotas still work.
        let files = s.files_for(owner);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].uploader, owner);
        assert_eq!(files[0].location_name, "#secret");
        assert_eq!(s.quota_for(owner).used, 13);
        assert!(s.blob_bytes_for(bob, &hash).is_some());
        assert!(!s.audit_log(owner, 50).unwrap().is_empty());
    }

    /// Deleting a channel has to release the files shared in it, or their bytes
    /// are stranded with nothing holding a count and nothing able to reach them.
    #[test]
    fn deleting_a_channel_reclaims_the_files_shared_in_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
        let owner = state.join("owner", None, None).unwrap();
        let extra = state
            .create_channel_of_kind(owner, "temp", ChannelKind::Public, vec![])
            .unwrap()
            .id;
        let env = state
            .post_file(
                owner,
                extra,
                "t.txt".into(),
                "text/plain".into(),
                b"temp".to_vec(),
            )
            .unwrap();
        let hash = file_payload(&env).hash.clone();

        state.delete_channel(owner, extra).unwrap();
        assert!(!state.blobs.contains_key(&hash));
        assert!(!dir.path().join(BLOB_DIR).join(&hash).exists());
        assert!(state.files_for(owner).is_empty());
        assert_eq!(state.quota_for(owner).used, 0);

        // The last channel cannot go: `default_channel` needs one to exist.
        let last = state.default_channel();
        assert!(state.delete_channel(owner, last).is_err());
    }

    /// A file too large to fit in one frame goes up in chunks and comes back
    /// down the same way, and the result is indistinguishable from an inline
    /// upload once it lands.
    #[test]
    fn a_chunked_upload_round_trips_and_posts_a_file_message() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
        let owner = state.join("owner", None, None).unwrap();
        let channel = state.default_channel();

        // Deliberately not a multiple of the chunk size, so the last chunk is
        // short — the case an off-by-one gets wrong.
        let payload: Vec<u8> = (0..(PARTY_CHUNK_BYTES * 2 + 77))
            .map(|i| (i % 251) as u8)
            .collect();

        let upload = state
            .start_upload(
                owner,
                "big.bin".into(),
                "application/octet-stream".into(),
                payload.len() as u64,
                UploadTarget::Channel(channel),
            )
            .unwrap();
        for chunk in payload.chunks(PARTY_CHUNK_BYTES) {
            state.upload_chunk(owner, upload, chunk).unwrap();
        }
        let (env, target) = state.finish_upload(owner, upload).unwrap();
        assert_eq!(target, UploadTarget::Channel(channel));
        let meta = file_payload(&env);
        assert_eq!(meta.size, payload.len() as u64);
        assert_eq!(meta.hash, blob_hash(&payload));

        // It is a normal file message in the channel, and a normal blob.
        assert_eq!(state.history_since(owner, channel, 0).len(), 1);
        assert_eq!(
            state.blob_bytes_for(owner, &meta.hash),
            Some(payload.clone())
        );

        // And it comes back down in order, in chunks, with the total attached.
        let mut got = Vec::new();
        loop {
            let (data, total) = state
                .blob_chunk_for(owner, &meta.hash, got.len() as u64)
                .unwrap();
            assert_eq!(total, payload.len() as u64);
            if data.is_empty() {
                break;
            }
            got.extend_from_slice(&data);
            if got.len() as u64 >= total {
                break;
            }
        }
        assert_eq!(got, payload);
    }

    /// The declared size is the contract: a client that sends more than it said
    /// it would is cut off rather than allowed to grow the server's memory.
    #[test]
    fn an_upload_that_exceeds_its_declared_size_is_dropped() {
        let mut state = PartyState::new("Srv", None);
        let owner = state.join("owner", None, None).unwrap();
        let channel = state.default_channel();
        let upload = state
            .start_upload(
                owner,
                "x.bin".into(),
                "application/octet-stream".into(),
                4,
                UploadTarget::Channel(channel),
            )
            .unwrap();
        state.upload_chunk(owner, upload, b"abcd").unwrap();
        assert!(state.upload_chunk(owner, upload, b"more").is_err());
        // The upload is gone, so finishing it fails too.
        assert!(state.finish_upload(owner, upload).is_err());
    }

    /// Finishing early must not store a truncated file under a hash that claims
    /// to be the whole thing.
    #[test]
    fn an_incomplete_upload_cannot_be_finished() {
        let mut state = PartyState::new("Srv", None);
        let owner = state.join("owner", None, None).unwrap();
        let channel = state.default_channel();
        let upload = state
            .start_upload(
                owner,
                "x.bin".into(),
                "application/octet-stream".into(),
                10,
                UploadTarget::Channel(channel),
            )
            .unwrap();
        state.upload_chunk(owner, upload, b"abc").unwrap();
        let err = state.finish_upload(owner, upload).unwrap_err();
        assert!(err.contains("incomplete"), "got: {err}");
    }

    /// Everything checkable is checked before a byte moves — otherwise the
    /// server spools a hundred megabytes and only then says no.
    #[test]
    fn uploads_are_refused_up_front_on_size_quota_and_permission() {
        let mut state = PartyState::new("Srv", None);
        let owner = state.join("owner", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let channel = state.default_channel();
        let octet = || "application/octet-stream".to_string();

        // Past the server's hard ceiling.
        assert!(state
            .start_upload(
                bob,
                "huge".into(),
                octet(),
                MAX_PARTY_FILE_BYTES + 1,
                UploadTarget::Channel(channel),
            )
            .is_err());
        // Empty.
        assert!(state
            .start_upload(bob, "e".into(), octet(), 0, UploadTarget::Channel(channel),)
            .is_err());
        // Past the member's allowance.
        state.set_max_member_blob_bytes(1024);
        let err = state
            .start_upload(
                bob,
                "big".into(),
                octet(),
                4096,
                UploadTarget::Channel(channel),
            )
            .unwrap_err();
        assert!(err.contains("allowance"), "got: {err}");
        state.set_max_member_blob_bytes(MAX_MEMBER_BLOB_BYTES);
        // Too many at once. The cap counts the spools the state is actually
        // holding, so it has to be reached by opening them, not by asserting it.
        let opened: Vec<Uuid> = (0..MAX_CONCURRENT_UPLOADS)
            .map(|i| {
                state
                    .start_upload(
                        bob,
                        format!("n{i}"),
                        octet(),
                        16,
                        UploadTarget::Channel(channel),
                    )
                    .expect("under the cap")
            })
            .collect();
        let err = state
            .start_upload(bob, "n".into(), octet(), 16, UploadTarget::Channel(channel))
            .unwrap_err();
        assert!(err.contains("in progress"), "got: {err}");
        for upload in opened {
            state.cancel_upload(bob, upload);
        }
        // A channel they may not post to.
        let locked = state
            .create_channel_of_kind(owner, "locked", ChannelKind::Locked, vec![])
            .unwrap()
            .id;
        assert!(state
            .start_upload(bob, "l".into(), octet(), 16, UploadTarget::Channel(locked),)
            .is_err());
        // A guest cannot upload at all, DM included.
        state.set_role(owner, bob, Role::Guest).unwrap();
        assert!(state
            .start_upload(bob, "g".into(), octet(), 16, UploadTarget::Dm(owner))
            .is_err());
    }

    /// One member's upload id must be useless to anyone else, and a disconnect
    /// must not leave the spool pinned for the life of the process.
    #[test]
    fn uploads_are_owned_by_their_uploader_and_cleaned_up() {
        let mut state = PartyState::new("Srv", None);
        let owner = state.join("owner", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let channel = state.default_channel();
        let upload = state
            .start_upload(
                bob,
                "x".into(),
                "application/octet-stream".into(),
                8,
                UploadTarget::Channel(channel),
            )
            .unwrap();

        // Someone else's id gets the same answer as a made-up one.
        assert!(state.upload_chunk(owner, upload, b"abcd").is_err());
        assert!(state.finish_upload(owner, upload).is_err());
        state.cancel_upload(owner, upload);
        assert!(
            state.upload_chunk(bob, upload, b"abcd").is_ok(),
            "still bob's"
        );

        // Bob's connection goes away.
        state.cancel_uploads_for(bob);
        assert!(state.upload_chunk(bob, upload, b"abcd").is_err());
    }

    /// An upload finished short must not leave its spool behind.
    ///
    /// This was an unbounded memory leak a Guest could drive: declare the
    /// maximum, send all but a byte of it, call `FinishUpload`. The server
    /// answered "upload is incomplete" and kept every byte, while the
    /// connection — which cleared its own list *before* the call — reported
    /// nothing in flight, so `MAX_CONCURRENT_UPLOADS` read zero and the
    /// disconnect sweep, gated on that same list, never ran. Each round leaked
    /// up to `MAX_PARTY_FILE_BYTES` and nothing short of a restart reclaimed it.
    #[test]
    fn an_upload_finished_short_does_not_leak_its_spool() {
        let mut state = PartyState::new("Srv", None);
        let owner = state.join("owner", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let channel = state.default_channel();
        let _ = owner;

        for _ in 0..(MAX_CONCURRENT_UPLOADS * 3) {
            let upload = state
                .start_upload(
                    bob,
                    "x".into(),
                    "application/octet-stream".into(),
                    64,
                    UploadTarget::Channel(channel),
                )
                .expect("the cap must not be reached by abandoned uploads");
            state.upload_chunk(bob, upload, &[0u8; 32]).unwrap();

            let err = state.finish_upload(bob, upload).unwrap_err();
            assert!(err.contains("incomplete"), "got: {err}");

            assert_eq!(
                state.staged_upload_bytes(),
                0,
                "the spool survived a short finish"
            );
            assert_eq!(
                state.uploads_in_flight(bob),
                0,
                "the abandoned upload still counts against the cap"
            );
        }
    }

    /// The concurrency cap is enforced against the spools the server holds, not
    /// against a number a caller passes in. Nothing a connection forgets can
    /// buy a member more of them.
    #[test]
    fn the_upload_cap_counts_what_the_server_is_holding() {
        let mut state = PartyState::new("Srv", None);
        let bob = state.join("bob", None, None).unwrap();
        let channel = state.default_channel();

        let mut opened = Vec::new();
        for i in 0..MAX_CONCURRENT_UPLOADS {
            opened.push(
                state
                    .start_upload(
                        bob,
                        format!("f{i}"),
                        "application/octet-stream".into(),
                        16,
                        UploadTarget::Channel(channel),
                    )
                    .expect("under the cap"),
            );
        }
        assert_eq!(state.uploads_in_flight(bob), MAX_CONCURRENT_UPLOADS);
        assert!(state
            .start_upload(
                bob,
                "one-too-many".into(),
                "application/octet-stream".into(),
                16,
                UploadTarget::Channel(channel),
            )
            .is_err());

        // Cancelling one frees exactly one slot.
        state.cancel_upload(bob, opened[0]);
        assert_eq!(state.uploads_in_flight(bob), MAX_CONCURRENT_UPLOADS - 1);
        assert!(state
            .start_upload(
                bob,
                "now-there-is-room".into(),
                "application/octet-stream".into(),
                16,
                UploadTarget::Channel(channel),
            )
            .is_ok());
    }

    /// A chunked download reads one chunk, not the whole file.
    ///
    /// Serving 64 KiB of a 100 MiB blob used to read all 100 MiB off disk —
    /// about 160 GiB over a full download's 1,600 chunks, every byte of it with
    /// the state mutex held, so one member fetching a large file repeatedly
    /// stalled everyone else's messages.
    ///
    /// The observable contract is unchanged, which is what this pins: same
    /// bytes, same reported total, same behaviour past the end.
    #[test]
    fn a_chunked_read_returns_the_same_bytes_from_disk_as_from_memory() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
        let owner = state.join("owner", None, None).unwrap();
        let channel = state.default_channel();

        // Two and a bit chunks, so offsets land mid-file and at the tail.
        let payload: Vec<u8> = (0..(PARTY_CHUNK_BYTES * 2 + 1234))
            .map(|i| (i % 251) as u8)
            .collect();
        let env = state
            .post_file(
                owner,
                channel,
                "big.bin".into(),
                "application/octet-stream".into(),
                payload.clone(),
            )
            .unwrap();
        let hash = file_payload(&env).hash.clone();

        let mut reassembled = Vec::new();
        let mut offset = 0u64;
        loop {
            let (chunk, total) = state
                .blob_chunk_for(owner, &hash, offset)
                .expect("the owner may read their own file");
            assert_eq!(total, payload.len() as u64, "reported size drifted");
            if chunk.is_empty() {
                break;
            }
            assert!(
                chunk.len() <= PARTY_CHUNK_BYTES,
                "a chunk must not exceed the chunk size"
            );
            reassembled.extend_from_slice(&chunk);
            offset += chunk.len() as u64;
        }
        assert_eq!(reassembled, payload, "the file did not round-trip");

        // Past the end is an empty chunk and the real total, not an error.
        let (tail, total) = state
            .blob_chunk_for(owner, &hash, payload.len() as u64 + 99)
            .unwrap();
        assert!(tail.is_empty());
        assert_eq!(total, payload.len() as u64);

        // Access is still decided before any byte is read.
        let mallory = state.join("mallory", None, None).unwrap();
        state.set_role(owner, mallory, Role::Guest).unwrap();
        let private = state
            .create_channel_of_kind(owner, "private", ChannelKind::Private, vec![])
            .unwrap()
            .id;
        let env = state
            .post_file(
                owner,
                private,
                "secret.bin".into(),
                "application/octet-stream".into(),
                b"secret".to_vec(),
            )
            .unwrap();
        let secret_hash = file_payload(&env).hash.clone();
        assert!(
            state.blob_chunk_for(mallory, &secret_hash, 0).is_none(),
            "a chunked read must not bypass the access check"
        );
    }

    /// Peak disk is bounded by the storage ceiling, not by what happens to
    /// arrive at once.
    ///
    /// Phase 2 writes the whole payload before phase 3 re-checks the ceiling, so
    /// without a reservation the limit bounds only what is *stored*: members
    /// each finishing several uploads would stage far past it and only then be
    /// refused, having already written every byte. Reserving in phase 1 is what
    /// makes the ceiling mean something about the disk.
    #[test]
    fn staged_uploads_are_counted_against_the_storage_ceiling() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
        let owner = state.join("owner", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let channel = state.default_channel();
        // Room for two of these and no more.
        state.set_max_blob_bytes(200);
        state.set_max_member_blob_bytes(1_000_000);
        let _ = owner;

        let mut staged = Vec::new();
        for i in 0..2 {
            let upload = state
                .start_upload(
                    bob,
                    format!("f{i}"),
                    "application/octet-stream".into(),
                    100,
                    UploadTarget::Channel(channel),
                )
                .unwrap();
            state.upload_chunk(bob, upload, &[b'x'; 100]).unwrap();
            staged.push(state.begin_upload(bob, upload).expect("within the ceiling"));
        }

        // Nothing is stored yet — every byte of that is a reservation — and the
        // third upload is refused *before* it is written, not after.
        assert_eq!(state.blobs.len(), 0, "nothing has been committed yet");
        let upload = state
            .start_upload(
                bob,
                "third".into(),
                "application/octet-stream".into(),
                100,
                UploadTarget::Channel(channel),
            )
            .unwrap();
        state.upload_chunk(bob, upload, &[b'z'; 100]).unwrap();
        let err = state
            .begin_upload(bob, upload)
            .expect_err("the ceiling is already spoken for");
        assert!(err.contains("storage is full"), "got: {err}");

        // Giving one back frees exactly its reservation.
        let first = staged.remove(0);
        state.discard_upload(first);
        assert!(
            state.begin_upload(bob, upload).is_ok(),
            "discarding a staged upload must release its claim"
        );
    }

    /// A member whose connection dies between phase 1 and phase 3 must not
    /// leave a claim on storage that nothing releases.
    #[test]
    fn a_disconnect_releases_a_staged_upload_s_claim() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let channel = state.default_channel();
        state.set_max_blob_bytes(150);

        let upload = state
            .start_upload(
                bob,
                "f".into(),
                "application/octet-stream".into(),
                100,
                UploadTarget::Channel(channel),
            )
            .unwrap();
        state.upload_chunk(bob, upload, &[b'x'; 100]).unwrap();
        let staged = state.begin_upload(bob, upload).unwrap();

        // The connection goes away with the upload mid-flight.
        state.cancel_uploads_for(bob);
        drop(staged);

        let upload = state
            .start_upload(
                bob,
                "again".into(),
                "application/octet-stream".into(),
                100,
                UploadTarget::Channel(channel),
            )
            .unwrap();
        state.upload_chunk(bob, upload, &[b'y'; 100]).unwrap();
        assert!(
            state.begin_upload(bob, upload).is_ok(),
            "the abandoned upload's claim was never released"
        );
    }

    /// Two members uploading identical content must not be able to delete each
    /// other's bytes.
    ///
    /// Staging under the *content hash* would let one member's failed commit
    /// unlink the file the other's commit is about to record, leaving a file
    /// message that answers "unknown file" for good. Staging under a per-upload
    /// id makes that impossible rather than unlikely.
    #[test]
    fn identical_concurrent_uploads_do_not_delete_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
        // The first joiner is the Owner and cannot be demoted, so the two
        // uploaders are ordinary members joining after them.
        let owner = state.join("owner", None, None).unwrap();
        let alice = state.join("alice", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let channel = state.default_channel();
        let payload = b"a forwarded image".to_vec();

        let stage_for = |state: &mut PartyState, who: Uuid| {
            let upload = state
                .start_upload(
                    who,
                    "same.bin".into(),
                    "application/octet-stream".into(),
                    payload.len() as u64,
                    UploadTarget::Channel(channel),
                )
                .unwrap();
            state.upload_chunk(who, upload, &payload).unwrap();
            let mut staged = state.begin_upload(who, upload).unwrap();
            staged.write_staging().unwrap();
            staged
        };

        // Both stage the same bytes, both written to disk, neither committed.
        let alice_staged = stage_for(&mut state, alice);
        let bob_staged = stage_for(&mut state, bob);

        // Alice's commit fails on the far side of the write — she was demoted
        // while her bytes were going down.
        state.set_role(owner, alice, Role::Guest).unwrap();
        assert!(
            state.commit_upload(alice_staged).is_err(),
            "a guest may not post"
        );

        // Bob's commit must still produce a file that downloads.
        let (env, _) = state.commit_upload(bob_staged).expect("bob may still post");
        let hash = file_payload(&env).hash.clone();
        assert_eq!(
            state.blob_bytes_for(bob, &hash),
            Some(payload),
            "the surviving upload's bytes were deleted by the failed one"
        );
    }

    /// The staging directory does not accumulate files.
    #[test]
    fn a_discarded_upload_leaves_no_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = dir.path().join("blobs");
        let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let channel = state.default_channel();

        for _ in 0..3 {
            let upload = state
                .start_upload(
                    bob,
                    "f".into(),
                    "application/octet-stream".into(),
                    8,
                    UploadTarget::Channel(channel),
                )
                .unwrap();
            state.upload_chunk(bob, upload, b"12345678").unwrap();
            let mut staged = state.begin_upload(bob, upload).unwrap();
            staged.write_staging().unwrap();
            state.discard_upload(staged);
        }

        let leftovers: Vec<_> = std::fs::read_dir(&blobs)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("staging_"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "staging files left behind: {leftovers:?}"
        );
    }

    /// A crash leaves staging files nothing else will ever remove, so the next
    /// start sweeps them.
    #[test]
    fn startup_sweeps_staging_files_a_crash_left_behind() {
        let dir = tempfile::tempdir().unwrap();
        {
            let _state = PartyState::load("Srv", None, dir.path()).unwrap();
        }
        let blobs = dir.path().join("blobs");
        std::fs::write(blobs.join("staging_orphan-one"), b"partial").unwrap();
        std::fs::write(blobs.join("staging_orphan-two"), b"partial").unwrap();
        // A real blob, which must survive: its name is a content hash and no
        // hash starts with the staging prefix.
        let keep = blobs.join("abc123");
        std::fs::write(&keep, b"real").unwrap();

        let _state = PartyState::load("Srv", None, dir.path()).unwrap();

        assert!(keep.exists(), "the sweep removed a real blob");
        let leftovers: Vec<_> = std::fs::read_dir(&blobs)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("staging_"))
            .collect();
        assert!(leftovers.is_empty(), "sweep missed: {leftovers:?}");
    }

    /// Permission is re-checked on the far side of the write, not only before it.
    ///
    /// `begin_upload` checks; for a 100 MiB upload that check is minutes old by
    /// the time the bytes land, and the lock was released for all of it. Anyone
    /// demoted, or any channel deleted, in that window used to get its message
    /// appended anyway. `check_may_post_file` is one function called from both
    /// ends precisely so the two cannot drift — which is how the DM recipient
    /// came to be checked only on the way in.
    #[test]
    fn permission_lost_during_the_write_refuses_the_commit() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
        let owner = state.join("owner", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let carol = state.join("carol", None, None).unwrap();
        let channel = state.default_channel();

        // (a) The uploader is demoted to a read-only role mid-DM-upload.
        let upload = state
            .start_upload(
                bob,
                "for-carol.bin".into(),
                "application/octet-stream".into(),
                4,
                UploadTarget::Dm(carol),
            )
            .unwrap();
        state.upload_chunk(bob, upload, b"abcd").unwrap();
        let mut staged = state.begin_upload(bob, upload).unwrap();
        staged.write_staging().unwrap();
        state.set_role(owner, bob, Role::Guest).unwrap();

        let err = state
            .commit_upload(staged)
            .expect_err("a guest may not send a DM");
        assert!(err.contains("read-only"), "got: {err}");
        assert!(
            state
                .dm_history(messenger_core::party::dm_thread_id(bob, carol), 0)
                .is_empty(),
            "the message was appended despite the refusal"
        );

        // (b) The channel is deleted mid-upload.
        state.set_role(owner, bob, Role::Member).unwrap();
        let doomed = state
            .create_channel_of_kind(owner, "doomed", ChannelKind::Public, vec![])
            .unwrap()
            .id;
        let upload = state
            .start_upload(
                bob,
                "f.bin".into(),
                "application/octet-stream".into(),
                4,
                UploadTarget::Channel(doomed),
            )
            .unwrap();
        state.upload_chunk(bob, upload, b"efgh").unwrap();
        let mut staged = state.begin_upload(bob, upload).unwrap();
        staged.write_staging().unwrap();
        state.delete_channel(owner, doomed).unwrap();

        assert!(
            state.commit_upload(staged).is_err(),
            "a deleted channel still accepted a file"
        );

        // And nothing of either attempt is left on disk.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path().join("blobs"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("staging_"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "staging files left behind: {leftovers:?}"
        );

        // A permitted upload still works, so this is not just refusing everything.
        let upload = state
            .start_upload(
                bob,
                "ok.bin".into(),
                "application/octet-stream".into(),
                4,
                UploadTarget::Channel(channel),
            )
            .unwrap();
        state.upload_chunk(bob, upload, b"ijkl").unwrap();
        let mut staged = state.begin_upload(bob, upload).unwrap();
        staged.write_staging().unwrap();
        assert!(state.commit_upload(staged).is_ok());
    }

    /// Someone else's failed `FinishUpload` must not discard your spool. The
    /// "no such upload" answer is deliberately the same for a stranger's id and
    /// a made-up one, and it must stay just as inert.
    #[test]
    fn a_strangers_finish_does_not_drop_your_upload() {
        let mut state = PartyState::new("Srv", None);
        let owner = state.join("owner", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let channel = state.default_channel();

        let upload = state
            .start_upload(
                bob,
                "x".into(),
                "application/octet-stream".into(),
                8,
                UploadTarget::Channel(channel),
            )
            .unwrap();
        state.upload_chunk(bob, upload, b"abcd").unwrap();

        assert!(state.finish_upload(owner, upload).is_err());
        assert_eq!(state.uploads_in_flight(bob), 1, "bob's upload was dropped");
        assert!(state.upload_chunk(bob, upload, b"efgh").is_ok());
        assert!(state.finish_upload(bob, upload).is_ok());
    }

    /// The rights are separate on purpose: seeing that a file exists is not the
    /// same as being able to fetch it.
    #[test]
    fn view_and_download_are_separate_rights() {
        let mut state = PartyState::new("Srv", None);
        let owner = state.join("owner", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let chan = state.default_channel();
        let env = state
            .post_file(
                owner,
                chan,
                "d.txt".into(),
                "text/plain".into(),
                b"bytes".to_vec(),
            )
            .unwrap();
        let hash = file_payload(&env).hash.clone();

        // The default: bob can see it and fetch it.
        assert_eq!(state.files_for(bob).len(), 1);
        assert!(state.blob_bytes_for(bob, &hash).is_some());

        // Downgrade to view-only, for everyone.
        state
            .set_file_permissions(
                owner,
                &hash,
                chan,
                None,
                FilePermissions {
                    view: true,
                    download: false,
                    delete: false,
                    share: false,
                },
            )
            .unwrap();
        assert_eq!(state.files_for(bob).len(), 1, "he can still see it");
        assert!(
            state.blob_bytes_for(bob, &hash).is_none(),
            "but not fetch it"
        );

        // Take view away too and it disappears from his listing entirely.
        state
            .set_file_permissions(owner, &hash, chan, None, FilePermissions::none())
            .unwrap();
        assert!(state.files_for(bob).is_empty());
        // The uploader is unaffected by the grant they handed out.
        assert!(state.blob_bytes_for(owner, &hash).is_some());
    }

    /// A per-member grant overrides the default for that member and nobody else.
    #[test]
    fn a_member_grant_overrides_the_default() {
        let mut state = PartyState::new("Srv", None);
        let owner = state.join("owner", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let carol = state.join("carol", None, None).unwrap();
        let chan = state.default_channel();
        let env = state
            .post_file(
                owner,
                chan,
                "d.txt".into(),
                "text/plain".into(),
                b"bytes".to_vec(),
            )
            .unwrap();
        let hash = file_payload(&env).hash.clone();

        state
            .set_file_permissions(owner, &hash, chan, None, FilePermissions::none())
            .unwrap();
        state
            .set_file_permissions(
                owner,
                &hash,
                chan,
                Some(bob),
                FilePermissions {
                    view: true,
                    download: true,
                    delete: false,
                    share: true,
                },
            )
            .unwrap();

        assert!(
            state.blob_bytes_for(bob, &hash).is_some(),
            "bob was granted"
        );
        assert!(
            state.blob_bytes_for(carol, &hash).is_none(),
            "carol still has the default, which is nothing"
        );
    }

    /// You can only hand out rights you hold — otherwise a grant is a way to
    /// mint authority rather than pass it on.
    #[test]
    fn a_grant_cannot_exceed_what_the_granter_holds() {
        let mut state = PartyState::new("Srv", None);
        let owner = state.join("owner", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let carol = state.join("carol", None, None).unwrap();
        let chan = state.default_channel();

        // Bob shares a file, so he is its uploader and holds everything.
        let env = state
            .post_file(
                bob,
                chan,
                "b.txt".into(),
                "text/plain".into(),
                b"bobs".to_vec(),
            )
            .unwrap();
        let hash = file_payload(&env).hash.clone();
        assert!(state
            .set_file_permissions(bob, &hash, chan, Some(carol), FilePermissions::all())
            .is_ok());

        // Carol holds everything now, but she is not the uploader and not an
        // admin, so she may not rewrite who else may do what.
        let err = state
            .set_file_permissions(carol, &hash, chan, Some(owner), FilePermissions::all())
            .unwrap_err();
        assert!(err.contains("only the member who shared"), "got: {err}");
    }

    /// Re-sharing spends a reference on content the server already holds, and
    /// the new share starts at the default rather than inheriting the sharer's
    /// authority over it.
    #[test]
    fn sharing_a_file_elsewhere_costs_a_reference_not_a_copy() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
        let owner = state.join("owner", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let first = state.default_channel();
        let second = state
            .create_channel_of_kind(owner, "second", ChannelKind::Public, vec![])
            .unwrap()
            .id;

        let env = state
            .post_file(
                owner,
                first,
                "d.txt".into(),
                "text/plain".into(),
                b"shared".to_vec(),
            )
            .unwrap();
        let hash = file_payload(&env).hash.clone();

        // Bob has only the default grant, which does not include share.
        let err = state
            .share_file(bob, &hash, first, UploadTarget::Channel(second))
            .unwrap_err();
        assert!(err.contains("permission to share"), "got: {err}");

        // Granted, he can — and it is a reference, not a second upload.
        state
            .set_file_permissions(
                owner,
                &hash,
                first,
                Some(bob),
                FilePermissions {
                    view: true,
                    download: true,
                    delete: false,
                    share: true,
                },
            )
            .unwrap();
        let shared = state
            .share_file(bob, &hash, first, UploadTarget::Channel(second))
            .unwrap();
        assert_eq!(file_payload(&shared).hash, hash);
        assert_eq!(state.blobs[&hash].refcount, 2, "one blob, two references");
        assert_eq!(
            state.blob_bytes(&hash),
            Some(b"shared".to_vec()),
            "the bytes were never moved again"
        );

        // The new share starts at the default: bob passed on the file, not his
        // own right to spread it further.
        let there = state
            .file_refs
            .iter()
            .find(|r| r.location == second)
            .unwrap();
        assert_eq!(there.default_perms, FilePermissions::default());
        assert_eq!(there.uploader, bob, "he is who put it there");

        // Sharing it into the same place twice is refused rather than
        // silently inflating the reference count.
        assert!(state
            .share_file(bob, &hash, first, UploadTarget::Channel(second))
            .is_err());

        // Deleting one reference leaves the other holding the bytes.
        state.delete_file(bob, &hash, second).unwrap();
        assert_eq!(state.blobs[&hash].refcount, 1);
        assert!(state.blob_bytes(&hash).is_some());
    }

    /// A guest is read-only everywhere, and that outranks any grant — otherwise
    /// this would be the one place a guest could write.
    #[test]
    fn a_grant_cannot_give_a_guest_write_rights() {
        let mut state = PartyState::new("Srv", None);
        let owner = state.join("owner", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let chan = state.default_channel();
        let env = state
            .post_file(
                owner,
                chan,
                "d.txt".into(),
                "text/plain".into(),
                b"bytes".to_vec(),
            )
            .unwrap();
        let hash = file_payload(&env).hash.clone();

        state
            .set_file_permissions(owner, &hash, chan, Some(bob), FilePermissions::all())
            .unwrap();
        state.set_role(owner, bob, Role::Guest).unwrap();

        // He kept view and download, and lost the two that are writes.
        let listing = state.files_for(bob);
        assert_eq!(listing.len(), 1);
        assert!(listing[0].perms.view && listing[0].perms.download);
        assert!(!listing[0].perms.delete && !listing[0].perms.share);
        assert!(state.delete_file(bob, &hash, chan).is_err());
    }

    /// Permissions have to survive a restart, or a grant quietly evaporates.
    #[test]
    fn file_permissions_survive_a_reload() {
        let dir = tempfile::tempdir().unwrap();
        let (owner, bob, chan, hash);
        {
            let mut s = PartyState::load("Srv", None, dir.path()).unwrap();
            owner = s.join("owner", None, None).unwrap();
            bob = s.join("bob", None, None).unwrap();
            chan = s.default_channel();
            let env = s
                .post_file(
                    owner,
                    chan,
                    "d.txt".into(),
                    "text/plain".into(),
                    b"bytes".to_vec(),
                )
                .unwrap();
            hash = file_payload(&env).hash.clone();
            s.set_file_permissions(owner, &hash, chan, None, FilePermissions::none())
                .unwrap();
            s.set_file_permissions(
                owner,
                &hash,
                chan,
                Some(bob),
                FilePermissions {
                    view: true,
                    download: true,
                    delete: true,
                    share: false,
                },
            )
            .unwrap();
        }

        let s = PartyState::load("Srv", None, dir.path()).unwrap();
        let r = s.file_refs.iter().find(|r| r.hash == hash).unwrap();
        assert_eq!(r.default_perms, FilePermissions::none());
        assert!(r.grants.get(&bob).copied().unwrap().delete);
        assert!(s.blob_bytes_for(bob, &hash).is_some(), "his grant survived");
    }

    /// Incoherent grants are normalised rather than stored as written:
    /// downloading what you cannot see, or sharing what you cannot download, is
    /// not a thing to represent.
    #[test]
    fn grants_are_normalised() {
        let odd = FilePermissions {
            view: false,
            download: true,
            delete: false,
            share: true,
        }
        .normalized();
        assert!(odd.view, "downloading implies seeing");
        assert!(odd.download, "sharing implies downloading");

        assert!(FilePermissions::all().covers(FilePermissions::default()));
        assert!(!FilePermissions::default().covers(FilePermissions::all()));
        assert!(FilePermissions::none().covers(FilePermissions::none()));
    }

    /// A whole channel in one frame stopped fitting once history grew past
    /// `MAX_PACKET_SIZE`, and the send failure dropped the connection. History
    /// is paged instead.
    #[test]
    fn history_is_paged_so_a_reply_always_fits_in_one_frame() {
        let mut state = PartyState::new("Open", None);
        let alice = state.join("alice", None, None).unwrap();
        let chan = state.default_channel();
        for i in 0..(MAX_HISTORY_BATCH + 25) {
            state.post_message(alice, chan, format!("m{i}")).unwrap();
        }

        let first = state.history_since(alice, chan, 0);
        assert_eq!(first.len(), MAX_HISTORY_BATCH, "the batch is capped");
        assert_eq!(first[0].seq, 1);

        // Paging with the last seq seen returns the remainder.
        let last_seq = first.last().unwrap().seq;
        let second = state.history_since(alice, chan, last_seq);
        assert_eq!(second.len(), 25);
        assert_eq!(second[0].seq, last_seq + 1);

        // And the page after the end is empty, which is how a client stops.
        let done = state.history_since(alice, chan, second.last().unwrap().seq);
        assert!(done.is_empty());
    }

    #[test]
    fn dm_history_is_paged_too() {
        let mut state = PartyState::new("Open", None);
        let alice = state.join("alice", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        for i in 0..(MAX_HISTORY_BATCH + 5) {
            state.post_dm(alice, bob, format!("m{i}")).unwrap();
        }
        let thread = messenger_core::party::dm_thread_id(alice, bob);
        assert_eq!(state.dm_history(thread, 0).len(), MAX_HISTORY_BATCH);
        assert_eq!(state.dm_history(thread, MAX_HISTORY_BATCH as u64).len(), 5);
    }

    /// Channel creation is open to every member with no admin role, so the cap
    /// is the only thing bounding server state.
    #[test]
    fn channel_creation_is_capped() {
        let mut state = PartyState::new("Open", None);
        // `general` already exists.
        for i in 1..MAX_CHANNELS {
            state
                .create_channel(&format!("chan{i}"))
                .unwrap_or_else(|e| panic!("channel {i} should be allowed: {e}"));
        }
        let err = state
            .create_channel("one-too-many")
            .expect_err("past the cap");
        assert!(err.contains("maximum"), "error should say why: {err}");
        assert_eq!(state.channels.len(), MAX_CHANNELS);
    }

    /// The server-wide [`MAX_CHANNELS`] cap does not tell a hundred members
    /// making one channel each from one member making a hundred. Without a
    /// per-member limit, any single joined member can walk the server to its
    /// ceiling in a burst — broadcasting a refreshed directory to every
    /// connection each time and denying everyone else the feature until an admin
    /// cleans up.
    #[test]
    fn a_member_cannot_burst_create_channels() {
        let mut state = PartyState::new("Open", None);
        let _owner = state.join("owner", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let now = Instant::now();

        for i in 0..MAX_CHANNELS_PER_MEMBER {
            state
                .create_channel_of_kind_at(
                    bob,
                    &format!("bob{i}"),
                    ChannelKind::Public,
                    vec![],
                    now,
                )
                .unwrap_or_else(|e| panic!("channel {i} is within the allowance: {e}"));
        }
        let err = state
            .create_channel_of_kind_at(bob, "one-more", ChannelKind::Public, vec![], now)
            .expect_err("past the per-member allowance");
        assert!(
            err.contains("wait a moment"),
            "the error should say it is temporary: {err}"
        );
        assert_eq!(
            state.channels.len(),
            1 + MAX_CHANNELS_PER_MEMBER,
            "general plus the allowed burst, and nothing more"
        );
    }

    /// The limit is per member, not per server: one member burning their
    /// allowance must not stop everyone else from making a channel.
    #[test]
    fn the_channel_rate_limit_is_per_member() {
        let mut state = PartyState::new("Open", None);
        let _owner = state.join("owner", None, None).unwrap();
        let bob = state.join("bob", None, None).unwrap();
        let carol = state.join("carol", None, None).unwrap();
        let now = Instant::now();

        for i in 0..MAX_CHANNELS_PER_MEMBER {
            state
                .create_channel_of_kind_at(
                    bob,
                    &format!("bob{i}"),
                    ChannelKind::Public,
                    vec![],
                    now,
                )
                .unwrap();
        }
        assert!(state
            .create_channel_of_kind_at(bob, "bob-extra", ChannelKind::Public, vec![], now)
            .is_err());
        assert!(
            state
                .create_channel_of_kind_at(carol, "carol1", ChannelKind::Public, vec![], now)
                .is_ok(),
            "an unrelated member must not be caught in someone else's limit"
        );
    }

    /// It is a rate limit, not a quota — the allowance comes back.
    #[test]
    fn the_channel_rate_limit_window_slides() {
        let mut state = PartyState::new("Open", None);
        let bob = state.join("bob", None, None).unwrap();
        let now = Instant::now();

        for i in 0..MAX_CHANNELS_PER_MEMBER {
            state
                .create_channel_of_kind_at(
                    bob,
                    &format!("bob{i}"),
                    ChannelKind::Public,
                    vec![],
                    now,
                )
                .unwrap();
        }
        assert!(state
            .create_channel_of_kind_at(bob, "blocked", ChannelKind::Public, vec![], now)
            .is_err());

        let later = now + CHANNEL_CREATE_WINDOW + Duration::from_secs(1);
        assert!(state
            .create_channel_of_kind_at(bob, "later", ChannelKind::Public, vec![], later)
            .is_ok());
    }

    /// A refused creation must not cost the member part of their allowance —
    /// otherwise mistyping a duplicate name five times locks you out of making
    /// the channel you were trying to make.
    #[test]
    fn a_rejected_channel_name_does_not_consume_the_allowance() {
        let mut state = PartyState::new("Open", None);
        let bob = state.join("bob", None, None).unwrap();
        let now = Instant::now();

        for _ in 0..20 {
            assert!(state
                .create_channel_of_kind_at(bob, "general", ChannelKind::Public, vec![], now)
                .is_err());
        }
        for i in 0..MAX_CHANNELS_PER_MEMBER {
            state
                .create_channel_of_kind_at(
                    bob,
                    &format!("bob{i}"),
                    ChannelKind::Public,
                    vec![],
                    now,
                )
                .unwrap_or_else(|e| panic!("attempt {i} should still be allowed: {e}"));
        }
    }

    /// A non-member must not be able to read history by guessing a channel id.
    #[test]
    fn history_requires_membership() {
        let mut state = PartyState::new("Open", None);
        let alice = state.join("alice", None, None).unwrap();
        let chan = state.default_channel();
        state.post_message(alice, chan, "hi".to_string()).unwrap();

        let stranger = Uuid::new_v4();
        assert!(state.history_since(stranger, chan, 0).is_empty());
    }
}
