use anyhow::{anyhow, Result};
use rsa::{RsaPrivateKey, RsaPublicKey};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use crate::core::{
    derive_session_key, fingerprint_pubkey, generate_ephemeral_keypair, parse_x25519_public,
    pem_decode_public, pem_encode_public, recv_packet, send_packet, AesCipher, ProtocolMessage,
    PROTOCOL_VERSION,
};
use crate::types::SessionEvent;

/// HKDF context string for key derivation
const HKDF_INFO: &[u8] = b"p2p-messenger-v2-forward-secrecy";

/// Run host session: listen, accept, handshake, message loop
pub async fn run_host_session(
    port: u16,
    privkey: RsaPrivateKey,
    to_app_tx: mpsc::UnboundedSender<SessionEvent>,
    from_app_rx: mpsc::UnboundedReceiver<ProtocolMessage>,
) -> Result<()> {
    // 1. Bind listener
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!("Host listening on port {}", port);

    to_app_tx
        .send(SessionEvent::Listening { port })
        .map_err(|e| anyhow!("Send error: {}", e))?;

    // 2. Accept connection
    let (mut stream, peer_addr) = listener.accept().await?;
    tracing::info!("Client connected from {}", peer_addr);

    to_app_tx
        .send(SessionEvent::Connected {
            peer: peer_addr.to_string(),
        })
        .map_err(|e| anyhow!("Send error: {}", e))?;

    // 3. Send protocol version
    let version_msg = ProtocolMessage::Version {
        version: PROTOCOL_VERSION,
    };
    send_packet(&mut stream, &version_msg.to_plain_bytes()).await?;
    tracing::debug!("Sent protocol version: {}", PROTOCOL_VERSION);

    // 4. Receive client protocol version
    let client_version_bytes = recv_packet(&mut stream).await?;
    let client_version_msg = ProtocolMessage::from_plain_bytes(&client_version_bytes)
        .ok_or_else(|| anyhow!("Failed to parse client version"))?;
    
    let client_version = match client_version_msg {
        ProtocolMessage::Version { version } => version,
        _ => return Err(anyhow!("Expected Version message, got {:?}", client_version_msg)),
    };
    
    tracing::info!("Client protocol version: {}", client_version);
    
    // Check version compatibility
    if client_version < 2 {
        return Err(anyhow!("Client version {} not supported (need v2+)", client_version));
    }

    // 5. Send host public key (for identity/fingerprint)
    let host_pub_pem = pem_encode_public(&RsaPublicKey::from(&privkey))?;
    send_packet(&mut stream, host_pub_pem.as_bytes()).await?;
    tracing::debug!("Sent host RSA public key");

    // 6. Receive client public key
    let client_pub_pem = recv_packet(&mut stream).await?;
    let client_pub_pem_str = String::from_utf8(client_pub_pem)?;
    let _client_pubkey = pem_decode_public(&client_pub_pem_str)?;
    let client_fingerprint = fingerprint_pubkey(client_pub_pem_str.as_bytes());
    tracing::debug!("Received client RSA public key, fingerprint: {}", client_fingerprint);

    // 7. Display fingerprint and wait for user confirmation
    to_app_tx
        .send(SessionEvent::FingerprintReceived {
            fingerprint: client_fingerprint.clone(),
        })
        .map_err(|e| anyhow!("Send error: {}", e))?;

    // TODO: Wait for user confirmation via channel
    // For now, auto-accept after small delay
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // 8. Generate ephemeral X25519 keypair for forward secrecy
    let (host_ephemeral_secret, host_ephemeral_public) = generate_ephemeral_keypair();
    tracing::debug!("Generated host ephemeral X25519 keypair");

    // 9. Send host ephemeral public key
    let host_ephemeral_msg = ProtocolMessage::EphemeralKey {
        public_key: host_ephemeral_public.as_bytes().to_vec(),
    };
    send_packet(&mut stream, &host_ephemeral_msg.to_plain_bytes()).await?;
    tracing::debug!("Sent host ephemeral public key");

    // 10. Receive client ephemeral public key
    let client_ephemeral_bytes = recv_packet(&mut stream).await?;
    let client_ephemeral_msg = ProtocolMessage::from_plain_bytes(&client_ephemeral_bytes)
        .ok_or_else(|| anyhow!("Failed to parse client ephemeral key"))?;
    
    let client_ephemeral_public = match client_ephemeral_msg {
        ProtocolMessage::EphemeralKey { public_key } => parse_x25519_public(&public_key)?,
        _ => return Err(anyhow!("Expected EphemeralKey message")),
    };
    tracing::debug!("Received client ephemeral public key");

    // 11. Derive session key using ECDH + HKDF
    let aes_key = derive_session_key(host_ephemeral_secret, &client_ephemeral_public, HKDF_INFO);
    tracing::info!("Derived session key using X25519 ECDH + HKDF (forward secrecy enabled)");

    let cipher = AesCipher::new(&aes_key);

    // 12. Enter message loop
    to_app_tx
        .send(SessionEvent::Ready)
        .map_err(|e| anyhow!("Send error: {}", e))?;

    run_message_loop(stream, cipher, to_app_tx, from_app_rx).await
}

