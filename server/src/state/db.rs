//! Durable backing store: the SQLite schema, per-mutation row writes, the
//! reload into memory, and the one-time import of the legacy JSON snapshot.

use super::*;

#[derive(Deserialize)]
pub(super) struct LegacyMember {
    pub(super) id: Uuid,
    pub(super) username: String,
    pub(super) fingerprint: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct LegacyChannel {
    pub(super) id: Uuid,
    pub(super) name: String,
    pub(super) kind: ChannelKind,
    pub(super) messages: Vec<Envelope>,
}

#[derive(Deserialize)]
pub(super) struct LegacyDmThread {
    pub(super) id: Uuid,
    pub(super) messages: Vec<Envelope>,
}

#[derive(Deserialize, Default)]
pub(super) struct LegacySnapshot {
    pub(super) members: Vec<LegacyMember>,
    pub(super) channels: Vec<LegacyChannel>,
    #[serde(default)]
    pub(super) dm_threads: Vec<LegacyDmThread>,
}

impl PartyState {
    /// Run one logical mutation as one database transaction.
    ///
    /// Every `persist_*` helper writes through the same connection, so the
    /// rows a mutation touches — a refcount, a file reference, a message, an
    /// audit entry — commit together or not at all. Before this each was its
    /// own autocommit statement: a crash part-way through deleting a channel
    /// left refcounts decremented for references that survived the restart,
    /// and each statement paid its own commit.
    ///
    /// A failed statement is still only logged, exactly as before: the
    /// in-memory model has already moved on, and refusing the request would
    /// not undo that. What this changes is that a crash can no longer leave
    /// half of a mutation on disk. Nests safely: an inner call inside an open
    /// transaction just runs.
    pub(super) fn with_tx<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let began = match &self.db {
            Some(conn) if conn.is_autocommit() => match conn.execute_batch("BEGIN") {
                Ok(()) => true,
                Err(e) => {
                    tracing::error!(error = %e, "could not begin a party.db transaction");
                    false
                }
            },
            _ => false,
        };
        let out = f(self);
        if began {
            if let Some(conn) = &self.db {
                if let Err(e) = conn.execute_batch("COMMIT") {
                    tracing::error!(error = %e, "could not commit a party.db transaction");
                    let _ = conn.execute_batch("ROLLBACK");
                }
            }
        }
        out
    }

    pub(super) fn persist_member(&self, m: &Member) {
        let Some(conn) = &self.db else { return };
        if let Err(e) = insert_member_row(conn, m.id, &m.username, m.fingerprint.as_deref(), m.role)
        {
            tracing::error!(error = %e, "failed to persist party member");
        }
    }

