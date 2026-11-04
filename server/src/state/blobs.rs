//! Blob storage: the upload pipeline (reserve → stage → commit), quotas,
//! file references and access-checked reads.

use super::*;

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
pub(super) fn blocking_io<T>(f: impl FnOnce() -> T) -> T {
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
pub(super) struct Reservation {
    pub(super) member: Uuid,
    pub(super) bytes: u64,
}

/// The prefix every in-progress upload's file carries in the blob directory.
///
/// Distinct from a blob's name, which is its content hash, so a sweep can tell
/// the two apart with certainty: no hash starts with this.
pub(super) const STAGING_PREFIX: &str = "staging_";

/// Everything [`PartyState::stage`] needs to build a [`StagedUpload`].
///
/// A struct rather than seven positional arguments: they are mostly `Uuid` and
/// `String`, so a transposed pair would type-check and silently attribute an
/// upload to the wrong member or file it under the wrong name.
pub(super) struct StageRequest {
    /// Keys both the reservation and the staging file. For a chunked upload
    /// this is the client's own upload id, so the connection's list of open
    /// uploads doubles as its list of reservations to release.
    pub(super) id: Uuid,
    pub(super) uploader: Uuid,
    pub(super) target: UploadTarget,
    pub(super) name: String,
    pub(super) mime: String,
    pub(super) data: Vec<u8>,
    /// `None` means "not known here", which forces a pessimistic reservation.
    /// Every current caller knows it — see [`PartyState::stage`].
    pub(super) known_hash: Option<String>,
}

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
    pub(super) id: Uuid,
    pub(super) uploader: Uuid,
    pub(super) target: UploadTarget,
    pub(super) name: String,
    pub(super) mime: String,
    /// Taken by `write_staging`; `None` afterwards.
    pub(super) data: Option<Vec<u8>>,
    /// Where blobs live, copied in so step 2 needs nothing from the state.
    pub(super) blob_dir: Option<PathBuf>,
    /// Set by step 2.
    pub(super) hash: Option<String>,
    pub(super) size: u64,
    /// Set once the file is on disk, cleared once it has been renamed into
    /// place or deliberately removed. `Drop` acts on whatever is left.
    pub(super) staging_path: Option<PathBuf>,
}

