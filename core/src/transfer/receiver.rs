use anyhow::Result;
use std::io::Write;
use std::path::{Path, PathBuf};
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
    pub fn finalize(mut self) -> Result<PathBuf> {
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
}
