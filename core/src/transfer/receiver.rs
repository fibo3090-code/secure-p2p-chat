use anyhow::Result;
use std::io::Write;
use std::path::{Path, PathBuf};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::util::sanitize_filename;
use crate::MAX_FILE_SIZE;

/// Atomically reserve an unused path in `dest_dir` for `filename`, appending
/// `_1`, `_2`, … until one is free.
///
/// Returns a path that now exists as an empty placeholder owned by this process,
/// Blocking twin of [`reserve_unique_path`], for the synchronous receiver.
///
/// Same reasoning: `exists()` then `rename` leaves a window in which anything
/// able to write to the download directory can create the file and have it
/// silently replaced. `create_new` closes it by making the check and the create
/// one kernel operation.
fn reserve_unique_path_sync(dest_dir: &Path, filename: &str) -> Result<PathBuf> {
    const MAX_ATTEMPTS: u32 = 10_000;

    let stem = Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file")
        .to_string();
    let ext = Path::new(filename)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    for counter in 0..MAX_ATTEMPTS {
        let candidate = if counter == 0 {
            dest_dir.join(filename)
        } else if ext.is_empty() {
            dest_dir.join(format!("{stem}_{counter}"))
        } else {
            dest_dir.join(format!("{stem}_{counter}.{ext}"))
        };

        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(_) => return Ok(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }

    Err(anyhow::anyhow!(
        "could not find a free filename for {filename} after {MAX_ATTEMPTS} attempts"
    ))
}

/// so a subsequent rename onto it cannot clobber somebody else's file.
async fn reserve_unique_path(dest_dir: &Path, filename: &str) -> Result<PathBuf> {
    // A bound, so a directory already holding every candidate cannot spin here
    // forever. In practice it stops at 1 or 2.
    const MAX_ATTEMPTS: u32 = 10_000;

    let stem = Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file")
        .to_string();
    let ext = Path::new(filename)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    for counter in 0..MAX_ATTEMPTS {
        let candidate = if counter == 0 {
            dest_dir.join(filename)
        } else if ext.is_empty() {
            dest_dir.join(format!("{stem}_{counter}"))
        } else {
            dest_dir.join(format!("{stem}_{counter}.{ext}"))
        };

        // `create_new(true)` is O_EXCL: the existence check and the creation are
        // one atomic operation, so two racing callers cannot both win.
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
            .await
        {
            Ok(_) => return Ok(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }

    Err(anyhow::anyhow!(
        "could not find a free filename for {filename} after {MAX_ATTEMPTS} attempts"
    ))
}

/// Incoming file being received
pub struct IncomingFile {
    tmp_path: PathBuf,
    file: File,
    received: u64,
    expected: u64,
    filename: String,
}

impl IncomingFile {
    /// Start receiving a file (create temporary file)
    pub async fn start_meta(filename: &str, size: u64, tmp_dir: &Path) -> Result<Self> {
        if size > MAX_FILE_SIZE {
            anyhow::bail!(
                "File size {} exceeds maximum allowed ({} bytes)",
                size,
                MAX_FILE_SIZE
            );
        }
        // Sanitize filename
        let safe_filename = sanitize_filename(filename);

        tracing::info!(
            "Starting file reception: {} ({} bytes)",
            safe_filename,
            size
        );

        // Create temporary file
        tokio::fs::create_dir_all(tmp_dir).await?;
        let tmp_name = format!("tmp_{}_{}", Uuid::new_v4(), safe_filename);
        let tmp_path = tmp_dir.join(tmp_name);

        let file = File::create(&tmp_path).await?;

        Ok(Self {
            tmp_path,
            file,
            received: 0,
            expected: size,
            filename: safe_filename,
        })
    }

    /// Append a chunk to the file.
    ///
    /// The overflow check runs **before** the write: checking afterwards still
    /// let a peer put the excess bytes on our disk, which is the thing the cap
    /// exists to prevent.
    pub async fn append_chunk(&mut self, chunk: &[u8]) -> Result<()> {
        let would_be = self.received.saturating_add(chunk.len() as u64);
        if would_be > self.expected {
            anyhow::bail!(
                "received more data than expected: {} > {}",
                would_be,
                self.expected
            );
        }

        self.file.write_all(chunk).await?;
        self.received = would_be;

        tracing::trace!("Received chunk ({}/{} bytes)", self.received, self.expected);

        Ok(())
    }

    /// Finalize the file transfer (rename to final destination)
    pub async fn finalize(mut self, dest_dir: &Path) -> Result<PathBuf> {
        // Flush and close
        self.file.flush().await?;
        self.file.sync_all().await?;
        drop(self.file);

        // Verify size
        if self.received != self.expected {
            anyhow::bail!(
                "size mismatch: expected {}, got {}",
                self.expected,
                self.received
            );
        }

        // Create destination directory
        tokio::fs::create_dir_all(dest_dir).await?;

        // Claim a free filename, atomically.
        //
        // This used to be `while final_path.exists()` followed by a rename, which
        // leaves a window between the check and the rename. Anything that can
        // write to the download directory — another local account, a shared
        // downloads folder, a second transfer of the same name landing at the
        // same moment — can create the file in that window, and the rename then
        // silently replaces it. Overwriting a file the user already had because
        // a peer sent one with a matching name is a bad enough outcome on its
        // own; it is worse when the timing can be forced.
        //
        // `create_new` is the atomic form of "only if it does not exist": the
        // kernel does the check and the create as one operation, so the name is
        // reserved before anyone else can take it. Renaming onto the placeholder
        // afterwards is safe, because by then the file is ours.
        let final_path = reserve_unique_path(dest_dir, &self.filename).await?;

        // Atomic rename onto the name we just reserved.
        tokio::fs::rename(&self.tmp_path, &final_path).await?;

        tracing::info!("File saved to: {:?}", final_path);

        Ok(final_path)
    }

    /// Abort and cleanup temporary file
    pub async fn abort_cleanup(self) -> Result<()> {
        drop(self.file);
        tokio::fs::remove_file(&self.tmp_path).await.ok();
        tracing::warn!("File transfer aborted, cleaned up temp file");
        Ok(())
    }

    /// Get progress as percentage (0-100)
    pub fn progress_percent(&self) -> f64 {
        if self.expected == 0 {
            0.0
        } else {
            (self.received as f64 / self.expected as f64) * 100.0
        }
    }

    /// Get received bytes
    pub fn received(&self) -> u64 {
        self.received
    }

    /// Get expected size
    pub fn expected(&self) -> u64 {
        self.expected
    }
}

/// Synchronous incoming file for use in non-async contexts
///
/// # The filename contract
///
/// A peer chooses the filename, so **this type sanitises it**, at the boundary,
/// and does not rely on anyone upstream having done so. That is a change: it
/// used to take a whole `dest_path` and keep it verbatim, and `finalize` handed
/// `dest_path.file_name()` straight to [`reserve_unique_path_sync`]. Nothing on
/// that path called [`sanitize_filename`] at all, so
/// `IncomingFileSync::new(&download_dir.join("../../escaped.txt"), n)` wrote
/// `escaped.txt` two directories above the download directory — both the temp
/// spool and the final rename, because the traversal was in the *directory*
/// component by then, not the file name.
///
/// It was not exploitable, because `ProtocolMessage::from_plain_bytes` sanitises
/// `FileMeta.filename` at decode. But that made the entire traversal defence
/// rest on one call in the decoder, with no test anywhere pinning the two
/// together — which is exactly the shape of the last filename bug. So the
/// directory and the name are now separate arguments: the directory is ours and
/// is used as given, the name is the peer's and is sanitised here. The decoder
/// still sanitises too; `sanitize_filename` is idempotent, so applying it twice
/// costs nothing and neither call is load-bearing on its own.
pub struct IncomingFileSync {
    tmp_path: PathBuf,
    file: std::fs::File,
    received: u64,
    expected: u64,
    /// Directory the finished file lands in. Chosen by us (the configured
    /// download directory), never derived from anything a peer sent.
    dest_dir: PathBuf,
    /// Peer-supplied name, already through [`sanitize_filename`].
    safe_filename: String,
}

impl IncomingFileSync {
    /// Start receiving into `dest_dir` under the peer-supplied `filename`.
    ///
    /// `filename` is treated as untrusted and sanitised here; `dest_dir` is
    /// trusted and used as given. See the type-level note on why these are two
    /// arguments rather than one joined path.
    pub fn new(dest_dir: &Path, filename: &str, expected_size: u64) -> Result<Self> {
        if expected_size > crate::MAX_FILE_SIZE {
            anyhow::bail!(
                "File size {} exceeds maximum allowed ({} bytes)",
                expected_size,
                crate::MAX_FILE_SIZE
            );
        }

        let safe_filename = sanitize_filename(filename);

        // The spool lives *in* the download directory, because finalizing is a
        // rename and a rename is only atomic within one filesystem.
        std::fs::create_dir_all(dest_dir)?;

        let tmp_name = format!("tmp_{}_{}", Uuid::new_v4(), safe_filename);
        let tmp_path = dest_dir.join(tmp_name);

        let file = std::fs::File::create(&tmp_path)?;

        Ok(Self {
            tmp_path,
            file,
            received: 0,
            expected: expected_size,
            dest_dir: dest_dir.to_path_buf(),
            safe_filename,
        })
    }

    /// Write a chunk to the file. The cap is enforced *before* the write — see
    /// [`IncomingFile::append_chunk`].
    pub fn write_chunk(&mut self, chunk: &[u8]) -> Result<()> {
        let would_be = self.received.saturating_add(chunk.len() as u64);
        if would_be > self.expected {
            anyhow::bail!(
                "Received more data than expected: {} > {}",
                would_be,
                self.expected
            );
        }

        self.file.write_all(chunk)?;
        self.received = would_be;

        Ok(())
    }

    /// Get bytes received so far
    pub fn bytes_received(&self) -> u64 {
        self.received
    }

    /// Finalize the file transfer
    pub fn finalize(mut self) -> Result<PathBuf> {
        // Flush and sync
        self.file.flush()?;
        self.file.sync_all()?;
        drop(self.file);

        // Verify size
        if self.received != self.expected {
            anyhow::bail!(
                "Size mismatch: expected {}, got {}",
                self.expected,
                self.received
            );
        }

        // Ensure the destination directory exists.
        std::fs::create_dir_all(&self.dest_dir)?;

        // Sanitise again on the way out. `self.safe_filename` came through
        // `sanitize_filename` in `new`, and the function is idempotent, so this
        // is a no-op — which is the point: it costs one pass over a short string
        // and it means no future edit to this struct can reintroduce a
        // peer-controlled name on the rename path without the check still being
        // in front of it.
        let filename = sanitize_filename(&self.safe_filename);

        // Claim the name atomically — see `reserve_unique_path_sync`. This is the
        // path the desktop and terminal clients actually take, so it matters more
        // than the async twin above, not less.
        let final_path = reserve_unique_path_sync(&self.dest_dir, &filename)?;

        // Rename onto the name we just reserved.
        std::fs::rename(&self.tmp_path, &final_path)?;
        tracing::info!("File saved to: {:?}", final_path);

        Ok(final_path)
    }

    /// Abort the transfer and remove the temporary file.
    pub fn abort_cleanup(self) -> Result<()> {
        drop(self.file);
        std::fs::remove_file(&self.tmp_path).ok();
        tracing::warn!("File transfer aborted, cleaned up temp file");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_incoming_file_roundtrip() {
        let temp_dir = TempDir::new().unwrap();

        // Start receiving
        let mut incoming = IncomingFile::start_meta("test.txt", 21, temp_dir.path())
            .await
            .unwrap();

        // Append chunk
        incoming
            .append_chunk(b"Hello, file transfer!")
            .await
            .unwrap();

        // Finalize
        let final_path = incoming.finalize(temp_dir.path()).await.unwrap();

        // Verify content
        let content = tokio::fs::read_to_string(&final_path).await.unwrap();
        assert_eq!(content, "Hello, file transfer!");
    }

    #[tokio::test]
    async fn test_incoming_file_size_mismatch() {
        let temp_dir = TempDir::new().unwrap();

        let mut incoming = IncomingFile::start_meta("test.txt", 10, temp_dir.path())
            .await
            .unwrap();

        incoming.append_chunk(b"Hello").await.unwrap();

        // Should fail due to size mismatch
        let result = incoming.finalize(temp_dir.path()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_filename_conflict() {
        let temp_dir = TempDir::new().unwrap();

        // Create first file
        let file1_path = temp_dir.path().join("test.txt");
        tokio::fs::write(&file1_path, b"first").await.unwrap();

        // Receive file with same name
        let mut incoming = IncomingFile::start_meta("test.txt", 6, temp_dir.path())
            .await
            .unwrap();

        incoming.append_chunk(b"second").await.unwrap();
        let final_path = incoming.finalize(temp_dir.path()).await.unwrap();

        // Should have different name
        assert_ne!(final_path, file1_path);

        // Safely check filename
        let final_filename = final_path.file_name().unwrap().to_str().unwrap();
        assert!(final_filename.starts_with("test_") && final_filename.ends_with(".txt"));
    }

    #[tokio::test]
    async fn test_abort_cleanup_removes_temp_file() {
        let temp_dir = TempDir::new().unwrap();

        let incoming = IncomingFile::start_meta("test.txt", 5, temp_dir.path())
            .await
            .unwrap();
        incoming.abort_cleanup().await.unwrap();

        let remaining: Vec<_> = std::fs::read_dir(temp_dir.path()).unwrap().collect();
        assert!(
            remaining.is_empty(),
            "Temp directory should be empty after abort"
        );
    }

    #[test]
    fn test_sync_write_chunk_overflow() {
        let temp_dir = TempDir::new().unwrap();
        let mut incoming = IncomingFileSync::new(temp_dir.path(), "test.txt", 4).unwrap();

        let err = incoming
            .write_chunk(b"hello")
            .expect_err("should reject oversize chunk");
        assert!(err.to_string().contains("Received more data than expected"));
        assert_eq!(
            incoming.bytes_received(),
            0,
            "a rejected chunk must not be counted"
        );
    }

    /// A peer that keeps sending past the size it declared must not get those
    /// bytes onto our disk — the check has to precede the write.
    #[tokio::test]
    async fn overflowing_chunks_are_rejected_before_they_are_written() {
        let temp_dir = TempDir::new().unwrap();
        let mut incoming = IncomingFile::start_meta("test.txt", 4, temp_dir.path())
            .await
            .unwrap();

        incoming.append_chunk(b"abcd").await.unwrap();
        let err = incoming
            .append_chunk(b"more")
            .await
            .expect_err("past the declared size");
        assert!(err.to_string().contains("more data than expected"));
        assert_eq!(
            incoming.received(),
            4,
            "a rejected chunk must not be counted"
        );

        // The rejected bytes were never handed to the file, so what lands on
        // disk is exactly what was declared.
        let dest = temp_dir.path().join("out");
        let final_path = incoming.finalize(&dest).await.expect("finalizes cleanly");
        assert_eq!(
            std::fs::read(&final_path).unwrap(),
            b"abcd",
            "the excess bytes must never reach the disk"
        );
    }

    /// The receiver is responsible for the peer's filename, not the decoder.
    ///
    /// This used to escape. `IncomingFileSync` kept the joined path verbatim and
    /// `finalize` fed `file_name()` straight to `reserve_unique_path_sync`, so a
    /// `FileMeta` naming `../../escaped.txt` put the file two directories above
    /// the download directory. It was unreachable in practice only because
    /// `ProtocolMessage::from_plain_bytes` sanitises at decode — one call, in a
    /// different module, with nothing pinning the two together. This test is that
    /// pin, at the boundary that actually writes to disk.
    #[test]
    fn a_traversing_filename_cannot_escape_the_download_directory() {
        let temp_dir = TempDir::new().unwrap();
        let download_dir = temp_dir.path().join("Downloads");
        std::fs::create_dir_all(&download_dir).unwrap();

        // Deliberately *not* pre-sanitised: the decoder is not in this test.
        for hostile in [
            "../../escaped.txt",
            "..\\..\\escaped.txt",
            "/etc/passwd",
            "....//escaped.txt",
        ] {
            let mut incoming = IncomingFileSync::new(&download_dir, hostile, 4).unwrap();
            incoming.write_chunk(b"boom").unwrap();
            let final_path = incoming.finalize().expect("finalizes");

            assert_eq!(
                final_path.parent(),
                Some(download_dir.as_path()),
                "{hostile:?} landed outside the download directory at {final_path:?}"
            );
            assert!(
                !final_path.to_string_lossy().contains(".."),
                "{hostile:?} left a traversal component in {final_path:?}"
            );
        }

        // Nothing was written above the download directory, including the spool.
        assert!(!temp_dir.path().join("escaped.txt").exists());
        assert!(!temp_dir.path().join("passwd").exists());
    }

    /// The spool must be inside the download directory too — both because a
    /// rename is only atomic within one filesystem, and because a traversal in
    /// the temp name is the same escape one step earlier.
    #[test]
    fn the_spool_stays_inside_the_download_directory() {
        let temp_dir = TempDir::new().unwrap();
        let download_dir = temp_dir.path().join("Downloads");

        let incoming = IncomingFileSync::new(&download_dir, "../spooled.bin", 1).unwrap();
        let spool = incoming.tmp_path.clone();
        assert_eq!(spool.parent(), Some(download_dir.as_path()));
        assert!(spool.exists());

        incoming.abort_cleanup().unwrap();
        assert!(!spool.exists(), "aborting must remove the spool");
    }

    /// A name the download directory already holds must not be clobbered.
    #[test]
    fn a_colliding_name_is_given_a_free_one() {
        let temp_dir = TempDir::new().unwrap();
        std::fs::write(temp_dir.path().join("notes.txt"), b"mine").unwrap();

        let mut incoming = IncomingFileSync::new(temp_dir.path(), "notes.txt", 5).unwrap();
        incoming.write_chunk(b"theirs").ok();
        incoming.write_chunk(b"peer!").unwrap();
        let final_path = incoming.finalize().unwrap();

        assert_ne!(final_path.file_name().unwrap(), "notes.txt");
        assert_eq!(
            std::fs::read(temp_dir.path().join("notes.txt")).unwrap(),
            b"mine",
            "the file the user already had must survive"
        );
    }
}