impl StagedUpload {
    /// The id this upload's reservation and staging file are keyed by.
    ///
    /// The runtime records it so a connection torn down mid-upload can release
    /// the reservation, which `Drop` cannot reach — it can remove the file, but
    /// not touch the state that is counting the bytes.
    pub fn id(&self) -> Uuid {
        self.id
    }

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

impl PartyState {
    /// This member's storage allowance, or `None` when they are exempt.
    pub(super) fn member_blob_limit(&self, member: Uuid) -> Option<u64> {
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
    pub(super) fn member_blob_bytes(&self, member: Uuid) -> u64 {
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
    pub(super) fn record_file_ref(
        &mut self,
        uploader: Uuid,
        meta: &FileMeta,
        location: Uuid,
        is_dm: bool,
    ) {
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
    pub(super) fn effective_perms(&self, member: Uuid, r: &FileRef) -> FilePermissions {
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
    pub(super) fn can_reach_location(&self, member: Uuid, r: &FileRef) -> bool {
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
        self.with_tx(|s| s.share_file_in_tx(actor, hash, from, target))
    }

    fn share_file_in_tx(
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
        self.with_tx(|s| s.set_file_permissions_in_tx(actor, hash, location, member, perms))
    }

    fn set_file_permissions_in_tx(
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
    pub(super) fn member_label(&self, id: Uuid) -> String {
        self.username_of(id)
    }

    /// Drop one reference to a blob, deleting the bytes when the last one goes.
    ///
    /// This is the half of reference counting that never existed: uploads
    /// incremented the count and nothing ever decremented it, so storage only
    /// ever grew and the count was decorative.
    pub(super) fn release_blob(&mut self, hash: &str) {
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
        self.with_tx(|s| s.delete_file_in_tx(actor, hash, location))
    }

    fn delete_file_in_tx(
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
        // One frame carries the whole listing, and re-sharing is free, so the
        // number of entries has no natural bound. Keep the newest that fit.
        truncate_listing(&mut out, MAX_FILE_LISTING, FILE_LISTING_BYTES);
        out
    }

    /// Label a DM thread from `viewer`'s side: the other participant's name.
    pub(super) fn dm_location_name(&self, viewer: Uuid, thread: Uuid) -> String {
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
                // Bounded on arrival rather than at publish: both strings are
                // copied into every message, page and listing that mentions the
                // file, and until then they sit in memory on the uploader's
                // behalf.
                name: sanitize_filename(&name),
                mime: normalize_mime(&mime),
                declared: size,
                target,
                data: Vec::with_capacity(size.min(PARTY_CHUNK_BYTES as u64 * 4) as usize),
                hasher: BlobHasher::new(),
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
        pending.hasher.update(data);
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
        self.check_may_post_file(uploader, target)?;
        // The ceiling is checked inside `stage`, which knows the content hash
        // and can therefore skip the check entirely for content the store
        // already holds. Checking here as well would defeat that: it runs before
        // the hash is consulted, so it refuses a deduplicated re-upload that
        // costs no new storage.
        //
        // The spool is taken before that check can fail, so a refused finish
        // consumes it. That is deliberate: the client has to start over either
        // way, and holding its bytes until the connection drops is the shape of
        // the leak that `an_upload_finished_short_does_not_leak_its_spool`
        // exists to prevent.
        let pending = self.uploads.remove(&upload).expect("checked above");
        // The hash was accumulated chunk by chunk as the upload arrived, so it
        // is free here — no 100 MiB pass under the lock, and no pessimism in the
        // reservation. See `PendingUpload::hasher`.
        let hash = pending.hasher.finish();
        // Keyed by the upload's own id, so the connection's existing list of
        // open uploads is also the list of reservations to release if it dies.
        self.stage(StageRequest {
            id: upload,
            uploader,
            target,
            name: pending.name,
            mime: pending.mime,
            data: pending.data,
            known_hash: Some(hash),
        })
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
        // See `start_upload`: bounded before anything else copies them.
        let (name, mime) = (sanitize_filename(&name), normalize_mime(&mime));
        // Capped at 4 MiB and already in hand, so hashing here costs
        // single-digit milliseconds — cheap enough to buy an exact answer to "do
        // we already hold this?", which is what keeps a re-upload of stored
        // content free at a full ceiling. A chunked upload reaches the same
        // answer differently, by hashing each chunk as it arrives.
        let hash = blob_hash(&data);
        self.stage(StageRequest {
            // No client-supplied id for an inline upload, so one is minted here
            // and recorded on the connection by the dispatcher.
            id: Uuid::new_v4(),
            uploader,
            target,
            name,
            mime,
            data,
            known_hash: Some(hash),
        })
    }

    /// Build a [`StagedUpload`], reserving storage for it unless the content is
    /// one the store already holds.
    ///
    /// `known_hash` is `Some` for every path that reaches here today: an inline
    /// upload hashes its 4 MiB in hand, and a chunked one accumulates the hash
    /// as its chunks arrive. So no reservation is made for content the store
    /// already holds, and a deduplicated re-upload still succeeds at a full
    /// ceiling — the property `a_new_blob_past_the_storage_ceiling_is_rejected`
    /// pins.
    ///
    /// `None` is still handled, and handled pessimistically — a duplicate would
    /// reserve its size and give it back at commit — so a future caller that
    /// genuinely cannot know the hash up front is safe rather than wrong. It
    /// would just be needlessly strict at a full ceiling.
    pub(super) fn stage(&mut self, req: StageRequest) -> Result<StagedUpload, String> {
        let StageRequest {
            id,
            uploader,
            target,
            name,
            mime,
            data,
            known_hash,
        } = req;
        let size = data.len() as u64;
        // Content already held costs no new storage, so it needs no reservation
        // and no ceiling check — the same rule commit applies once the hash is
        // known for certain.
        let already_held = known_hash
            .as_deref()
            .is_some_and(|h| self.blobs.contains_key(h));
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
        staged: StagedUpload,
    ) -> Result<(Envelope, UploadTarget), String> {
        // The blob row (or its refcount), the file reference and the message
        // are one commit: a crash between them used to leave a refcount with
        // no reference to release it, or a blob on disk with no row.
        self.with_tx(|s| s.commit_upload_in_tx(staged))
    }

    fn commit_upload_in_tx(
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
    pub(super) fn publish_staged_blob(
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
    pub(super) fn check_may_post_file(
        &self,
        uploader: Uuid,
        target: UploadTarget,
    ) -> Result<(), String> {
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
    pub(super) fn check_storage_room(&self, uploader: Uuid, size: u64) -> Result<(), String> {
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
    pub(super) fn total_committed_and_reserved_bytes(&self) -> u64 {
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

    /// Drop the uploads and reservations belonging to **one connection**.
    ///
    /// Per connection, not per member: one person can be signed in from two
    /// devices, each with uploads in flight. Sweeping by member would make
    /// closing a laptop cancel the phone's transfer — the connection that went
    /// away is the one whose work should go with it.
    ///
    /// `ids` are the things this connection had open: pending upload spools and
    /// staged reservations, both keyed the same way. A staged upload's `Drop`
    /// removes its own file but cannot reach the state to give the storage back,
    /// so the reservation has to be released from here.
    pub fn cancel_connection_uploads(&mut self, member: Uuid, ids: &[Uuid]) {
        for id in ids {
            if self.uploads.get(id).is_some_and(|p| p.uploader == member) {
                self.uploads.remove(id);
            }
            if self
                .reservations
                .get(id)
                .is_some_and(|r| r.member == member)
            {
                self.reservations.remove(id);
            }
        }
    }

    /// Drop every upload belonging to `member`, across all their connections.
    ///
    /// Only for tests and for a deliberate per-member reset; the disconnect path
    /// wants [`Self::cancel_connection_uploads`], which does not reach across a
    /// member's other devices.
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
    pub(super) fn check_inline_size(data: Vec<u8>) -> Result<Vec<u8>, String> {
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
    pub(super) fn post_inline_file(
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
    pub(super) fn append_file_message(
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
    pub(super) fn append_file_dm(
        &mut self,
        from: Uuid,
        to: Uuid,
        meta: FileMeta,
    ) -> Result<Envelope, String> {
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
    pub(super) fn member_can_access_blob(&self, member: Uuid, hash: &str) -> bool {
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
}
