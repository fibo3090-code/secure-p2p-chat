//! Blob storage: the upload pipeline (reserve → stage → commit), quotas,
//! file references and access-checked reads.

use super::*;

/// Run a blocking filesystem operation without stalling the async runtime.
///
/// `block_in_place` panics on a current-thread runtime (the `#[tokio::test]`
/// default), so outside a multi-threaded runtime this is a plain call. It frees
/// the runtime worker, not the state lock: bulk blob I/O is deferred until the
/// lock is released (see [`BlobReadPlan`] and [`StagedUpload`]).
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

/// Prefix of in-progress upload files in the blob directory; no content hash
/// starts with it, so a sweep can tell staging files from blobs.
pub(super) const STAGING_PREFIX: &str = "staging_";

/// Everything [`PartyState::stage`] needs to build a [`StagedUpload`] (a struct,
/// because transposed `Uuid`/`String` arguments would still type-check).
pub(super) struct StageRequest {
    /// Keys both the reservation and the staging file (a chunked upload's own
    /// id, so the connection's open uploads are also its reservations).
    pub(super) id: Uuid,
    pub(super) uploader: Uuid,
    pub(super) target: UploadTarget,
    pub(super) name: String,
    pub(super) mime: String,
    pub(super) data: Vec<u8>,
    /// `None` forces a pessimistic reservation; every current caller knows it.
    pub(super) known_hash: Option<String>,
}

/// An upload that has passed its checks, carried across the gap where the state
/// lock is *not* held:
///
/// 1. Created by [`PartyState::begin_upload`] under the lock, holding the bytes.
/// 2. Written by [`Self::write_staging`] with no lock held.
/// 3. Consumed by [`PartyState::commit_upload`] or [`PartyState::discard_upload`]
///    under the lock again.
///
/// If dropped before step 3, `Drop` removes its staging file; the reservation
/// is released by `commit_upload`/`discard_upload`/`cancel_connection_uploads`.
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
    /// Set by step 2 (or known up front).
    pub(super) hash: Option<String>,
    pub(super) size: u64,
    /// Set once on disk, cleared once renamed into place. `Drop` removes it.
    pub(super) staging_path: Option<PathBuf>,
}

impl StagedUpload {
    /// The id this upload's reservation and staging file are keyed by, recorded
    /// so a torn-down connection can release the reservation `Drop` cannot reach.
    pub fn id(&self) -> Uuid {
        self.id
    }

