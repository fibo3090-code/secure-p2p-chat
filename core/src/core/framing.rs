use std::io::{Error, ErrorKind, Result};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::MAX_PACKET_SIZE;

/// Send a packet framed as a 4-byte big-endian length, then the payload.
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

    stream.write_all(&(len as u32).to_be_bytes()).await?;
    stream.write_all(payload).await?;
    stream.flush().await?;

    tracing::trace!("Sent packet: {} bytes", len);
    Ok(())
}

/// Receive a packet framed by [`send_packet`]. Reachable before the handshake by
/// anyone who can connect, so the length is capped and the body read in chunks:
/// a forged prefix cannot make us allocate the whole claimed size up front.
pub async fn recv_packet<S>(stream: &mut S) -> Result<Vec<u8>>
where
    S: AsyncRead + Unpin,
{
    let mut header = [0u8; 4];
    stream.read_exact(&mut header).await?;
    let total_len = u32::from_be_bytes(header) as usize;

    if total_len > MAX_PACKET_SIZE {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!(
                "packet size exceeds limit: {} > {}",
                total_len, MAX_PACKET_SIZE
            ),
        ));
    }

    const CHUNK_SIZE: usize = 64 * 1024;
    let mut buf = Vec::with_capacity(CHUNK_SIZE.min(total_len));
    let mut remaining = total_len;

    while remaining > 0 {
        let to_read = remaining.min(CHUNK_SIZE);
        let start = buf.len();
        buf.resize(start + to_read, 0);

        stream.read_exact(&mut buf[start..]).await?;
        remaining -= to_read;
    }

    tracing::trace!("Received packet: {} bytes", total_len);
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_framing_roundtrip() {
        let (mut client, mut server) = tokio::io::duplex(1024);

        let payload = b"Hello, world!";
        tokio::spawn(async move {
            send_packet(&mut client, payload).await.unwrap();
        });
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
    async fn test_framing_recv_reject_oversized_header() {
        let (mut client, mut server) = tokio::io::duplex(1024);

        // A header claiming an oversized payload, with no body: recv must fail at once.
        let oversized_len = (MAX_PACKET_SIZE as u32 + 1).to_be_bytes();

        tokio::spawn(async move {
            client.write_all(&oversized_len).await.ok();
        });

        let result = recv_packet(&mut server).await;
        assert!(
            result.is_err(),
            "recv_packet should reject oversized header"
        );
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
