use std::io::{Error, ErrorKind, Result};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::MAX_PACKET_SIZE;

/// Send a length-prefixed packet over TCP
/// Format: 4 bytes big-endian length || payload
pub async fn send_packet<S>(stream: &mut S, payload: &[u8]) -> Result<()>
where
    S: AsyncWrite + Unpin,
{
    let len = payload.len();
    if len > MAX_PACKET_SIZE {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("payload too large: {} > {}", len, MAX_PACKET_SIZE),
        ));
    }

    // Send length header (4 bytes big-endian)
    let header = (len as u32).to_be_bytes();
    stream.write_all(&header).await?;

    // Send payload
    stream.write_all(payload).await?;
    stream.flush().await?;

    tracing::trace!("Sent packet: {} bytes", len);
    Ok(())
}

/// Receive a length-prefixed packet from TCP
/// Format: 4 bytes big-endian length || payload
pub async fn recv_packet<S>(stream: &mut S) -> Result<Vec<u8>>
where
    S: AsyncRead + Unpin,
{
    // Read length header
    let mut header = [0u8; 4];
    stream.read_exact(&mut header).await?;
    let len = u32::from_be_bytes(header) as usize;

    // Validate length
    if len > MAX_PACKET_SIZE {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("packet size exceeds limit: {} > {}", len, MAX_PACKET_SIZE),
        ));
    }

    // Read payload
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;

    tracing::trace!("Received packet: {} bytes", len);
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_framing_roundtrip() {
        let (mut client, mut server) = tokio::io::duplex(1024);

        let payload = b"Hello, world!";

        // Send
        tokio::spawn(async move {
            send_packet(&mut client, payload).await.unwrap();
        });

        // Receive
        let received = recv_packet(&mut server).await.unwrap();

        assert_eq!(payload, &received[..]);
    }

    #[tokio::test]
    async fn test_framing_large_payload() {
        let (mut client, mut server) = tokio::io::duplex(10 * 1024 * 1024);

        let payload = vec![42u8; 1024 * 1024]; // 1 MB
        let payload_clone = payload.clone();
        tokio::spawn(async move {
            send_packet(&mut client, &payload_clone).await.unwrap();
        });

        let received = recv_packet(&mut server).await.unwrap();

        assert_eq!(payload, received);
    }

    #[tokio::test]
    async fn test_framing_reject_oversized() {
        let (mut client, mut _server) = tokio::io::duplex(1024);

        let payload = vec![0u8; MAX_PACKET_SIZE + 1];

        let result = send_packet(&mut client, &payload).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_framing_multiple_packets() {
        let (mut client, mut server) = tokio::io::duplex(4096);

        let payloads = vec![b"first".to_vec(), b"second".to_vec(), b"third".to_vec()];
        let payloads_clone = payloads.clone();

        tokio::spawn(async move {
            for payload in payloads_clone {
                send_packet(&mut client, &payload).await.unwrap();
            }
        });

        for expected in payloads {
            let received = recv_packet(&mut server).await.unwrap();
            assert_eq!(expected, received);
        }
    }
}