/// Run client session: connect, handshake, message loop
pub async fn run_client_session(
    host: &str,
    port: u16,
    privkey: RsaPrivateKey,
    to_app_tx: mpsc::UnboundedSender<SessionEvent>,
    from_app_rx: mpsc::UnboundedReceiver<ProtocolMessage>,
) -> Result<()> {
    // 1. Connect to host
    let mut stream = TcpStream::connect((host, port)).await?;
    tracing::info!("Connected to {}:{}", host, port);

    to_app_tx
        .send(SessionEvent::Connected {
            peer: format!("{}:{}", host, port),
        })
        .map_err(|e| anyhow!("Send error: {}", e))?;

    // 2. Receive host protocol version
    let host_version_bytes = recv_packet(&mut stream).await?;
    let host_version_msg = ProtocolMessage::from_plain_bytes(&host_version_bytes)
        .ok_or_else(|| anyhow!("Failed to parse host version"))?;
    
    let host_version = match host_version_msg {
        ProtocolMessage::Version { version } => version,
        _ => return Err(anyhow!("Expected Version message, got {:?}", host_version_msg)),
    };
    
    tracing::info!("Host protocol version: {}", host_version);
    
    // Check version compatibility
    if host_version < 2 {
        return Err(anyhow!("Host version {} not supported (need v2+)", host_version));
    }

    // 3. Send client protocol version
    let version_msg = ProtocolMessage::Version {
        version: PROTOCOL_VERSION,
    };
    send_packet(&mut stream, &version_msg.to_plain_bytes()).await?;
    tracing::debug!("Sent protocol version: {}", PROTOCOL_VERSION);

    // 4. Receive host RSA public key (for identity/fingerprint)
    let host_pub_pem = recv_packet(&mut stream).await?;
    let host_pub_pem_str = String::from_utf8(host_pub_pem)?;
    let _host_pubkey = pem_decode_public(&host_pub_pem_str)?;
    let host_fingerprint = fingerprint_pubkey(host_pub_pem_str.as_bytes());
    tracing::debug!("Received host RSA public key, fingerprint: {}", host_fingerprint);

    // 5. Display fingerprint
    to_app_tx
        .send(SessionEvent::FingerprintReceived {
            fingerprint: host_fingerprint.clone(),
        })
        .map_err(|e| anyhow!("Send error: {}", e))?;

    // TODO: Wait for user confirmation
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // 6. Send client RSA public key
    let client_pub_pem = pem_encode_public(&RsaPublicKey::from(&privkey))?;
    send_packet(&mut stream, client_pub_pem.as_bytes()).await?;
    tracing::debug!("Sent client RSA public key");

    // 7. Receive host ephemeral public key
    let host_ephemeral_bytes = recv_packet(&mut stream).await?;
    let host_ephemeral_msg = ProtocolMessage::from_plain_bytes(&host_ephemeral_bytes)
        .ok_or_else(|| anyhow!("Failed to parse host ephemeral key"))?;
    
    let host_ephemeral_public = match host_ephemeral_msg {
        ProtocolMessage::EphemeralKey { public_key } => parse_x25519_public(&public_key)?,
        _ => return Err(anyhow!("Expected EphemeralKey message")),
    };
    tracing::debug!("Received host ephemeral public key");

    // 8. Generate ephemeral X25519 keypair for forward secrecy
    let (client_ephemeral_secret, client_ephemeral_public) = generate_ephemeral_keypair();
    tracing::debug!("Generated client ephemeral X25519 keypair");

    // 9. Send client ephemeral public key
    let client_ephemeral_msg = ProtocolMessage::EphemeralKey {
        public_key: client_ephemeral_public.as_bytes().to_vec(),
    };
    send_packet(&mut stream, &client_ephemeral_msg.to_plain_bytes()).await?;
    tracing::debug!("Sent client ephemeral public key");

    // 10. Derive session key using ECDH + HKDF
    let aes_key = derive_session_key(client_ephemeral_secret, &host_ephemeral_public, HKDF_INFO);
    tracing::info!("Derived session key using X25519 ECDH + HKDF (forward secrecy enabled)");

    let cipher = AesCipher::new(&aes_key);

    // 11. Enter message loop
    to_app_tx
        .send(SessionEvent::Ready)
        .map_err(|e| anyhow!("Send error: {}", e))?;

    run_message_loop(stream, cipher, to_app_tx, from_app_rx).await
}

