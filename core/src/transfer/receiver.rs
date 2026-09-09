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
pub struct IncomingFileSync {
    tmp_path: PathBuf,
    file: std::fs::File,
    received: u64,
    expected: u64,
    dest_dir: PathBuf,
    filename: String,
}

impl IncomingFileSync {
    /// Start receiving a file into `dest_dir` under the peer-supplied `filename`.
    ///
    /// The directory and the name are separate arguments on purpose. This used
    /// to take one joined `dest_path` and keep it verbatim as the final
    /// destination, sanitising only the *temporary* name; `finalize` then took
    /// `final_dest.file_name()` raw, and `final_dest.parent()` with it. A name
    /// of `../../escaped.txt` therefore landed two directories above the
    /// download folder, and the only thing standing between a peer and that was
    /// the sanitising call in `ProtocolMessage::from_plain_bytes` — one call, in
    /// another crate module, with nothing pinning the coupling.
    ///
    /// Now the caller supplies the directory it chose and the name the peer
    /// chose, and this constructor sanitises the name before either the spool or
    /// the final path is built. `sanitize_filename` is idempotent, so doing it
    /// again here costs nothing and the decoder stays free to keep doing it too.
    pub fn new(dest_dir: &Path, filename: &str, expected_size: u64) -> Result<Self> {
        if expected_size > crate::MAX_FILE_SIZE {
            anyhow::bail!(
                "File size {} exceeds maximum allowed ({} bytes)",
                expected_size,
                crate::MAX_FILE_SIZE
            );
        }

        // The spool lives in the download directory: finalizing is a rename, and
        // a rename is only atomic within one filesystem.
        std::fs::create_dir_all(dest_dir)?;

        let safe_filename = sanitize_filename(filename);
        let tmp_name = format!("tmp_{}_{}", Uuid::new_v4(), safe_filename);
        let tmp_path = dest_dir.join(tmp_name);

        let file = std::fs::File::create(&tmp_path)?;

        Ok(Self {
            tmp_path,
            file,
            received: 0,
            expected: expected_size,
            dest_dir: dest_dir.to_path_buf(),
            filename: safe_filename,
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

        std::fs::create_dir_all(&self.dest_dir)?;

        // Claim the name atomically — see `reserve_unique_path_sync`. This is the
        // path the desktop and terminal clients actually take, so it matters more
        // than the async twin above, not less.
        //
        // `self.filename` was sanitised in `new`, and `self.dest_dir` is the
        // caller's own directory rather than anything derived from the peer's
        // string, so neither half of this join can walk out of the download
        // folder.
        let final_path = reserve_unique_path_sync(&self.dest_dir, &self.filename)?;

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

    /// The receiver sanitises the peer's filename itself.
    ///
    /// The decoder in `core/src/core/protocol.rs` also sanitises, and until now
    /// that was the *only* place it happened: `IncomingFileSync` kept the joined
    /// path verbatim and `finalize` reused its `file_name()` and `parent()` as
    /// they came. So the whole traversal defence rested on one call in another
    /// module, with nothing pinning the coupling — and this test is that pin. It
    /// deliberately bypasses the decoder and hands the receiver a hostile name
    /// directly, which is what an internal caller reaching for the type would do.
    #[test]
    fn a_traversing_filename_cannot_escape_the_download_directory() {
        let temp_dir = TempDir::new().unwrap();
        let download_dir = temp_dir.path().join("downloads");

        for hostile in [
            "../../escaped.txt",
            "..\\..\\escaped.txt",
            "/etc/passwd",
            "..",
        ] {
            let payload = b"owned";
            let mut incoming =
                IncomingFileSync::new(&download_dir, hostile, payload.len() as u64).unwrap();
            incoming.write_chunk(payload).unwrap();
            let final_path = incoming.finalize().unwrap();

            assert!(
                final_path.starts_with(&download_dir),
                "{hostile:?} landed at {final_path:?}, outside the download directory"
            );
            let name = final_path.file_name().unwrap().to_str().unwrap();
            assert!(!name.contains(".."), "traversal survived into {name:?}");
        }

        // Nothing was written beside the download directory either.
        let stray: Vec<_> = std::fs::read_dir(temp_dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n != "downloads")
            .collect();
        assert!(
            stray.is_empty(),
            "files escaped the download dir: {stray:?}"
        );
    }

    /// The spool must live in the directory the file will be renamed into: a
    /// rename is only atomic within one filesystem.
    #[test]
    fn the_spool_lives_in_the_download_directory() {
        let temp_dir = TempDir::new().unwrap();
        let download_dir = temp_dir.path().join("downloads");
        let incoming = IncomingFileSync::new(&download_dir, "photo.png", 4).unwrap();

        let spooled: Vec<_> = std::fs::read_dir(&download_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(spooled.len(), 1, "expected exactly one spool file");
        assert!(
            spooled[0].starts_with("tmp_"),
            "unexpected {:?}",
            spooled[0]
        );
        assert!(spooled[0].ends_with("_photo.png"), "{:?}", spooled[0]);
        drop(incoming);
    }
}