    pub(super) fn persist_file_ref(&self, r: &FileRef) {
        let Some(conn) = &self.db else { return };
        if let Err(e) = execute_cached(
            conn,
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
    pub(super) fn delete_file_ref_row(&self, id: Uuid) {
        let Some(conn) = &self.db else { return };
        if let Err(e) = execute_cached(
            conn,
            "DELETE FROM file_refs WHERE id = ?1",
            params![id.to_string()],
        ) {
            tracing::error!(error = %e, "failed to delete file reference");
        }
    }

    pub(super) fn delete_blob_row(&self, hash: &str) {
        let Some(conn) = &self.db else { return };
        if let Err(e) = execute_cached(conn, "DELETE FROM blobs WHERE hash = ?1", params![hash]) {
            tracing::error!(error = %e, "failed to delete blob row");
        }
    }

    /// Remove a channel's rows — its messages, its file references and the
    /// channel itself — as one transaction.
    ///
    /// Three autocommit statements used to do this, so a crash between them
    /// left a channel with no messages, or messages with no channel.
    pub(super) fn delete_channel_rows(&self, channel: Uuid) {
        let Some(conn) = &self.db else { return };
        let id = channel.to_string();
        let result = atomically(conn, |conn| {
            for sql in [
                "DELETE FROM messages WHERE channel_id = ?1",
                "DELETE FROM file_refs WHERE location = ?1",
                "DELETE FROM channels WHERE id = ?1",
            ] {
                execute_cached(conn, sql, params![id])?;
            }
            Ok(())
        });
        if let Err(e) = result {
            tracing::error!(error = %e, "failed to delete channel rows");
        }
    }

    pub(super) fn persist_audit(&self, r: &AuditRecord) {
        let Some(conn) = &self.db else { return };
        if let Err(e) = execute_cached(
            conn,
            "INSERT INTO audit (at, actor, action, detail) VALUES (?1, ?2, ?3, ?4)",
            params![r.at as i64, r.actor.to_string(), r.action, r.detail],
        ) {
            tracing::error!(error = %e, "failed to persist audit entry");
        }
    }

    pub(super) fn persist_channel(&self, c: &Channel, position: usize) {
        let Some(conn) = &self.db else { return };
        if let Err(e) = insert_channel_row(conn, c.id, &c.name, c.kind, &c.members, position) {
            tracing::error!(error = %e, "failed to persist party channel");
        }
    }

    pub(super) fn persist_message(&self, e: &Envelope) {
        let Some(conn) = &self.db else { return };
        if let Err(e) = insert_message_row(conn, e) {
            tracing::error!(error = %e, "failed to persist party message");
        }
    }

    pub(super) fn persist_dm(&self, thread_id: Uuid, e: &Envelope) {
        let Some(conn) = &self.db else { return };
        if let Err(e) = insert_dm_message_row(conn, thread_id, e) {
            tracing::error!(error = %e, "failed to persist party direct message");
        }
    }

    pub(super) fn persist_blob_row(&self, hash: &str, size: u64, mime: &str, refcount: u32) {
        let Some(conn) = &self.db else { return };
        if let Err(e) = insert_blob_row(conn, hash, size, mime, refcount) {
            tracing::error!(error = %e, "failed to persist file blob row");
        }
    }

    pub(super) fn persist_blob_refcount(&self, hash: &str, refcount: u32) {
        let Some(conn) = &self.db else { return };
        if let Err(e) = execute_cached(
            conn,
            "UPDATE blobs SET refcount = ?1 WHERE hash = ?2",
            params![refcount, hash],
        ) {
            tracing::error!(error = %e, "failed to update file blob refcount");
        }
    }

    /// Rebuild the in-memory model from the database. Presence resets to offline.
    pub(super) fn reload_from_db(&mut self) -> anyhow::Result<()> {
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
                        // An older server stored whatever it was sent.
                        mime: normalize_mime(&mime),
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
pub(super) fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
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
    // Deleting a channel removes its references by location; without an index
    // that is a scan of every share on the server.
    conn.execute(
        "CREATE INDEX IF NOT EXISTS file_refs_location ON file_refs (location)",
        [],
    )?;
    Ok(())
}

/// Tune a freshly opened database for a server that writes on every request.
///
/// SQLite's defaults are a rollback journal with `synchronous=FULL`: three or
/// four fsyncs per autocommit statement, every one of them taken while the
/// global state lock is held. That capped a whole community at roughly a
/// hundred writes a second on good disks and a few dozen on an SD card, and
/// every member's request queued behind each fsync.
///
/// - **WAL** appends to a log instead of rewriting pages through a journal, and
///   lets readers proceed alongside the writer.
/// - **`synchronous=NORMAL`** under WAL syncs at checkpoints rather than on
///   every commit. It is still safe against an application crash; a power cut
///   can lose the last few commits, never corrupt the database.
/// - **`busy_timeout`** waits out a lock held by an external reader (a backup,
///   an operator's `sqlite3` shell) instead of failing the write at once.
/// - The statement cache is sized to hold every statement this module issues,
///   so `prepare_cached` never has to evict and re-parse one.
pub(super) fn configure_connection(conn: &Connection) -> rusqlite::Result<()> {
    let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        // Some filesystems (certain network mounts) cannot hold a WAL. The
        // server still works in the mode SQLite fell back to, just slower.
        tracing::warn!(journal_mode = %mode, "party.db could not switch to WAL mode");
    }
    conn.execute_batch(
        "PRAGMA synchronous = NORMAL;
         PRAGMA busy_timeout = 5000;
         PRAGMA foreign_keys = ON;",
    )?;
    conn.set_prepared_statement_cache_capacity(STATEMENT_CACHE_CAPACITY);
    Ok(())
}

/// Comfortably more than the distinct statements this module prepares.
const STATEMENT_CACHE_CAPACITY: usize = 64;

/// Run one statement through the connection's prepared-statement cache.
///
/// `Connection::execute` re-parses and re-plans its SQL every call; these are a
/// handful of fixed statements issued on every post, so they are compiled once.
pub(super) fn execute_cached<P: rusqlite::Params>(
    conn: &Connection,
    sql: &str,
    params: P,
) -> rusqlite::Result<usize> {
    conn.prepare_cached(sql)?.execute(params)
}

/// Run `f` inside a transaction, unless one is already open.
///
/// Nesting is the point of the check: a helper that is atomic on its own (a DM
/// writes its thread row and its message) is also called from inside a larger
/// mutation that has already begun one, and SQLite refuses a nested `BEGIN`.
/// In that case the outer transaction is what makes it atomic.
pub(super) fn atomically<T>(
    conn: &Connection,
    f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
) -> rusqlite::Result<T> {
    if !conn.is_autocommit() {
        return f(conn);
    }
    let tx = conn.unchecked_transaction()?;
    let out = f(&tx)?;
    tx.commit()?;
    Ok(out)
}

pub(super) fn insert_blob_row(
    conn: &Connection,
    hash: &str,
    size: u64,
    mime: &str,
    refcount: u32,
) -> rusqlite::Result<()> {
    execute_cached(
        conn,
        "INSERT OR REPLACE INTO blobs (hash, size, mime, refcount) VALUES (?1, ?2, ?3, ?4)",
        params![hash, size as i64, mime, refcount],
    )?;
    Ok(())
}

/// True when no members, channels, or DM threads exist yet (a pristine database).
pub(super) fn db_is_empty(conn: &Connection) -> rusqlite::Result<bool> {
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
pub(super) enum CountedTable {
    Members,
    Channels,
    DmThreads,
}

impl CountedTable {
    /// The literal that goes into the SQL. Every arm is a fixed string.
    pub(super) fn as_sql(self) -> &'static str {
        match self {
            CountedTable::Members => "members",
            CountedTable::Channels => "channels",
            CountedTable::DmThreads => "dm_threads",
        }
    }
}

pub(super) fn table_count(conn: &Connection, table: CountedTable) -> rusqlite::Result<i64> {
    conn.query_row(
        &format!("SELECT COUNT(*) FROM {}", table.as_sql()),
        [],
        |r| r.get(0),
    )
}

impl PartyState {
    pub(super) fn count_channels(&self) -> rusqlite::Result<i64> {
        match &self.db {
            Some(conn) => table_count(conn, CountedTable::Channels),
            None => Ok(0),
        }
    }
}

pub(super) fn insert_member_row(
    conn: &Connection,
    id: Uuid,
    username: &str,
    fingerprint: Option<&str>,
    role: Role,
) -> rusqlite::Result<()> {
    let role = serde_json::to_string(&role).expect("Role serializes");
    execute_cached(
        conn,
        "INSERT OR REPLACE INTO members (id, username, fingerprint, role) VALUES (?1, ?2, ?3, ?4)",
        params![id.to_string(), username, fingerprint, role],
    )?;
    Ok(())
}

pub(super) fn insert_channel_row(
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
    execute_cached(
        conn,
        "INSERT OR REPLACE INTO channels (id, name, kind, members, position)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id.to_string(), name, kind, members, position as i64],
    )?;
    Ok(())
}

pub(super) fn insert_message_row(conn: &Connection, e: &Envelope) -> rusqlite::Result<()> {
    let json = serde_json::to_string(e).expect("Envelope serializes");
    execute_cached(
        conn,
        "INSERT OR REPLACE INTO messages (channel_id, seq, envelope) VALUES (?1, ?2, ?3)",
        params![e.channel.to_string(), e.seq as i64, json],
    )?;
    Ok(())
}

pub(super) fn insert_dm_message_row(
    conn: &Connection,
    thread_id: Uuid,
    e: &Envelope,
) -> rusqlite::Result<()> {
    let json = serde_json::to_string(e).expect("Envelope serializes");
    // Two statements, one transaction: a DM used to cost two commits, and a
    // crash between them left a thread row with no message.
    atomically(conn, |conn| {
        execute_cached(
            conn,
            "INSERT OR IGNORE INTO dm_threads (id) VALUES (?1)",
            params![thread_id.to_string()],
        )?;
        execute_cached(
            conn,
            "INSERT OR REPLACE INTO dm_messages (thread_id, seq, envelope) VALUES (?1, ?2, ?3)",
            params![thread_id.to_string(), e.seq as i64, json],
        )?;
        Ok(())
    })
}

/// Load and deserialize an ordered envelope history from `table` where the keying
/// column `key_col` equals `key`. Both `table` and `key_col` are module constants.
/// Remove staging files a previous run left behind. Best effort: a file that
/// cannot be removed is logged and skipped, because failing a server start over
/// one orphaned temporary is a worse outcome than leaving it there.
pub(super) fn sweep_staging_files(blob_dir: &Path) {
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

pub(super) fn load_messages(
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
        out.push(repair_envelope(serde_json::from_str(&json)?));
    }
    Ok(out)
}

/// Bound the member-supplied strings of a stored file message.
///
/// An older server stored a file's name and MIME type exactly as sent, so a
/// database may hold an envelope whose metadata alone is megabytes — large
/// enough to make every page or broadcast carrying it unsendable. Reducing it
/// on load lets such a community be joined again without editing the database.
pub(super) fn repair_envelope(mut env: Envelope) -> Envelope {
    if let MessagePayload::File(meta) = &mut env.payload {
        meta.name = sanitize_filename(&meta.name);
        meta.mime = normalize_mime(&meta.mime);
    }
    env
}

/// One-time import of a legacy JSON snapshot into the (empty) database, in a single
/// transaction so it is all-or-nothing.
pub(super) fn import_legacy_snapshot(conn: &Connection, path: &Path) -> anyhow::Result<()> {
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
