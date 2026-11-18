//! In-memory Party server state plus its durable backing store.
//!
//! The authoritative runtime model: members, channels, and durable per-channel
//! history (a reconnecting member fetches history after the last seq it saw).
//! Network-free, so it is unit-tested deterministically; the TCP runtime drives it.
//!
//! State lives in memory and every mutation writes its delta to an embedded
//! **SQLite** database (`party.db`). [`PartyState::new`] is memory-only (tests);
//! [`PartyState::load`] opens the database, imports a legacy `party_state.json`
//! snapshot once, and rebuilds the model from the tables.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use messenger_core::party::{
    blob_hash, encoded_len, take_history_page, AuditEntry, BlobHasher, ChannelInfo, ChannelKind,
    Envelope, FileEntry, FileMeta, FilePermissions, MemberInfo, MessagePayload, QuotaInfo, Role,
    TrustTier, UploadTarget, MAX_INLINE_FILE_BYTES, MAX_PARTY_FILE_BYTES, PARTY_CHUNK_BYTES,
};
use messenger_core::util::{current_timestamp_millis, sanitize_filename};
use rusqlite::{params, Connection};
use serde::Deserialize;
use uuid::Uuid;

mod blobs;
mod db;
mod history;
#[cfg(test)]
mod tests;
mod write_rate;

use blobs::*;
pub use blobs::{BlobReadPlan, StagedUpload};
use db::*;
use history::{History, RESIDENT_HISTORY};
use write_rate::WriteBuckets;
pub use write_rate::{WriteRateLimit, DEFAULT_WRITE_RATE, WRITE_RATE_REFUSAL};

/// Filename of the embedded SQLite database under the operator's data dir.
const DB_FILE: &str = "party.db";
/// Subdirectory under the data dir holding content-addressed file blobs.
const BLOB_DIR: &str = "blobs";
/// Default ceiling on the total bytes of distinct blobs the server stores
/// (adjustable via [`PartyState::set_max_blob_bytes`]).
const MAX_TOTAL_BLOB_BYTES: u64 = 1024 * 1024 * 1024; // 1 GiB
/// Default per-member ceiling on distinct uploaded bytes, so the first member
/// to fill the server-wide ceiling cannot deny the feature to everyone else.
/// Admins are exempt — they are the ones who clean up.
const MAX_MEMBER_BLOB_BYTES: u64 = 128 * 1024 * 1024; // 128 MiB
/// Most audit entries retained in memory and returned to an admin.
const MAX_AUDIT_ENTRIES: usize = 1000;
/// Most channels one server will hold (any member may create a public one).
const MAX_CHANNELS: usize = 128;
/// How many channels one member may create within [`CHANNEL_CREATE_WINDOW`],
/// so one member cannot walk the server to [`MAX_CHANNELS`] in a burst.
/// Generous: setting a community up genuinely means making a few at once.
const MAX_CHANNELS_PER_MEMBER: usize = 5;
/// The sliding window [`MAX_CHANNELS_PER_MEMBER`] is counted over.
const CHANNEL_CREATE_WINDOW: Duration = Duration::from_secs(60);

/// Filename of the legacy JSON snapshot, imported once into SQLite if present.
const LEGACY_SNAPSHOT_FILE: &str = "party_state.json";

/// Maximum length (in Unicode scalar values) of a member username — a display
/// handle shown in every directory, not free text.
pub const MAX_USERNAME_CHARS: usize = 32;
/// Maximum length (in Unicode scalar values) of a channel name.
pub const MAX_CHANNEL_NAME_CHARS: usize = 64;
/// Maximum size (in bytes) of a channel or DM message's text, matching the P2P
/// cap (`messenger_core::MAX_TEXT_MESSAGE_BYTES`).
pub const MAX_MESSAGE_TEXT_BYTES: usize = messenger_core::MAX_TEXT_MESSAGE_BYTES;