/// Main message loop: send and receive encrypted messages
async fn run_message_loop(
    mut stream: TcpStream,
    cipher: AesCipher,
    to_app_tx: mpsc::UnboundedSender<SessionEvent>,
    mut from_app_rx: mpsc::UnboundedReceiver<ProtocolMessage>,
) -> Result<()> {
    loop {
        tokio::select! {
            // Receive from network
            result = recv_packet(&mut stream) => {
                match result {
                    Ok(encrypted) => {
                        tracing::trace!("Received {} bytes encrypted", encrypted.len());

                        if let Some(plaintext) = cipher.decrypt(&encrypted) {
                            tracing::trace!("Decrypted {} bytes", plaintext.len());

                            if let Some(msg) = ProtocolMessage::from_plain_bytes(&plaintext) {
                                tracing::debug!("Received message: {:?}", msg);

                                if let Err(e) = to_app_tx.send(SessionEvent::MessageReceived(msg)) {
                                    tracing::error!("Failed to send MessageReceived event: {}", e);
                                    return Err(anyhow!("Event channel closed: {}", e));
                                }
                            } else {
                                tracing::warn!("Failed to parse message from {} bytes", plaintext.len());
                                tracing::debug!("Raw plaintext: {:?}", String::from_utf8_lossy(&plaintext));
                            }
                        } else {
                            tracing::error!("Decryption failed - possible tampering or key mismatch!");
                        }
                    }
                    Err(e) => {
                        tracing::error!("Network receive error: {}", e);
                        break;
                    }
                }
            }

            // Send to network
            Some(msg) = from_app_rx.recv() => {
                tracing::debug!("Sending message: {:?}", msg);

                let plaintext = msg.to_plain_bytes();
                tracing::trace!("Plaintext {} bytes", plaintext.len());

                let encrypted = cipher.encrypt(&plaintext);
                tracing::trace!("Encrypted to {} bytes", encrypted.len());

                if let Err(e) = send_packet(&mut stream, &encrypted).await {
                    tracing::error!("Network send error: {}", e);
                    break;
                } else {
                    tracing::debug!("Message sent successfully");
                }
            }
        }
    }

    to_app_tx
        .send(SessionEvent::Disconnected)
        .map_err(|e| anyhow!("Send error: {}", e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::generate_rsa_keypair;
    use crate::RSA_KEY_BITS;

    #[tokio::test]
    async fn test_full_handshake() {
        let host_privkey = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let client_privkey = generate_rsa_keypair(RSA_KEY_BITS).unwrap();

        let (mut host_stream, mut client_stream) = tokio::io::duplex(8192);

        // Host side
        let host_handle = tokio::spawn(async move {
            // Send host pubkey
            let host_pub_pem = pem_encode_public(&RsaPublicKey::from(&host_privkey)).unwrap();
            send_packet(&mut host_stream, host_pub_pem.as_bytes())
                .await
                .unwrap();

            // Receive client pubkey
            let client_pub_pem = recv_packet(&mut host_stream).await.unwrap();
            let client_pubkey =
                pem_decode_public(&String::from_utf8(client_pub_pem).unwrap()).unwrap();

            // Generate and send AES key
            let mut aes_key = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut aes_key);
            let encrypted_aes = rsa_encrypt_oaep(&client_pubkey, &aes_key).unwrap();
            send_packet(&mut host_stream, &encrypted_aes).await.unwrap();

            aes_key
        });

        // Client side
        let client_handle = tokio::spawn(async move {
            // Receive host pubkey
            let _host_pub_pem = recv_packet(&mut client_stream).await.unwrap();

            // Send client pubkey
            let client_pub_pem = pem_encode_public(&RsaPublicKey::from(&client_privkey)).unwrap();
            send_packet(&mut client_stream, client_pub_pem.as_bytes())
                .await
                .unwrap();

            // Receive encrypted AES key
            let encrypted_aes = recv_packet(&mut client_stream).await.unwrap();
            let aes_key_vec = rsa_decrypt_oaep(&client_privkey, &encrypted_aes).unwrap();

            let mut aes_key = [0u8; 32];
            aes_key.copy_from_slice(&aes_key_vec);
            aes_key
        });

        let host_aes = host_handle.await.unwrap();
        let client_aes = client_handle.await.unwrap();

        // Keys should match
        assert_eq!(host_aes, client_aes);
    }
}