    /// Hash the bytes and write them to this upload's own staging file.
    ///
    /// **Call this with no lock held** (up to `MAX_PARTY_FILE_BYTES` of I/O).
    /// The staging name is per-upload, never the content hash, so one member's
    /// failed commit cannot unlink bytes another's commit is about to record.
    /// Unlike the persistence mirrors, a failed write propagates: these bytes
    /// *are* the file, so acknowledging the upload would publish a dead link.
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
                // Unlink the partial so a disk-full server does not get worse
                // with every retry.
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

/// A download that has been authorised (under the lock) but not yet performed
/// (after releasing it), so one download does not stall every other member.
#[derive(Debug, Clone)]
pub enum BlobReadPlan {
    /// The memory-only store already holds the bytes; nothing to defer.
    Resident { bytes: Vec<u8>, total: u64 },
    /// Read `len` bytes at `offset` from `path`. `total` is the blob's full
    /// recorded size, which the client is told regardless of `len`.
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
            Self::Resident { total, .. } | Self::OnDisk { total, .. } => *total,
        }
    }

    /// Perform the read. **Call this with no lock held.** A blob deleted while
    /// in flight reads as `None`, reported like an unknown hash.
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

    /// Distinct bytes this member currently holds, plus their reservations.
    /// Counted over *distinct* blobs, like the server-wide ceiling, so one file
    /// shared into three channels costs its size once.
    pub(super) fn member_blob_bytes(&self, member: Uuid) -> u64 {
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut total = 0u64;
        for r in self.file_refs.iter().filter(|r| r.uploader == member) {
            if seen.insert(r.hash.as_str()) {
                total = total.saturating_add(self.blobs.get(&r.hash).map_or(0, |b| b.size));
            }
        }
        // Reservations count too, or several uploads that each fit could
        // together overshoot the allowance after every byte was written.
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
    ) -> Uuid {
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
        let id = entry.id;
        self.file_refs.push(entry);
        id
    }

    /// Undo a file reference and the blob reference it held, for a file
    /// message that could not be stored. The transaction around a commit
    /// commits even when it returns `Err` (`with_tx`), so without this a
    /// refused message left the reference recorded and the bytes counted:
    /// the retry was refused as "already shared there", and the blob was
    /// never collected, charging the member's quota for a file nobody holds.
    fn forget_file_ref(&mut self, id: Uuid, hash: &str) {
        self.file_refs.retain(|r| r.id != id);
        self.delete_file_ref_row(id);
        self.release_blob(hash);
    }

    /// What `member` may do with the share `r`: an admin holds everything;
    /// anyone who cannot reach the location holds nothing; otherwise the
    /// uploader holds everything, an explicit grant overrides the default, and
    /// a guest never holds a write right whatever they were granted.
    pub(super) fn effective_perms(&self, member: Uuid, r: &FileRef) -> FilePermissions {
        if self.role_of(member).is_admin() {
            return FilePermissions::all();
        }
        // Reachability gates uploaders too: making a channel private must not
        // leave its files reachable by the people just excluded from it.
        if !self.can_reach_location(member, r) {
            return FilePermissions::none();
        }
        if r.uploader == member {
            return FilePermissions::all();
        }
        // A guest's read-only role outranks any grant.
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
    /// `from` names the share the caller's rights come from (one blob can sit in
    /// several places under different grants). The new reference starts at the
    /// default grant: re-sharing passes on the file, not the sharer's authority.
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

        let Some(rec) = self.blobs.get(hash) else {
            return Err("that file is no longer stored here".to_string());
        };
        let meta = FileMeta {
            hash: hash.to_string(),
            name: source.name.clone(),
            size: rec.size,
            mime: rec.mime.clone(),
        };

        // The message first: it is the one write that can be refused (QUAL-05),
        // and nothing may be counted for a share that was never posted.
        let envelope = if is_dm {
            self.append_dm_message(actor, location, MessagePayload::File(meta.clone()))?
        } else {
            self.append_channel_message(actor, location, MessagePayload::File(meta.clone()))?
        };
        // Then the new reference to existing bytes: a refcount, counted
        // against the sharer's quota as one more holder.
        if let Some(rec) = self.blobs.get_mut(hash) {
            rec.refcount += 1;
            let refcount = rec.refcount;
            self.persist_blob_refcount(hash, refcount);
        }
        self.record_file_ref(actor, &meta, location, is_dm);
        let detail = format!("shared {} again", source.name);
        self.record_audit(actor, "file.share", &detail);
        Ok(envelope)
    }

    /// Change what a share grants, by default or for one member. Only the
    /// uploader or an admin may, and only within their own effective rights
    /// ("you can only delegate rights you hold").
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
                    format!("changed what {name} grants {}", self.username_of(target))
                }
                None => {
                    r.default_perms = perms;
                    format!("changed what {name} grants by default")
                }
            }
        };
        self.persist_file_ref(&self.file_refs[index]);
        self.record_audit(actor, "file.permissions", &detail);
        Ok(detail)
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
    /// The *message* stays in history (removing it would renumber the channel
    /// for everyone who fetched it); the reference goes, and the bytes with it
    /// once nothing else holds them.
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
        // `effective_perms` grants nothing for an unreachable location, so this
        // one check also covers visibility.
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

    /// Every file `member` may see (same rule as downloading), newest first.
    pub fn files_for(&self, member: Uuid) -> Vec<FileEntry> {
        self.files_page(member, 0).0
    }

    /// One page of [`Self::files_for`] from `offset` on, and the offset of the
    /// next page (`None` at the end), bounded by [`MAX_FILE_LISTING`] entries and
    /// [`FILE_LISTING_BYTES`] so every page fits in a frame (audit SRV-01).
    pub fn files_page(&self, member: Uuid, offset: usize) -> (Vec<FileEntry>, Option<usize>) {
        let mut all = self.visible_files(member);
        let total = all.len();
        let mut page: Vec<FileEntry> = all.drain(offset.min(total)..).collect();
        truncate_listing(&mut page, MAX_FILE_LISTING, FILE_LISTING_BYTES);
        // Always make progress, even past an entry larger than the budget.
        let end = offset.min(total) + page.len().max(usize::from(offset < total));
        (page, (end < total).then_some(end))
    }

    /// Every file `member` may see, newest first, unbounded.
    fn visible_files(&self, member: Uuid) -> Vec<FileEntry> {
        let mut out: Vec<FileEntry> = self
            .file_refs
            .iter()
            .filter_map(|r| {
                let perms = self.effective_perms(member, r);
                if !perms.view {
                    return None;
                }
                let location_name = if r.is_dm {
                    self.dm_location_name(member, r.location)
                } else {
                    self.channel(r.location)
                        .map_or_else(|| "#unknown".to_string(), |c| format!("#{}", c.name))
                };
                let blob = self.blobs.get(&r.hash);
                Some(FileEntry {
                    hash: r.hash.clone(),
                    name: r.name.clone(),
                    size: blob.map_or(0, |b| b.size),
                    mime: blob.map(|b| b.mime.clone()).unwrap_or_default(),
                    uploader: r.uploader,
                    uploader_name: self.username_of(r.uploader),
                    location: r.location,
                    location_name,
                    is_dm: r.is_dm,
                    shared_at: r.shared_at,
                    can_delete: perms.delete,
                    perms,
                })
            })
            .collect();
        // Total order, so page boundaries are stable between requests.
        out.sort_by(|a, b| {
            b.shared_at
                .cmp(&a.shared_at)
                .then_with(|| a.hash.cmp(&b.hash))
                .then_with(|| a.location.cmp(&b.location))
        });
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

    // --- Chunked upload ----------------------------------------------------------

    /// Begin a chunked upload, refusing it before a byte moves if the size,
    /// target or allowance rule it out.
    ///
    /// The concurrency cap counts the spools *this state* holds for the member,
    /// not what the connection believes it has open: trusting the connection's
    /// list let an abandoned upload be repeated past the cap indefinitely.
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
        self.check_may_post_file(uploader, target)?;
        // Provisional: deduplicated content costs nothing, but that is only
        // knowable once the bytes have arrived. Spools are held in memory until
        // they finish, so what is already in flight counts too: checked one at
        // a time, four 100 MiB uploads each passed a 128 MiB allowance (SRV-07).
        let in_flight = |only: Option<Uuid>| -> u64 {
            self.uploads
                .values()
                .filter(|p| only.is_none_or(|m| p.uploader == m))
                .map(|p| p.declared)
                .sum()
        };
        if in_flight(None).saturating_add(size) > self.max_upload_spool_bytes {
            return Err(
                "the server is busy receiving other uploads — try again shortly".to_string(),
            );
        }
        if let Some(limit) = self.member_blob_limit(uploader) {
            let used = self
                .member_blob_bytes(uploader)
                .saturating_add(in_flight(Some(uploader)));
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
                // Bounded on arrival: both strings are copied into every
                // message, page and listing that mentions the file.
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

    /// Append one chunk, refusing anything past the declared size *before*
    /// buffering it.
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

    /// Complete an upload, running all three phases back to back.
    ///
    /// The spool is dropped on **every** path out of here: an incomplete
    /// finish used to leave its bytes behind until restart.
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
    /// take the bytes. No I/O happens here; the caller writes the returned
    /// [`StagedUpload`] once the lock is released, then hands it to
    /// [`Self::commit_upload`].
    pub fn begin_upload(&mut self, uploader: Uuid, upload: Uuid) -> Result<StagedUpload, String> {
        let Some(pending) = self.uploads.get(&upload) else {
            return Err("no such upload".to_string());
        };
        if pending.uploader != uploader {
            // Someone else's upload: reveal nothing and leave it alone.
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

        // Check the target *before* taking the bytes, so a refusal leaves the
        // spool as it was.
        let target = pending.target;
        self.check_may_post_file(uploader, target)?;
        // The ceiling is checked in `stage`, which knows the hash and so lets a
        // deduplicated re-upload through. A refusal there consumes the spool on
        // purpose: the client must start over either way, and keeping the bytes
        // is the leak `an_upload_finished_short_does_not_leak_its_spool` pins.
        let pending = self.uploads.remove(&upload).expect("checked above");
        // Accumulated chunk by chunk (`PendingUpload::hasher`): free here.
        let hash = pending.hasher.finish();
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

    /// Phase 1 for an **inline** upload (one that arrived whole). Staged,
    /// committed and reclaimed exactly like a chunked one, so "no blob write
    /// happens under the state lock" holds without exception.
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
        // Bounded before anything else copies them (see `start_upload`).
        let (name, mime) = (sanitize_filename(&name), normalize_mime(&mime));
        // At most 4 MiB in hand: hashing it now gives an exact "already held?"
        // answer, which keeps a re-upload free at a full ceiling.
        let hash = blob_hash(&data);
        self.stage(StageRequest {
            // Minted here; the dispatcher records it on the connection.
            id: Uuid::new_v4(),
            uploader,
            target,
            name,
            mime,
            data,
            known_hash: Some(hash),
        })
    }

    /// Build a [`StagedUpload`], reserving storage for it unless the store
    /// already holds the content (so a deduplicated re-upload succeeds at a full
    /// ceiling — see `a_new_blob_past_the_storage_ceiling_is_rejected`).
    ///
    /// An unknown hash (no current caller) is handled pessimistically: the
    /// reservation is taken and given back at commit.
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
        // Content already held needs no reservation and no ceiling check.
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
    /// The re-check matters: during the unlocked write the uploader may have
    /// been demoted or removed, the channel deleted, or a DM recipient removed.
    pub fn commit_upload(
        &mut self,
        staged: StagedUpload,
    ) -> Result<(Envelope, UploadTarget), String> {
        // Blob row/refcount, file reference and message commit together.
        self.with_tx(|s| s.commit_upload_in_tx(staged))
    }

    fn commit_upload_in_tx(
        &mut self,
        mut staged: StagedUpload,
    ) -> Result<(Envelope, UploadTarget), String> {
        // Release the claim first: the bytes are now recorded or discarded.
        self.reservations.remove(&staged.id);

        let Some(hash) = staged.hash.clone() else {
            return Err("upload was never written".to_string());
        };
        let uploader = staged.uploader;
        let target = staged.target;

        self.check_may_post_file(uploader, target)?;

        // Only new content is re-checked against the ceiling, now by real hash.
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

    /// Give up on a staged upload: release its reservation (its `Drop` removes
    /// the file).
    pub fn discard_upload(&mut self, staged: StagedUpload) {
        self.reservations.remove(&staged.id);
        drop(staged); // its `Drop` unlinks the staging file
    }

    /// Move a staged upload's bytes into the blob store under their content hash.
    /// A rename of the fsynced staging file, so a blob is absent or complete.
    pub(super) fn publish_staged_blob(
        &mut self,
        staged: &mut StagedUpload,
        hash: &str,
    ) -> Result<FileMeta, String> {
        let size = staged.size;
        let mime = staged.mime.clone();

        if let Some(rec) = self.blobs.get_mut(hash) {
            // Already held: count the reference; `Drop` removes the duplicate.
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
            // Member-supplied: reduce it to a safe filename.
            name: sanitize_filename(&staged.name),
            size,
            mime,
        })
    }

    /// May this member put a file in this place, right now? Shared by every
    /// entry check and the commit re-check, so they cannot drift apart.
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
    /// Reservations count, so uploads still being written are budgeted for.
    pub(super) fn check_storage_room(&self, uploader: Uuid, size: u64) -> Result<(), String> {
        let stored = self.total_committed_and_reserved_bytes();
        if stored.saturating_add(size) > self.max_blob_bytes {
            return Err("server file storage is full".to_string());
        }
        self.check_member_allowance(uploader, size)
    }

    /// Refuse `size` more bytes past this member's allowance (admins exempt).
    pub(super) fn check_member_allowance(&self, uploader: Uuid, size: u64) -> Result<(), String> {
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

    /// Discard an in-flight upload (explicit cancel or disconnect).
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
    /// Per connection, not per member, so closing a laptop does not cancel the
    /// same member's transfer from their phone. `ids` cover both pending spools
    /// and staged reservations (keyed the same way); the latter must be released
    /// here because a staged upload's `Drop` cannot reach the state.
    pub fn cancel_connection_uploads(&mut self, member: Uuid, ids: &[Uuid]) {
        for id in ids {
            self.cancel_upload(member, *id);
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
    /// For tests and deliberate resets; disconnects use
    /// [`Self::cancel_connection_uploads`].
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

    /// Bytes currently held in upload spools, across every member (lets tests
    /// assert an abandoned upload was reclaimed).
    pub fn staged_upload_bytes(&self) -> u64 {
        self.uploads.values().map(|p| p.data.len() as u64).sum()
    }

    /// One chunk of a blob plus its total size, if `member` may see it. `None`
    /// when unknown or not permitted, so existence is never revealed.
    ///
    /// Reads **under the lock**: for in-process callers and tests. The server
    /// uses [`Self::plan_blob_read`], to which this delegates.
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

    /// Run all three upload phases back to back, under whatever lock the caller
    /// holds — the in-process/test form of the server's staged path.
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

    /// Record a file reference and append the file message to a channel (the
    /// single append path for inline and chunked uploads).
    pub(super) fn append_file_message(
        &mut self,
        sender: Uuid,
        channel: Uuid,
        meta: FileMeta,
    ) -> Result<Envelope, String> {
        let (hash, file_ref) = (
            meta.hash.clone(),
            self.record_file_ref(sender, &meta, channel, false),
        );
        self.append_channel_message(sender, channel, MessagePayload::File(meta))
            .map_err(|e| {
                self.forget_file_ref(file_ref, &hash);
                if e == "unknown channel" {
                    "channel no longer exists".to_string()
                } else {
                    e
                }
            })
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
        let (hash, file_ref) = (
            meta.hash.clone(),
            self.record_file_ref(from, &meta, thread_id, true),
        );
        self.append_dm_message(from, thread_id, MessagePayload::File(meta))
            .inspect_err(|_| self.forget_file_ref(file_ref, &hash))
    }

    /// The bytes of a stored blob by content hash, or `None` if unknown.
    /// Disk-backed stores read on demand, so the storage ceiling is not also a
    /// memory ceiling.
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

    /// Whether `member` may download the blob `hash`: some share of it must
    /// grant them `download` (content is stored once, so the endpoint is what
    /// enforces who sees what). Decided over the reference table, so a deleted
    /// reference stops granting access immediately.
    pub(super) fn member_can_access_blob(&self, member: Uuid, hash: &str) -> bool {
        self.file_refs
            .iter()
            .filter(|r| r.hash == hash)
            .any(|r| self.effective_perms(member, r).download)
    }

    /// The bytes of a blob, if `member` may see it (`None` otherwise, so
    /// existence is never revealed). Reads under the lock, like
    /// [`Self::blob_chunk_for`].
    pub fn blob_bytes_for(&self, member: Uuid, hash: &str) -> Option<Vec<u8>> {
        self.plan_blob_read(member, hash, None)?.perform()
    }

    /// Decide a download *without performing it*: authorise under the lock, and
    /// let the caller move the bytes once it is released (`BlobReadPlan::perform`).
    ///
    /// Authorisation is point-in-time; content addressing guarantees a late read
    /// still returns the same bytes, and revocation applies from the next
    /// request. `offset` selects a chunked read (`PARTY_CHUNK_BYTES` from
    /// there); `None` asks for the whole blob.
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

        // Memory-only store (tests): the bytes are already here.
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
            // Bounded by the recorded size (or one chunk), never by the file.
            len: match offset {
                None => total,
                Some(at) => (total.saturating_sub(at)).min(PARTY_CHUNK_BYTES as u64),
            },
            total,
        })
    }
}
