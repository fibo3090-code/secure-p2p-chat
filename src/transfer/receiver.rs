use anyhow::Result;
use std::path::{Path, PathBuf};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;
use std::io::Write;

use crate::util::sanitize_filename;

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
        // Sanitize filename
        let safe_filename = sanitize_filename(filename);

        tracing::info!("Starting file reception: {} ({} bytes)", safe_filename, size);

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

    /// Append a chunk to the file
    pub async fn append_chunk(&mut self, chunk: &[u8]) -> Result<()> {
        self.file.write_all(chunk).await?;
        self.received += chunk.len() as u64;

        if self.received > self.expected {
            anyhow::bail!(
                "received more data than expected: {} > {}",
                self.received,
                self.expected
            );
        }

        tracing::trace!(
            "Received chunk ({}/{} bytes)",
            self.received,
            self.expected
        );

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

        // Handle filename conflicts
        let mut final_path = dest_dir.join(&self.filename);
        let mut counter = 1;

        while final_path.exists() {
            let stem = Path::new(&self.filename)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("file");
            let ext = Path::new(&self.filename)
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("");

            let new_filename = if ext.is_empty() {
                format!("{}_{}", stem, counter)
            } else {
                format!("{}_{}.{}", stem, counter, ext)
            };

            final_path = dest_dir.join(new_filename);
            counter += 1;
        }

        // Atomic rename to final destination
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
    filename: String,
}

impl IncomingFileSync {
    /// Create a new incoming file
    pub fn new(dest_path: &Path, expected_size: u64) -> Result<Self> {
        let filename = dest_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow::anyhow!("Invalid filename"))?
            .to_string();
        
        let safe_filename = sanitize_filename(&filename);
        
        // Create temp directory if needed
        let tmp_dir = dest_path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(tmp_dir)?;
        
        let tmp_name = format!("tmp_{}_{}", Uuid::new_v4(), safe_filename);
        let tmp_path = tmp_dir.join(tmp_name);
        
        let file = std::fs::File::create(&tmp_path)?;
        
        Ok(Self {
            tmp_path,
            file,
            received: 0,
            expected: expected_size,
            filename: safe_filename,
        })
    }
    
    /// Write a chunk to the file
    pub fn write_chunk(&mut self, chunk: &[u8]) -> Result<()> {
        self.file.write_all(chunk)?;
        self.received += chunk.len() as u64;
        
        if self.received > self.expected {
            anyhow::bail!(
                "Received more data than expected: {} > {}",
                self.received,
                self.expected
            );
        }
        
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
        
        // The temp path is the final location
        Ok(self.tmp_path)
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
        assert!(final_path.to_str().unwrap().contains("test_1.txt"));
    }
}
