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

use messenger_core::party::{
    blob_hash, ChannelInfo, ChannelKind, Envelope, FileMeta, MemberInfo, MessagePayload, TrustTier,
    MAX_HISTORY_BATCH, MAX_INLINE_FILE_BYTES,
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
/// Most channels one server will hold. Channel creation is open to every joined
/// member and there is no admin role, so this is the only thing standing between
/// a bored member and unbounded server state.
const MAX_CHANNELS: usize = 128;
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
}

struct Channel {
    id: Uuid,
    name: String,
    kind: ChannelKind,
    /// Durable history; index order is delivery order. `seq` is `index + 1`.
    messages: Vec<Envelope>,
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
    /// Ceiling on the total bytes of distinct blobs the store will hold.
    max_blob_bytes: u64,
    /// When present, mutations are mirrored to this database.
    db: Option<Connection>,
    /// When present, blob bytes are mirrored to files under this directory.
    blob_dir: Option<PathBuf>,
}

impl PartyState {
    /// Create a new in-memory Administered server with a default `general` channel
    /// and no durable backing store. Mutations are not persisted.
    pub fn new(name: impl Into<String>, password: Option<String>) -> Self {
        let general = Channel {
            id: Uuid::new_v4(),
            name: "general".to_string(),
            kind: ChannelKind::Public,
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
            max_blob_bytes: MAX_TOTAL_BLOB_BYTES,
            db: None,
            blob_dir: None,
        }
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
        state.blob_dir = Some(blob_dir);

        if state.count_channels()? > 0 {
            // The database is authoritative; rebuild the in-memory model from it.
            state.reload_from_db()?;
        } else {
            // Fresh database: persist the seeded default `general` channel.
            let general = &state.channels[0];
            insert_channel_row(
                state.db.as_ref().unwrap(),
                general.id,
                &general.name,
                general.kind,
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
        let member = Member {
            id,
            username: username.to_string(),
            fingerprint,
            online: true,
        };
        self.persist_member(&member);
        self.members.insert(id, member);
        Ok(id)
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

    /// The member directory, sorted by username for stable output.
    pub fn members(&self) -> Vec<MemberInfo> {
        let mut list: Vec<MemberInfo> = self
            .members
            .values()
            .map(|m| MemberInfo {
                id: m.id,
                username: m.username.clone(),
                online: m.online,
            })
            .collect();
        list.sort_by(|a, b| a.username.cmp(&b.username));
        list
    }

    pub fn channels(&self) -> Vec<ChannelInfo> {
        self.channels
            .iter()
            .map(|c| ChannelInfo {
                id: c.id,
                name: c.name.clone(),
                kind: c.kind,
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
            messages: Vec::new(),
        };
        let info = ChannelInfo {
            id: channel.id,
            name: channel.name.clone(),
            kind: channel.kind,
        };
        self.persist_channel(&channel, position);
        self.channels.push(channel);
        Ok(info)
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
    fn member_can_read_channel(&self, channel: Uuid) -> bool {
        match self.channel(channel).map(|c| c.kind) {
            Some(ChannelKind::Public) | Some(ChannelKind::Locked) | Some(ChannelKind::Announce) => {
                true
            }
            // No membership model exists for these yet: deny.
            Some(ChannelKind::Private) => false,
            None => false,
        }
    }

    /// Whether a member may *post* to `channel`. `Locked` and `Announce`
    /// channels are readable but not writable by ordinary members, and there is
    /// no administrator role yet, so nobody may post to them.
    fn member_can_post_to_channel(&self, channel: Uuid) -> Result<(), String> {
        match self.channel(channel).map(|c| c.kind) {
            Some(ChannelKind::Public) => Ok(()),
            Some(ChannelKind::Locked) => Err("this channel is locked".to_string()),
            Some(ChannelKind::Announce) => {
                Err("only announcements may be posted to this channel".to_string())
            }
            Some(ChannelKind::Private) => Err("you are not a member of this channel".to_string()),
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
        self.member_can_post_to_channel(channel)?;
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
        if !self.is_member(member) || !self.member_can_read_channel(channel) {
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

    /// Store `data` as a content-addressed blob (deduplicated by hash, with the
    /// bytes mirrored to disk) and return its [`FileMeta`]. A repeated upload of the
    /// same content reuses the existing blob and bumps its reference count. A new,
    /// distinct blob is rejected if it would push total stored bytes past the
    /// configured ceiling.
    fn store_blob(&mut self, name: &str, mime: &str, data: Vec<u8>) -> Result<FileMeta, String> {
        let hash = blob_hash(&data);
        let size = data.len() as u64;
        if let Some(rec) = self.blobs.get_mut(&hash) {
            rec.refcount += 1;
            let refcount = rec.refcount;
            self.persist_blob_refcount(&hash, refcount);
        } else {
            // Deduplicated re-uploads above never grow storage; only a distinct
            // new blob counts against the ceiling.
            let stored: u64 = self.blobs.values().map(|r| r.size).sum();
            if stored.saturating_add(size) > self.max_blob_bytes {
                return Err("server file storage is full".to_string());
            }
            self.write_blob_file(&hash, &data);
            self.persist_blob_row(&hash, size, mime, 1);
            // Only keep the bytes resident when there is nowhere to read them
            // back from; a disk-backed store reads on demand.
            let resident = if self.blob_dir.is_some() {
                None
            } else {
                Some(data)
            };
            self.blobs.insert(
                hash.clone(),
                BlobRecord {
                    size,
                    mime: mime.to_string(),
                    data: resident,
                    refcount: 1,
                },
            );
        }
        Ok(FileMeta {
            hash,
            // The display name is member-supplied: reduce it to a safe filename
            // here (the single choke point for channel and DM uploads) so no
            // client ever receives a name that could escape its download
            // directory (e.g. `..\..\evil.exe`). P2P transfers get the same
            // treatment at protocol decode.
            name: sanitize_filename(name),
            size,
            mime: mime.to_string(),
        })
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
        if !self.is_member(sender) {
            return Err("sender is not a member of this server".to_string());
        }
        self.member_can_post_to_channel(channel)?;
        let data = Self::check_inline_size(data)?;
        let tier = self.tier;
        let meta = self.store_blob(&name, &mime, data)?;
        // Re-borrow the channel after the blob store to append the message.
        let chan = self
            .channel_mut(channel)
            .expect("channel existence checked");
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
        if !self.is_member(from) {
            return Err("sender is not a member of this server".to_string());
        }
        if !self.is_member(to) {
            return Err("recipient is not a member of this server".to_string());
        }
        let data = Self::check_inline_size(data)?;
        let thread_id = messenger_core::party::dm_thread_id(from, to);
        let tier = self.tier;
        let meta = self.store_blob(&name, &mime, data)?;
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
        match std::fs::read(dir.join(hash)) {
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
    fn member_can_access_blob(&self, member: Uuid, hash: &str) -> bool {
        let references =
            |env: &Envelope| matches!(&env.payload, MessagePayload::File(f) if f.hash == hash);
        // Channels the member is allowed to read. This used to scan *every*
        // channel while claiming to check only public ones, so a file posted in
        // a non-public channel was downloadable by anyone who knew its hash.
        if self
            .channels
            .iter()
            .filter(|c| self.member_can_read_channel(c.id))
            .any(|c| c.messages.iter().any(references))
        {
            return true;
        }
        // Otherwise only DM threads this member participates in (the thread id is
        // derived from the two members' ids).
        self.members.keys().any(|other| {
            let tid = messenger_core::party::dm_thread_id(member, *other);
            self.dm_threads
                .get(&tid)
                .is_some_and(|t| t.messages.iter().any(references))
        })
    }

    /// The bytes of a stored blob, but only when `member` is permitted to see it.
    /// Returns `None` both when the blob is unknown and when access is denied, so
    /// the endpoint never reveals the existence of a file the member can't access.
    pub fn blob_bytes_for(&self, member: Uuid, hash: &str) -> Option<Vec<u8>> {
        if !self.member_can_access_blob(member, hash) {
            return None;
        }
        self.blob_bytes(hash)
    }

    // --- Durable mirroring (best-effort: failures are logged, not propagated, so a
    // transient disk error never drops a live request) --------------------------

    fn persist_member(&self, m: &Member) {
        let Some(conn) = &self.db else { return };
        if let Err(e) = insert_member_row(conn, m.id, &m.username, m.fingerprint.as_deref()) {
            tracing::error!(error = %e, "failed to persist party member");
        }
    }

    fn persist_channel(&self, c: &Channel, position: usize) {
        let Some(conn) = &self.db else { return };
        if let Err(e) = insert_channel_row(conn, c.id, &c.name, c.kind, position) {
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

    fn write_blob_file(&self, hash: &str, data: &[u8]) {
        let Some(dir) = &self.blob_dir else { return };
        let path = dir.join(hash);
        if let Err(e) = std::fs::write(&path, data) {
            tracing::error!(error = %e, path = %path.display(), "failed to write file blob");
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
            let mut stmt = conn.prepare("SELECT id, username, fingerprint FROM members")?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })?;
            for row in rows {
                let (id, username, fingerprint) = row?;
                let id = Uuid::parse_str(&id)?;
                members.insert(
                    id,
                    Member {
                        id,
                        username,
                        fingerprint,
                        online: false,
                    },
                );
            }
        }

        let mut channels = Vec::new();
        {
            let mut stmt =
                conn.prepare("SELECT id, name, kind FROM channels ORDER BY position ASC")?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            let raw: Vec<(String, String, String)> = rows.collect::<Result<_, _>>()?;
            for (id, name, kind) in raw {
                let id = Uuid::parse_str(&id)?;
                let kind: ChannelKind = serde_json::from_str(&kind)?;
                let messages = load_messages(conn, "messages", "channel_id", id)?;
                channels.push(Channel {
                    id,
                    name,
                    kind,
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

        self.members = members;
        self.channels = channels;
        self.dm_threads = dm_threads;
        self.blobs = blobs;
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
         );",
    )
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
    Ok(table_count(conn, "members")? == 0
        && table_count(conn, "channels")? == 0
        && table_count(conn, "dm_threads")? == 0)
}

fn table_count(conn: &Connection, table: &str) -> rusqlite::Result<i64> {
    // `table` is a compile-time constant from this module, never user input.
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
}

impl PartyState {
    fn count_channels(&self) -> rusqlite::Result<i64> {
        match &self.db {
            Some(conn) => table_count(conn, "channels"),
            None => Ok(0),
        }
    }
}

fn insert_member_row(
    conn: &Connection,
    id: Uuid,
    username: &str,
    fingerprint: Option<&str>,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO members (id, username, fingerprint) VALUES (?1, ?2, ?3)",
        params![id.to_string(), username, fingerprint],
    )?;
    Ok(())
}

fn insert_channel_row(
    conn: &Connection,
    id: Uuid,
    name: &str,
    kind: ChannelKind,
    position: usize,
) -> rusqlite::Result<()> {
    let kind = serde_json::to_string(&kind).expect("ChannelKind serializes");
    conn.execute(
        "INSERT OR REPLACE INTO channels (id, name, kind, position) VALUES (?1, ?2, ?3, ?4)",
        params![id.to_string(), name, kind, position as i64],
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
        insert_member_row(&tx, m.id, &m.username, m.fingerprint.as_deref())?;
    }
    for (position, c) in snap.channels.iter().enumerate() {
        insert_channel_row(&tx, c.id, &c.name, c.kind, position)?;
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

    /// `ChannelKind` was stored, shipped to clients, and enforced nowhere.
    /// Anything that is not `Public` now fails closed until a real per-channel
    /// membership model exists.
    #[test]
    fn non_public_channels_are_enforced_rather_than_decorative() {
        let mut state = PartyState::new("Open", None);
        let alice = state.join("alice", None, None).unwrap();
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
                messages: Vec::new(),
            });
            assert!(
                state.post_message(alice, id, "hi".to_string()).is_err(),
                "{label}: an ordinary member must not be able to post"
            );
            assert!(
                state
                    .post_file(alice, id, "f".into(), "text/plain".into(), b"x".to_vec())
                    .is_err(),
                "{label}: an ordinary member must not be able to upload"
            );
        }

        // Private channels are not readable either, and say nothing about
        // whether they exist.
        let private = state
            .channels
            .iter()
            .find(|c| c.kind == ChannelKind::Private)
            .unwrap()
            .id;
        assert!(state.history_since(alice, private, 0).is_empty());
        // The public channel is unaffected.
        assert!(state.post_message(alice, public, "hi".to_string()).is_ok());
        assert_eq!(state.history_since(alice, public, 0).len(), 1);
    }

    /// A file posted in a channel the member cannot read must not be reachable
    /// by content hash either — the download endpoint is where access is
    /// enforced, because blob storage is shared across the whole server.
    #[test]
    fn blobs_in_unreadable_channels_are_not_downloadable() {
        let mut state = PartyState::new("Open", None);
        let alice = state.join("alice", None, None).unwrap();
        let chan = state.default_channel();
        let env = state
            .post_file(
                alice,
                chan,
                "secret.txt".into(),
                "text/plain".into(),
                b"classified".to_vec(),
            )
            .unwrap();
        let hash = file_payload(&env).hash.clone();
        assert!(state.blob_bytes_for(alice, &hash).is_some());

        // Flip the channel to Private: the same member can no longer reach it.
        state.channel_mut(chan).unwrap().kind = ChannelKind::Private;
        assert!(
            state.blob_bytes_for(alice, &hash).is_none(),
            "a file in an unreadable channel must not be downloadable by hash"
        );
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