/// Most members one server will register, so the `Members` directory (one
/// frame, ~165 bytes per member) stays inside [`MAX_PARTY_PAYLOAD_BYTES`].
pub const MAX_MEMBERS: usize = 10_000;
/// Most members one private channel may list (every list travels in the one
/// `Channels` frame; 128 × 1,000 members is about 3 MiB).
pub const MAX_PRIVATE_CHANNEL_MEMBERS: usize = 1_000;
/// Longest MIME type the server stores, in bytes (RFC 6838 caps each half at
/// 127). It is copied into every file message, page and listing, so an
/// unbounded one could make them all unsendable.
pub const MAX_MIME_BYTES: usize = 255;
/// What a missing or malformed MIME type is stored as.
pub const FALLBACK_MIME: &str = "application/octet-stream";
/// Listing limits, shared with the client (it pages on when a reply is full).
pub use messenger_core::party::{FILE_LISTING_BYTES, MAX_FILE_LISTING};

/// Reduce a member-supplied MIME type to `type/subtype`, lowercase, or
/// [`FALLBACK_MIME`]. Parameters are dropped; the type is display metadata
/// only, so normalising beats refusing an upload over an odd guess.
pub fn normalize_mime(mime: &str) -> String {
    let essence = mime.split(';').next().unwrap_or("").trim();
    let restricted = |part: &str| {
        !part.is_empty()
            && part.len() <= 127
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&-^_.+".contains(&b))
    };
    match essence.split_once('/') {
        Some((ty, sub)) if essence.len() <= MAX_MIME_BYTES && restricted(ty) && restricted(sub) => {
            essence.to_ascii_lowercase()
        }
        _ => FALLBACK_MIME.to_string(),
    }
}

/// Keep the leading entries of `items` that fit both `max_items` and
/// `max_bytes` of encoded size, dropping the rest.
fn truncate_listing<T: serde::Serialize>(items: &mut Vec<T>, max_items: usize, max_bytes: usize) {
    let mut used = 0usize;
    let mut keep = 0usize;
    for item in items.iter().take(max_items) {
        let size = encoded_len(item);
        if used.saturating_add(size) > max_bytes {
            break;
        }
        used += size;
        keep += 1;
    }
    items.truncate(keep);
}

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
    /// The server already has [`MAX_MEMBERS`] members.
    ServerFull,
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
            JoinError::ServerFull => "this server has reached its member limit",
        }
    }
}

