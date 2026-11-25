use anyhow::Result;
use std::io::Write;
use std::path::{Path, PathBuf};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::util::sanitize_filename;

/// Bound on `_N` suffixes tried, so a directory holding every candidate cannot
/// spin forever.
const MAX_NAME_ATTEMPTS: u32 = 10_000;

/// Atomically reserve an unused path in `dest_dir` for `filename`, appending
/// `_1`, `_2`, … until one is free. The path is created as an empty placeholder
/// with `create_new` (O_EXCL), so the check and the create are one kernel
/// operation and the later rename cannot clobber a file someone else made in
/// between (an `exists()` check left that window open).
fn reserve_unique_path(dest_dir: &Path, filename: &str) -> Result<PathBuf> {
    let path = Path::new(filename);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");

    for counter in 0..MAX_NAME_ATTEMPTS {
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
        "could not find a free filename for {filename} after {MAX_NAME_ATTEMPTS} attempts"
    ))
}

/// Async receiver that spools to a separate `tmp_dir`. The app uses
/// [`IncomingFileSync`] instead, whose spool shares the download directory's
/// filesystem so that finalizing is an atomic rename.
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
        if size > crate::MAX_FILE_SIZE {
            anyhow::bail!(
                "File size {} exceeds maximum allowed ({} bytes)",
                size,
                crate::MAX_FILE_SIZE
            );
        }
        let safe_filename = sanitize_filename(filename);
        tracing::info!(
            "Starting file reception: {} ({} bytes)",
            safe_filename,
            size
        );

        tokio::fs::create_dir_all(tmp_dir).await?;
        let tmp_path = tmp_dir.join(format!("tmp_{}_{}", Uuid::new_v4(), safe_filename));
        let file = File::create(&tmp_path).await?;

        Ok(Self {
            tmp_path,
            file,
            received: 0,
            expected: size,
            filename: safe_filename,
        })
    }

    /// Append a chunk. The overflow check precedes the write, so excess bytes
    /// never reach the disk.
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

    /// Finalize the file transfer (rename onto a name reserved with
    /// `reserve_unique_path`).
    pub async fn finalize(mut self, dest_dir: &Path) -> Result<PathBuf> {
        self.file.flush().await?;
        self.file.sync_all().await?;
        drop(self.file);

        if self.received != self.expected {
            anyhow::bail!(
                "size mismatch: expected {}, got {}",
                self.expected,
                self.received
            );
        }

        tokio::fs::create_dir_all(dest_dir).await?;
        let (dir, name) = (dest_dir.to_path_buf(), self.filename.clone());
        let final_path =
            tokio::task::spawn_blocking(move || reserve_unique_path(&dir, &name)).await??;
        tokio::fs::rename(&self.tmp_path, &final_path).await?;
        crate::util::mark_as_downloaded(&final_path);

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

/// An incoming file being spooled to disk.
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
    /// Directory and name stay separate on purpose: the name is sanitised here,
    /// before the spool or final path is built, so traversal (`../../x`) cannot
    /// escape `dest_dir` even if the decoder's own sanitising is bypassed.
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
        let tmp_path = dest_dir.join(format!("tmp_{}_{}", Uuid::new_v4(), safe_filename));
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

    /// Write a chunk. The size cap is checked **before** the write, so excess
    /// bytes never reach the disk.
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

    pub fn bytes_received(&self) -> u64 {
        self.received
    }

    /// Flush, verify the size and rename the spool onto a reserved free name.
    /// Whatever goes wrong, the spool does not stay behind: a short `FileEnd`
    /// used to leave the partial file in the download folder, invisible to
    /// the app and outside every cap (APP-03).
    pub fn finalize(self) -> Result<PathBuf> {
        let spool = self.tmp_path.clone();
        self.finish().inspect_err(|_| {
            std::fs::remove_file(&spool).ok();
        })
    }

    fn finish(mut self) -> Result<PathBuf> {
        self.file.flush()?;
        self.file.sync_all()?;
        drop(self.file);

        if self.received != self.expected {
            anyhow::bail!(
                "Size mismatch: expected {}, got {}",
                self.expected,
                self.received
            );
        }

        std::fs::create_dir_all(&self.dest_dir)?;
        // `filename` was sanitised in `new` and `dest_dir` is the caller's, so
        // this join cannot leave the download folder.
        let final_path = reserve_unique_path(&self.dest_dir, &self.filename)?;
        // Give the reserved name back if the rename fails, or an empty
        // placeholder is left where the file was meant to be.
        if let Err(e) = std::fs::rename(&self.tmp_path, &final_path) {
            std::fs::remove_file(&final_path).ok();
            return Err(e.into());
        }
        crate::util::mark_as_downloaded(&final_path);
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

    /// TAURI-06: a received file carries the Internet zone, so SmartScreen
    /// and Office Protected View apply when it is opened — the in-app
    /// confirmation is one click, and the OS's own checks are the second line.
    #[cfg(windows)]
    #[test]
    fn a_received_file_is_marked_as_coming_from_the_internet() {
        let dir = tempfile::tempdir().unwrap();
        let mut incoming = IncomingFileSync::new(dir.path(), "setup.exe", 3).unwrap();
        incoming.write_chunk(b"MZ!").unwrap();
        let path = incoming.finalize().unwrap();
        let mut stream = path.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        let zone = std::fs::read_to_string(&stream).expect("no Zone.Identifier stream");
        assert!(zone.contains("ZoneId=3"), "{zone}");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"MZ!",
            "the tag touched the content"
        );
    }

    /// APP-03: a peer that declares N bytes and ends after fewer made finalize
    /// fail — and leave the partial spool in the download folder, invisible to
    /// the app and outside every cap once the transfer row was gone. Any
    /// failure to finalize removes what the transfer put on disk.
    #[test]
    fn a_failed_finalize_leaves_nothing_in_the_download_folder() {
        let dir = tempfile::tempdir().unwrap();
        let mut incoming = IncomingFileSync::new(dir.path(), "short.bin", 10).unwrap();
        incoming.write_chunk(b"12345").unwrap();
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "the spool"
        );
        assert!(incoming.finalize().is_err());
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert!(left.is_empty(), "left behind: {left:?}");
    }

    use tempfile::TempDir;

    #[tokio::test]
    async fn the_async_receiver_writes_checks_and_cleans_up() {
        let temp_dir = TempDir::new().unwrap();
        let taken = temp_dir.path().join("test.txt");
        std::fs::write(&taken, b"first").unwrap();

        let mut incoming = IncomingFile::start_meta("test.txt", 4, temp_dir.path())
            .await
            .unwrap();
        incoming.append_chunk(b"abcd").await.unwrap();
        let err = incoming.append_chunk(b"more").await.unwrap_err();
        assert!(err.to_string().contains("more data than expected"));
        assert_eq!(
            incoming.received(),
            4,
            "a rejected chunk must not be counted"
        );
        let final_path = incoming.finalize(temp_dir.path()).await.unwrap();
        assert_ne!(final_path, taken, "an existing file must not be replaced");
        assert_eq!(std::fs::read(&taken).unwrap(), b"first");
        assert_eq!(std::fs::read(&final_path).unwrap(), b"abcd");

        let mut short = IncomingFile::start_meta("short.txt", 10, temp_dir.path())
            .await
            .unwrap();
        short.append_chunk(b"Hello").await.unwrap();
        assert!(short.finalize(temp_dir.path()).await.is_err());

        let spool_dir = temp_dir.path().join("spool");
        let aborted = IncomingFile::start_meta("x.txt", 5, &spool_dir)
            .await
            .unwrap();
        aborted.abort_cleanup().await.unwrap();
        assert_eq!(std::fs::read_dir(&spool_dir).unwrap().count(), 0);
    }

    #[test]
    fn test_incoming_file_roundtrip() {
        let temp_dir = TempDir::new().unwrap();
        let mut incoming = IncomingFileSync::new(temp_dir.path(), "test.txt", 21).unwrap();
        incoming.write_chunk(b"Hello, file transfer!").unwrap();
        let final_path = incoming.finalize().unwrap();
        assert_eq!(
            std::fs::read_to_string(&final_path).unwrap(),
            "Hello, file transfer!"
        );
    }

    #[test]
    fn test_incoming_file_size_mismatch() {
        let temp_dir = TempDir::new().unwrap();
        let mut incoming = IncomingFileSync::new(temp_dir.path(), "test.txt", 10).unwrap();
        incoming.write_chunk(b"Hello").unwrap();
        assert!(incoming.finalize().is_err());
    }

    #[test]
    fn test_filename_conflict() {
        let temp_dir = TempDir::new().unwrap();
        let file1_path = temp_dir.path().join("test.txt");
        std::fs::write(&file1_path, b"first").unwrap();

        let mut incoming = IncomingFileSync::new(temp_dir.path(), "test.txt", 6).unwrap();
        incoming.write_chunk(b"second").unwrap();
        let final_path = incoming.finalize().unwrap();

        assert_ne!(final_path, file1_path);
        assert_eq!(std::fs::read(&file1_path).unwrap(), b"first");
        let final_filename = final_path.file_name().unwrap().to_str().unwrap();
        assert!(final_filename.starts_with("test_") && final_filename.ends_with(".txt"));
    }

    #[test]
    fn test_abort_cleanup_removes_temp_file() {
        let temp_dir = TempDir::new().unwrap();
        let incoming = IncomingFileSync::new(temp_dir.path(), "test.txt", 5).unwrap();
        incoming.abort_cleanup().unwrap();

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

    /// Bytes past the declared size must never reach the disk.
    #[test]
    fn overflowing_chunks_are_rejected_before_they_are_written() {
        let temp_dir = TempDir::new().unwrap();
        let mut incoming = IncomingFileSync::new(temp_dir.path(), "test.txt", 4).unwrap();

        incoming.write_chunk(b"abcd").unwrap();
        let err = incoming
            .write_chunk(b"more")
            .expect_err("past the declared size");
        assert!(err.to_string().contains("more data than expected"));
        assert_eq!(
            incoming.bytes_received(),
            4,
            "a rejected chunk must not be counted"
        );

        let final_path = incoming.finalize().expect("finalizes cleanly");
        assert_eq!(
            std::fs::read(&final_path).unwrap(),
            b"abcd",
            "the excess bytes must never reach the disk"
        );
    }

    /// Pins that the receiver sanitises the name itself, bypassing the decoder
    /// (which also sanitises) by handing it a hostile name directly.
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

    /// The spool must live where the file will be renamed (atomic rename).
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

    /// A received file keeps its own name when that name is free, and takes the
    /// next free `_N` (before the extension, if it has one) when it is not.
    #[test]
    fn a_name_already_taken_gets_the_next_free_number() {
        let dir = tempfile::tempdir().unwrap();
        let receive = |name: &str| {
            let mut incoming = IncomingFileSync::new(dir.path(), name, 1).unwrap();
            incoming.write_chunk(b"x").unwrap();
            incoming.finalize().unwrap()
        };
        let names: Vec<String> = ["photo.jpg", "photo.jpg", "photo.jpg", "README", "README"]
            .iter()
            .map(|n| {
                receive(n)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(
            names,
            [
                "photo.jpg",
                "photo_1.jpg",
                "photo_2.jpg",
                "README",
                "README_1"
            ]
        );
    }

    /// Only "already exists" means "try the next number"; any other error is
    /// reported as itself at once, not after a thousand doomed attempts as
    /// "no free filename".
    #[test]
    fn a_folder_that_cannot_be_written_is_reported_as_itself() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("not-there");
        let err = reserve_unique_path(&missing, "a.txt").unwrap_err();
        assert!(
            !err.to_string().contains("free filename"),
            "the real error was replaced: {err}"
        );
    }
}
