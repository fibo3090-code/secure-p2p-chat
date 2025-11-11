use anyhow::Result;
use std::path::Path;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWrite;

use crate::core::{send_packet, AesCipher, ProtocolMessage};
use crate::FILE_CHUNK_SIZE;

/// Send a file over the network in chunks
pub async fn send_file<S, F>(
    path: &Path,
    stream: &mut S,
    cipher: &AesCipher,
    mut progress_callback: F,
) -> Result<()>
where
    S: AsyncWrite + Unpin,
    F: FnMut(u64, u64),
{
    // 1. Get file metadata
    let metadata = tokio::fs::metadata(path).await?;
    let total_size = metadata.len();
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("invalid filename"))?;

    tracing::info!(
        "Starting file transfer: {} ({} bytes)",
        filename,
        total_size
    );

    // 2. Send FileMeta
    let meta_msg = ProtocolMessage::FileMeta {
        filename: filename.to_string(),
        size: total_size,
    };
    send_message(stream, cipher, &meta_msg).await?;

    // 3. Send chunks
    let mut file = File::open(path).await?;
    let mut buffer = vec![0u8; FILE_CHUNK_SIZE];
    let mut bytes_sent = 0u64;
    let mut seq = 0u64;

    loop {
        let n = file.read(&mut buffer).await?;
        if n == 0 {
            break; // EOF
        }

        let chunk_msg = ProtocolMessage::FileChunk {
            chunk: buffer[..n].to_vec(),
            seq,
        };
        send_message(stream, cipher, &chunk_msg).await?;

        bytes_sent += n as u64;
        seq += 1;
        progress_callback(bytes_sent, total_size);

        tracing::trace!("Sent chunk {} ({}/{} bytes)", seq, bytes_sent, total_size);
    }

    // 4. Send FileEnd
    send_message(stream, cipher, &ProtocolMessage::FileEnd).await?;

    tracing::info!("File transfer complete: {} bytes", bytes_sent);
    Ok(())
}

/// Helper to send encrypted protocol message
async fn send_message<S>(stream: &mut S, cipher: &AesCipher, msg: &ProtocolMessage) -> Result<()>
where
    S: AsyncWrite + Unpin,
{
    let plaintext = msg.to_plain_bytes();
    let encrypted = cipher.encrypt(&plaintext);
    send_packet(stream, &encrypted).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::recv_packet;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_send_file_small() {
        // Create temp file
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(b"Hello, file transfer!").unwrap();
        temp_file.flush().unwrap();

        let (mut client, mut server) = tokio::io::duplex(8192);
        let aes_key = [42u8; 32];
        let cipher = AesCipher::new(&aes_key);

        // Send file
        let path = temp_file.path().to_path_buf();
        let send_cipher = cipher.clone();
        tokio::spawn(async move {
            send_file(&path, &mut client, &send_cipher, |_, _| {})
                .await
                .unwrap();
        });

        // Verify FileMeta received
        let encrypted = recv_packet(&mut server).await.unwrap();
        let plaintext = cipher.decrypt(&encrypted).unwrap();
        let msg = ProtocolMessage::from_plain_bytes(&plaintext).unwrap();

        match msg {
            ProtocolMessage::FileMeta { filename, size } => {
                assert!(filename.ends_with(".tmp") || !filename.is_empty());
                assert_eq!(size, 21);
            }
            _ => panic!("Expected FileMeta"),
        }
    }
}