/// Constant-time string equality (an early length check leaks nothing a
/// timing observer could not already see).
fn ct_eq(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

/// Compare a join attempt's password against the server's in constant time,
/// so response timing cannot leak the secret a character at a time.
fn password_matches(expected: Option<&str>, supplied: Option<&str>) -> bool {
    match (expected, supplied) {
        (None, None) => true,
        (Some(expected), Some(supplied)) => ct_eq(expected, supplied),
        // An open server handed a password, or a protected one handed none.
        _ => false,
    }
}

/// A fresh one-time owner claim token: 122 random bits as 32 hex characters.
fn new_owner_claim_token() -> String {
    Uuid::new_v4().simple().to_string()
}

/// Reject a blank message, or one whose text exceeds [`MAX_MESSAGE_TEXT_BYTES`]
/// (channel posts and DMs alike).
fn validate_message_text(text: &str) -> Result<(), String> {
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
    /// Authority on this server. Only the operator becomes [`Role::Owner`]:
    /// by joining with the owner claim token, or from the configured owner
    /// fingerprint (see [`PartyState::owner_claim_token`]).
    role: Role,
}

struct Channel {
    id: Uuid,
    name: String,
    kind: ChannelKind,
    /// Explicit membership, consulted only for [`ChannelKind::Private`].
    members: Vec<Uuid>,
    /// Durable history: the newest envelopes in memory, all of them in the
    /// database. See [`History`].
    history: History,
}
impl Channel {
    fn info(&self) -> ChannelInfo {
        ChannelInfo {
            id: self.id,
            name: self.name.clone(),
            kind: self.kind,
            members: self.members.clone(),
        }
    }
}

/// One share of a file into a channel or DM thread — the provenance record the
/// Drive panel lists and `DeleteFile` removes. Separate from the message, which
/// stays put on delete (clients merge history on sequence numbers).
#[derive(Clone)]
struct FileRef {
    /// Identity of this *share*, not of the content: keying on
    /// `(hash, location)` collapsed repeated shares into one row while the
    /// refcount counted them all, leaking the blob after a restart.
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

/// A chunked upload in flight, spooled in memory until `FinishUpload` (bounded
/// by [`MAX_PARTY_FILE_BYTES`] × [`MAX_CONCURRENT_UPLOADS`]).
struct PendingUpload {
    uploader: Uuid,
    name: String,
    mime: String,
    /// Size the uploader declared, checked before a byte was accepted.
    declared: u64,
    target: UploadTarget,
    data: Vec<u8>,
    /// The content address, accumulated as chunks arrive, so `begin_upload`
    /// knows under the lock whether the content is already held (and needs no
    /// reservation) without a 100 MiB pass.
    hasher: BlobHasher,
}

/// Uploads one member may have in flight at once (each up to
/// [`MAX_PARTY_FILE_BYTES`]).
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
    history: History,
}

/// A content-addressed file blob, deduplicated by hash and reference-counted
/// (one count per share); on disk under `<data_dir>/blobs/<hash>`.
struct BlobRecord {
    size: u64,
    mime: String,
    /// The bytes, resident **only** without a blob directory (the in-memory
    /// mode tests use); a disk-backed server reads on demand.
    data: Option<Vec<u8>>,
    refcount: u32,
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
    /// Storage claimed by uploads being written but not yet recorded, keyed by
    /// staging id, so the ceilings bound peak disk and not only stored bytes.
    reservations: HashMap<Uuid, Reservation>,
    /// Per-member ceiling on distinct uploaded bytes. Admins are exempt.
    max_member_blob_bytes: u64,
    /// Ceiling on the total bytes of distinct blobs the store will hold.
    max_blob_bytes: u64,
    /// Most members this server registers (see [`MAX_MEMBERS`]).
    max_members: usize,
    /// How many of each thread's newest envelopes stay in memory when there is
    /// a database to page older ones from (see [`History`]).
    resident_history: usize,
    /// The fingerprint of the identity that owns this community, when the
    /// operator configured one (see [`Self::set_owner_fingerprint`]).
    owner_fingerprint: Option<String>,
    /// The pending one-time owner claim (see [`Self::owner_claim_token`]).
    owner_claim: Option<String>,
    /// When present, mutations are mirrored to this database.
    db: Option<Connection>,
    /// When present, blob bytes are mirrored to files under this directory.
    blob_dir: Option<PathBuf>,
    /// Recent channel creations per member, for the burst limit. In memory
    /// only: a restart forgiving it costs nothing.
    channel_creations: HashMap<Uuid, Vec<Instant>>,
    /// Per-member write budgets (see [`WriteRateLimit`]).
    write_rate: WriteBuckets,
    /// How long messages and shared files are kept. `None` keeps everything
    /// (see [`Self::prune_expired`]).
    retention: Option<std::time::Duration>,
}

/// What one retention pass removed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PruneReport {
    /// Messages deleted from the database (or from memory, without one).
    pub messages: usize,
    /// File shares removed; their bytes go once nothing else references them.
    pub files: usize,
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
            history: History::new(),
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
            max_members: MAX_MEMBERS,
            resident_history: RESIDENT_HISTORY,
            owner_fingerprint: None,
            // A fresh server has no members, so no Owner: it needs a claim.
            owner_claim: Some(new_owner_claim_token()),
            db: None,
            blob_dir: None,
            channel_creations: HashMap::new(),
            write_rate: WriteBuckets::default(),
            retention: None,
        }
    }

    /// Override the per-member storage allowance (bytes of distinct content).
    pub fn set_max_member_blob_bytes(&mut self, bytes: u64) {
        self.max_member_blob_bytes = bytes;
    }

    /// Override how many of each thread's newest envelopes stay in memory on
    /// a disk-backed server. Older ones are read from the database on demand.
    pub fn set_resident_history(&mut self, envelopes: usize) {
        self.resident_history = envelopes.max(1);
    }

    /// How many envelopes a thread may keep in memory, or `None` (all of
    /// them) when there is no database to page the rest from.
    fn history_cap(&self) -> Option<usize> {
        self.db.as_ref().map(|_| self.resident_history)
    }

    /// Replace the per-member write budget; `None` turns it off (for an
    /// in-process caller or a test that drives thousands of posts).
    pub fn set_write_rate_limit(&mut self, limit: Option<WriteRateLimit>) {
        self.write_rate.set_limit(limit);
    }

    /// Spend one write carrying `text_bytes` from `member`'s budget, or refuse
    /// it. The dispatcher calls this before every mutating request; the state
    /// methods themselves are not limited, so in-process callers are not.
    pub fn check_write_rate(&mut self, member: Uuid, text_bytes: usize) -> Result<(), String> {
        self.check_write_rate_at(member, text_bytes, Instant::now())
    }

    /// [`Self::check_write_rate`] against a caller-supplied clock.
    pub fn check_write_rate_at(
        &mut self,
        member: Uuid,
        text_bytes: usize,
        now: Instant,
    ) -> Result<(), String> {
        self.write_rate.check(member, text_bytes, now)
    }

    /// Override the member ceiling. It may only be lowered: [`MAX_MEMBERS`] is
    /// what keeps the directory inside one frame.
    pub fn set_max_members(&mut self, members: usize) {
        self.max_members = members.min(MAX_MEMBERS);
    }

    /// Override the total-blob-storage ceiling (bytes of distinct content). Used
    /// to configure the quota; defaults to [`MAX_TOTAL_BLOB_BYTES`].
    pub fn set_max_blob_bytes(&mut self, bytes: u64) {
        self.max_blob_bytes = bytes;
    }

    /// Open (creating if needed) the durable state in `<data_dir>/party.db` and
    /// rebuild the in-memory model from it. A fresh database is seeded with
    /// `general`; a legacy `party_state.json` is imported into an empty one and
    /// renamed `party_state.json.imported`.
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

        // Sweep staging files a killed process left behind (`Drop` only runs
        // in a live one). No content hash starts with `staging_`.
        sweep_staging_files(&blob_dir);

        if state.count_channels()? > 0 {
            // The database is authoritative; rebuild the in-memory model from it.
            state.reload_from_db()?;
            // A community with an Owner needs no claim; one without keeps the
            // token `new` issued (never promote an arbitrary member).
            state.refresh_owner_claim();
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
    /// returning identity (matched by its verified fingerprint) or registers a
    /// new member with a unique, non-empty username. Returns the member id,
    /// marked online.
    ///
    /// Being first grants nothing: ownership goes only to a join presenting the
    /// owner claim token ([`Self::owner_claim_token`]) or from the identity set
    /// with [`Self::set_owner_fingerprint`].
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
        // The claim token stands in for the password: it is a stronger secret
        // than any password, and the operator needs no separate field for it.
        let claims_ownership = self.presents_owner_claim(password);
        if !claims_ownership && !password_matches(self.password.as_deref(), password) {
            return Err(JoinError::WrongPassword);
        }

        // Members are never removed on disconnect, so a reconnecting identity
        // reuses its membership instead of hitting its own "username taken".
        let returning = fingerprint.as_deref().and_then(|fp| {
            self.members
                .values()
                .find(|m| m.fingerprint.as_deref() == Some(fp))
                .map(|m| m.id)
        });
        let id = match returning {
            Some(id) => {
                if let Some(m) = self.members.get_mut(&id) {
                    m.online = true;
                }
                id
            }
            None => {
                // A returning member above is always let back in; only a new
                // one is refused at the ceiling.
                if self.members.len() >= self.max_members {
                    return Err(JoinError::ServerFull);
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
                let member = Member {
                    id,
                    username: username.to_string(),
                    fingerprint,
                    online: true,
                    role: Role::Member,
                };
                self.persist_member(&member);
                self.members.insert(id, member);
                id
            }
        };

        if claims_ownership {
            self.with_tx(|s| {
                s.make_owner(
                    id,
                    "role.claim",
                    "claimed ownership with the owner claim token",
                )
            });
        } else {
            self.with_tx(|s| s.apply_configured_owner());
        }
        Ok(id)
    }

    /// Whether `password` is the pending owner claim token (constant-time).
    fn presents_owner_claim(&self, password: Option<&str>) -> bool {
        matches!(
            (self.owner_claim.as_deref(), password.map(str::trim)),
            (Some(token), Some(supplied)) if ct_eq(token, supplied)
        )
    }

    /// Whether this community has an Owner.
    pub fn has_owner(&self) -> bool {
        self.members.values().any(|m| m.role == Role::Owner)
    }

    /// The one-time token that makes whoever joins with it the Owner, while the
    /// community has no Owner and no configured owner fingerprint. The operator
    /// gets it at startup and enters it as the password; it is never persisted
    /// and disappears once an Owner exists.
    pub fn owner_claim_token(&self) -> Option<&str> {
        self.owner_claim.as_deref()
    }

    /// Name the identity that owns this community by fingerprint (`None`
    /// clears it). That member becomes Owner on join — or now — and any other
    /// Owner moves to Admin: the operator's recovery path, since whoever
    /// controls the configuration already controls the data.
    pub fn set_owner_fingerprint(&mut self, fingerprint: Option<String>) {
        self.owner_fingerprint = fingerprint
            .map(|fp| messenger_core::util::canonical_fingerprint(&fp))
            .filter(|fp| !fp.is_empty());
        self.with_tx(|s| s.apply_configured_owner());
        self.refresh_owner_claim();
    }

    /// Promote the configured owner if they are a member and not yet Owner.
    fn apply_configured_owner(&mut self) {
        let Some(fp) = self.owner_fingerprint.clone() else {
            return;
        };
        let Some(id) = self
            .members
            .values()
            .find(|m| m.fingerprint.as_deref() == Some(fp.as_str()))
            .map(|m| m.id)
        else {
            return;
        };
        if self.role_of(id) != Role::Owner {
            self.make_owner(
                id,
                "role.configured",
                "is the owner configured by the operator",
            );
        }
    }

    /// Make `id` the Owner, moving any other Owner to Admin (a server has
    /// exactly one), and record it.
    fn make_owner(&mut self, id: Uuid, action: &str, why: &str) {
        let previous: Vec<Uuid> = self
            .members
            .values()
            .filter(|m| m.role == Role::Owner && m.id != id)
            .map(|m| m.id)
            .collect();
        for other in previous {
            if let Some(name) = self.store_role(other, Role::Admin) {
                let detail = format!("{name} is now Admin (ownership moved)");
                self.record_audit(id, "role.owner_moved", &detail);
            }
        }
        let Some(name) = self.store_role(id, Role::Owner) else {
            return;
        };
        tracing::info!(member = %id, username = %name, "{name} {why}");
        self.record_audit(id, action, &format!("{name} {why}"));
        self.refresh_owner_claim();
    }

    /// Set and persist a member's role, returning their name.
    fn store_role(&mut self, id: Uuid, role: Role) -> Option<String> {
        let member = self.members.get_mut(&id)?;
        member.role = role;
        let name = member.username.clone();
        self.persist_member(&self.members[&id]);
        Some(name)
    }

    /// Issue a claim token when the community needs one (no Owner, no
    /// configured owner), and drop it when it does not.
    fn refresh_owner_claim(&mut self) {
        let needed = !self.has_owner() && self.owner_fingerprint.is_none();
        match (needed, self.owner_claim.is_some()) {
            (true, false) => self.owner_claim = Some(new_owner_claim_token()),
            (false, true) => self.owner_claim = None,
            _ => {}
        }
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
        let name = self.store_role(target, role).expect("checked above");
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

    /// The audit log, newest first, for admins only.
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
    /// omitted, so a private channel does not advertise its existence.
    pub fn channels_for(&self, member: Uuid) -> Vec<ChannelInfo> {
        let role = self.role_of(member);
        self.channels
            .iter()
            .filter(|c| c.kind.may_read(role, c.members.contains(&member)))
            .map(Channel::info)
            .collect()
    }

    /// Whether every joined member may read `channel` (false for `Private`),
    /// i.e. whether its posts may be broadcast rather than fanned out by member.
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
        self.channels.iter().map(Channel::info).collect()
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
            history: History::new(),
        };
        let info = channel.info();
        self.persist_channel(&channel, position);
        self.channels.push(channel);
        Ok(info)
    }

    /// Refuse a member who has created [`MAX_CHANNELS_PER_MEMBER`] channels
    /// inside [`CHANNEL_CREATE_WINDOW`]. Admins too: a stolen admin session is
    /// exactly the one that can reach the restricted kinds.
    fn check_channel_rate(&mut self, creator: Uuid, now: Instant) -> Result<(), String> {
        // Forget members who have gone quiet, so the map stays bounded.
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
    /// are admin-only. The creator of a private channel is always in it.
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
        members: Vec<Uuid>,
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
        // Both checked before the channel is made, and the rate recorded only
        // once it is, so a refused request costs the member nothing.
        let members = self.channel_member_list(creator, kind, members)?;
        self.check_channel_rate(creator, now)?;
        self.create_channel(name)?;
        self.record_channel_creation(creator, now);
        let position = self.channels.len() - 1;
        if kind != ChannelKind::Public {
            let c = &mut self.channels[position];
            c.kind = kind;
            c.members = members;
            self.persist_channel(&self.channels[position], position);
        }
        let detail = format!("created {} channel #{}", kind.label().to_lowercase(), name);
        self.record_audit(creator, "channel.create", &detail);
        Ok(self.channels[position].info())
    }

    /// The membership list a channel of `kind` will store: for a private
    /// channel, the named members who exist, deduplicated, always including
    /// `actor` and capped at [`MAX_PRIVATE_CHANNEL_MEMBERS`]; otherwise nobody.
    fn channel_member_list(
        &self,
        actor: Uuid,
        kind: ChannelKind,
        mut members: Vec<Uuid>,
    ) -> Result<Vec<Uuid>, String> {
        if kind != ChannelKind::Private {
            return Ok(Vec::new());
        }
        let mut seen = std::collections::HashSet::new();
        members.retain(|m| self.members.contains_key(m) && seen.insert(*m));
        if !members.contains(&actor) {
            members.push(actor);
        }
        if members.len() > MAX_PRIVATE_CHANNEL_MEMBERS {
            return Err(format!(
                "a private channel may have at most {MAX_PRIVATE_CHANNEL_MEMBERS} members"
            ));
        }
        Ok(members)
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
        members: Vec<Uuid>,
    ) -> Result<String, String> {
        if !self.role_of(actor).is_admin() {
            return Err("only admins may change channel access".to_string());
        }
        let members = self.channel_member_list(actor, kind, members)?;
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
        // Release every file shared here, or its bytes are stranded.
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

    /// Whether a member may *read* `channel` (its history and files). Fails
    /// closed for an unknown channel.
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
        self.append_channel_message(sender, channel, MessagePayload::Text(text))
            .ok_or_else(|| "unknown channel".to_string())
    }

    /// How long the community keeps messages and shared files (`--retain-days`,
    /// audit SRV-06); `None` keeps everything. Applied by [`Self::prune_expired`].
    pub fn set_retention(&mut self, max_age: Option<std::time::Duration>) {
        self.retention = max_age;
    }

    /// Delete messages and file shares older than the retention period.
    ///
    /// Each thread keeps its newest message whatever its age, so numbering
    /// continues (clients merge on seq). Expired files are released like a
    /// delete, so their bytes go with the last reference.
    pub fn prune_expired(&mut self, now_ms: u64) -> PruneReport {
        let Some(max_age) = self.retention else {
            return PruneReport::default();
        };
        let cutoff = now_ms.saturating_sub(max_age.as_millis().min(u64::MAX as u128) as u64);
        self.with_tx(|s| s.prune_expired_in_tx(cutoff))
    }

    fn prune_expired_in_tx(&mut self, cutoff: u64) -> PruneReport {
        let mut report = PruneReport::default();
        let mut resident_pruned = 0;
        for chan in &mut self.channels {
            resident_pruned += chan.history.prune_before(cutoff);
        }
        for thread in self.dm_threads.values_mut() {
            resident_pruned += thread.history.prune_before(cutoff);
        }
        report.messages = match &self.db {
            Some(conn) => db::prune_messages_before(conn, cutoff).unwrap_or_else(|e| {
                tracing::error!(error = %e, "retention: could not prune party.db messages");
                0
            }),
            None => resident_pruned,
        };

        let expired: Vec<FileRef> = self
            .file_refs
            .iter()
            .filter(|r| r.shared_at < cutoff)
            .cloned()
            .collect();
        for entry in &expired {
            self.file_refs.retain(|r| r.id != entry.id);
            self.delete_file_ref_row(entry.id);
            self.release_blob(&entry.hash);
        }
        report.files = expired.len();

        if report.messages > 0 || report.files > 0 {
            let detail = format!(
                "retention removed {} messages and {} file shares",
                report.messages, report.files
            );
            self.record_audit(Uuid::nil(), "retention.prune", &detail);
        }
        report
    }

    /// Append a message to `channel` under the next sequence number, keep it
    /// in memory (subject to the resident cap) and persist it. `None` for an
    /// unknown channel. Every channel message — text, file, re-share — is
    /// appended here, so numbering has one source.
    fn append_channel_message(
        &mut self,
        sender: Uuid,
        channel: Uuid,
        payload: MessagePayload,
    ) -> Option<Envelope> {
        let (tier, cap) = (self.tier, self.history_cap());
        let chan = self.channel_mut(channel)?;
        let envelope = Envelope {
            tier,
            sender,
            channel,
            seq: chan.history.next_seq(),
            timestamp: current_timestamp_millis(),
            payload,
        };
        chan.history.push(envelope.clone(), cap);
        self.persist_message(&envelope);
        Some(envelope)
    }

    /// The DM twin of [`Self::append_channel_message`], creating the thread
    /// on its first message.
    fn append_dm_message(
        &mut self,
        sender: Uuid,
        thread_id: Uuid,
        payload: MessagePayload,
    ) -> Envelope {
        let (tier, cap) = (self.tier, self.history_cap());
        let thread = self
            .dm_threads
            .entry(thread_id)
            .or_insert_with(|| DmThread {
                id: thread_id,
                history: History::new(),
            });
        let envelope = Envelope {
            tier,
            sender,
            channel: thread_id,
            seq: thread.history.next_seq(),
            timestamp: current_timestamp_millis(),
            payload,
        };
        thread.history.push(envelope.clone(), cap);
        self.persist_dm(thread_id, &envelope);
        envelope
    }

    /// One page of a thread's history after `since_seq`: from memory when the
    /// thread's resident tail covers it, from the database otherwise.
    fn history_page(&self, table: ThreadTable, history: &History, since_seq: u64) -> Vec<Envelope> {
        if !history.covers(since_seq) {
            if let Some(conn) = &self.db {
                match load_page(conn, table, since_seq) {
                    Ok(rows) => return take_history_page(rows.iter()),
                    Err(e) => {
                        // Serve what memory has rather than nothing: the page
                        // starts later than asked, but the client still moves.
                        tracing::error!(error = %e, "could not read a history page from party.db");
                    }
                }
            }
        }
        take_history_page(history.after(since_seq))
    }

    /// Channel history strictly after `since_seq`, one page at a time (bounded
    /// by count **and** bytes — see [`take_history_page`] — so the reply always
    /// fits in one frame); the caller asks again from the last `seq` it got.
    ///
    /// A channel the member may not read yields `[]`, like an unknown one.
    pub fn history_since(&self, member: Uuid, channel: Uuid, since_seq: u64) -> Vec<Envelope> {
        if !self.is_member(member) || !self.member_can_read_channel(member, channel) {
            return Vec::new();
        }
        match self.channel(channel) {
            Some(c) => self.history_page(ThreadTable::Channel(c.id), &c.history, since_seq),
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
        Ok(self.append_dm_message(from, thread_id, MessagePayload::Text(text)))
    }

    /// Direct-message history for a thread strictly after `since_seq`, paged the
    /// same way as [`Self::history_since`] and for the same reason.
    pub fn dm_history(&self, thread_id: Uuid, since_seq: u64) -> Vec<Envelope> {
        match self.dm_threads.get(&thread_id) {
            Some(t) => self.history_page(ThreadTable::Dm(t.id), &t.history, since_seq),
            None => Vec::new(),
        }
    }
}
