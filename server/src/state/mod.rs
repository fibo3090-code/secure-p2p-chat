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
    blob_hash, AuditEntry, BlobHasher, ChannelInfo, ChannelKind, Envelope, FileEntry, FileMeta,
    FilePermissions, MemberInfo, MessagePayload, QuotaInfo, Role, TrustTier, UploadTarget,
    MAX_HISTORY_BATCH, MAX_INLINE_FILE_BYTES, MAX_PARTY_FILE_BYTES, PARTY_CHUNK_BYTES,
};
use messenger_core::util::{current_timestamp_millis, sanitize_filename};
use rusqlite::{params, Connection};
use serde::Deserialize;
use uuid::Uuid;

mod blobs;
mod db;
#[cfg(test)]
mod tests;

use blobs::*;
pub use blobs::{BlobReadPlan, StagedUpload};
use db::*;

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

/// Why a join was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinError {
    WrongPassword,
    UsernameTaken,
    EmptyUsername,
    UsernameTooLong,
    /// Invisible or text-direction characters, which let one name pass for
    /// another (see [`messenger_core::util::has_hidden_characters`]).
    HiddenCharacters,
}

impl JoinError {
    pub fn reason(&self) -> &'static str {
        match self {
            JoinError::WrongPassword => "incorrect server password",
            JoinError::UsernameTaken => "username already taken",
            JoinError::EmptyUsername => "username must not be empty",
            JoinError::UsernameTooLong => "username is too long (max 32 characters)",
            JoinError::HiddenCharacters => {
                "username contains invisible or text-direction characters"
            }
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
    // A blank message is stored, broadcast and shown as an empty bubble to
    // everyone; no client sends one on purpose.
    if text.trim().is_empty() {
        return Err("message is empty".to_string());
    }
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
    /// The content address, accumulated as chunks arrive.
    ///
    /// Hashing here rather than at the end is what lets `begin_upload` know
    /// whether the server already holds this exact content *while it still has
    /// the lock*, and therefore whether it needs to reserve storage at all. The
    /// alternative — hashing in one pass later — forces a pessimistic
    /// reservation, which refuses a deduplicated re-upload at a full ceiling.
    /// Each chunk is at most `PARTY_CHUNK_BYTES`, so this adds microseconds to
    /// work already being done under the lock.
    hasher: BlobHasher,
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
        configure_connection(&conn)?;
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
        if messenger_core::util::has_hidden_characters(username) {
            return Err(JoinError::HiddenCharacters);
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
        self.with_tx(|s| s.set_role_in_tx(actor, target, role))
    }

    fn set_role_in_tx(&mut self, actor: Uuid, target: Uuid, role: Role) -> Result<String, String> {
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
        if messenger_core::util::has_hidden_characters(name) {
            return Err("channel name contains invisible or text-direction characters".to_string());
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
        members: Vec<Uuid>,
        now: Instant,
    ) -> Result<ChannelInfo, String> {
        self.with_tx(|s| s.create_channel_of_kind_in_tx(creator, name, kind, members, now))
    }

    fn create_channel_of_kind_in_tx(
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
        members: Vec<Uuid>,
    ) -> Result<String, String> {
        self.with_tx(|s| s.set_channel_access_in_tx(actor, channel, kind, members))
    }

    fn set_channel_access_in_tx(
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
        // One transaction: the blob refcounts released below and the rows
        // removed with the channel must not be able to disagree after a crash.
        self.with_tx(|s| s.delete_channel_in_tx(actor, channel))
    }

    fn delete_channel_in_tx(&mut self, actor: Uuid, channel: Uuid) -> Result<String, String> {
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
}
