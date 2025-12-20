use anyhow::{Result, anyhow};
use rsa::{RsaPrivateKey, RsaPublicKey};
use rsa::pss::{SigningKey, VerifyingKey};
use rsa::signature::{RandomizedSigner, SignatureEncoding, Verifier};
use sha2::{Sha256, Digest};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use rand::rngs::OsRng;
use zeroize::Zeroizing;

use crate::core::{
    AesCipher, PROTOCOL_VERSION, ProtocolMessage, derive_session_key, fingerprint_pubkey,
    generate_ephemeral_keypair, parse_x25519_public, pem_decode_public, pem_encode_public,
    recv_packet, send_packet,
};
use crate::types::SessionEvent;

/// HKDF context string for key derivation
const HKDF_INFO: &[u8] = b"p2p-messenger-v2-forward-secrecy";

#[derive(serde::Serialize, serde::Deserialize)]
struct SignedVersion {
    version: u32,
    signature: Vec<u8>,
}

/// Run host session: listen, accept, handshake, message loop
pub async fn run_host_session(
    port: u16,
    privkey: RsaPrivateKey,
    to_app_tx: mpsc::UnboundedSender<SessionEvent>,
    from_app_rx: mpsc::UnboundedReceiver<ProtocolMessage>,
    _confirm_rx: mpsc::UnboundedReceiver<bool>,
    chat_id: uuid::Uuid,
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

    // 3. Send host public key (for identity/fingerprint)
    let host_pub_pem = pem_encode_public(&RsaPublicKey::from(&privkey))?;
    send_packet(&mut stream, host_pub_pem.as_bytes()).await?;
    tracing::debug!("Sent host RSA public key");

    // 4. Receive client public key
    let client_pub_pem = recv_packet(&mut stream).await?;
    let client_pub_pem_str = String::from_utf8(client_pub_pem)?;
    let client_pubkey = pem_decode_public(&client_pub_pem_str)?;
    let client_fingerprint = fingerprint_pubkey(client_pub_pem_str.as_bytes());
    tracing::debug!(
        "Received client RSA public key, fingerprint: {}",
        client_fingerprint
    );

    // 5. Send signed protocol version
    let version_bytes = (PROTOCOL_VERSION as u32).to_be_bytes();
    let mut hasher = Sha256::new();
    hasher.update(version_bytes);
    let hashed_version = hasher.finalize();
    
    let signing_key = SigningKey::<Sha256>::new(privkey.clone());
    let mut rng = OsRng;
    let signature = signing_key.sign_with_rng(&mut rng, &hashed_version);

    let signed_version = SignedVersion {
        version: PROTOCOL_VERSION as u32,
        signature: signature.to_vec(),
    };
    let signed_version_bytes = bincode::serialize(&signed_version)?;
    send_packet(&mut stream, &signed_version_bytes).await?;
    tracing::debug!("Sent signed protocol version: {}", PROTOCOL_VERSION);

    // 6. Receive signed client protocol version
    let client_signed_version_bytes = recv_packet(&mut stream).await?;
    let client_signed_version: SignedVersion = bincode::deserialize(&client_signed_version_bytes)?;
    
    let verifying_key = VerifyingKey::<Sha256>::new(client_pubkey.clone());
    let mut client_hasher = Sha256::new();
    client_hasher.update(client_signed_version.version.to_be_bytes());
    let client_hashed_version = client_hasher.finalize();

    verifying_key.verify(&client_hashed_version, &rsa::pss::Signature::try_from(client_signed_version.signature.as_slice())?)
        .map_err(|e| anyhow!("Client version signature verification failed: {}", e))?;

    let client_version = client_signed_version.version;
    tracing::info!("Client protocol version: {}", client_version);

    // Check version compatibility
    if client_version < 2 {
        return Err(anyhow!(
            "Client version {} not supported (need v2+)",
            client_version
        ));
    }

    // 7. Receive chat_id from client (for logging/compat)
    let client_chat_id_bytes = recv_packet(&mut stream).await?;
    let client_chat_id = uuid::Uuid::from_slice(&client_chat_id_bytes)?;
    tracing::debug!("Received client chat_id: {}", client_chat_id);

    // 8. Display fingerprint and wait for user confirmation
    to_app_tx
        .send(SessionEvent::NewConnection {
            peer_addr: peer_addr.to_string(),
            fingerprint: client_fingerprint,
            chat_id, // use host session's chat id to avoid creating a second chat
        })
        .map_err(|e| anyhow!("Send error: {}", e))?;
        
    // 9. Generate ephemeral X25519 keypair for forward secrecy
    let (host_ephemeral_secret, host_ephemeral_public) = generate_ephemeral_keypair();
    tracing::debug!("Generated host ephemeral X25519 keypair");

    // 10. Send host ephemeral public key
    let host_ephemeral_msg = ProtocolMessage::EphemeralKey {
        public_key: host_ephemeral_public.as_bytes().to_vec(),
    };
    send_packet(&mut stream, &host_ephemeral_msg.to_plain_bytes()).await?;
    tracing::debug!("Sent host ephemeral public key");

    // 11. Receive client ephemeral public key
    let client_ephemeral_bytes = recv_packet(&mut stream).await?;
    let client_ephemeral_msg = ProtocolMessage::from_plain_bytes(&client_ephemeral_bytes)
        .ok_or_else(|| anyhow!("Failed to parse client ephemeral key"))?;

    let client_ephemeral_public = match client_ephemeral_msg {
        ProtocolMessage::EphemeralKey { public_key } => parse_x25519_public(&public_key)?,
        _ => return Err(anyhow!("Expected EphemeralKey message")),
    };
    tracing::debug!("Received client ephemeral public key");

    // 12. Derive session key using ECDH + HKDF
    let aes_key = Zeroizing::new(derive_session_key(host_ephemeral_secret, &client_ephemeral_public, HKDF_INFO));
    tracing::info!("Derived session key using X25519 ECDH + HKDF (forward secrecy enabled)");

    let cipher = AesCipher::new(&aes_key[..])?;

    // 13. Enter message loop
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
    mut confirm_rx: mpsc::UnboundedReceiver<bool>,
    chat_id: uuid::Uuid,
) -> Result<()> {
    // 1. Connect to host
    let mut stream = TcpStream::connect((host, port)).await?;
    tracing::info!("Connected to {}:{}", host, port);

    to_app_tx
        .send(SessionEvent::Connected {
            peer: format!("{}:{}", host, port),
        })
        .map_err(|e| anyhow!("Send error: {}", e))?;

    // 2. Receive host RSA public key (for identity/fingerprint)
    let host_pub_pem = recv_packet(&mut stream).await?;
    let host_pub_pem_str = String::from_utf8(host_pub_pem)?;
    let host_pubkey = pem_decode_public(&host_pub_pem_str)?;
    let host_fingerprint = fingerprint_pubkey(host_pub_pem_str.as_bytes());
    tracing::debug!(
        "Received host RSA public key, fingerprint: {}",
        host_fingerprint
    );

    // 3. Send client RSA public key
    let client_pub_pem = pem_encode_public(&RsaPublicKey::from(&privkey))?;
    send_packet(&mut stream, client_pub_pem.as_bytes()).await?;
    tracing::debug!("Sent client RSA public key");
    
    // 4. Receive signed host protocol version
    let host_signed_version_bytes = recv_packet(&mut stream).await?;
    let host_signed_version: SignedVersion = bincode::deserialize(&host_signed_version_bytes)?;
    
    let verifying_key = VerifyingKey::<Sha256>::new(host_pubkey.clone());
    let mut host_hasher = Sha256::new();
    host_hasher.update(host_signed_version.version.to_be_bytes());
    let host_hashed_version = host_hasher.finalize();

    verifying_key.verify(&host_hashed_version, &rsa::pss::Signature::try_from(host_signed_version.signature.as_slice())?)
        .map_err(|e| anyhow!("Host version signature verification failed: {}", e))?;
    
    let host_version = host_signed_version.version;

    tracing::info!("Host protocol version: {}", host_version);

    // Check version compatibility
    if host_version < 2 {
        return Err(anyhow!(
            "Host version {} not supported (need v2+)",
            host_version
        ));
    }

    // 5. Send signed client protocol version
    let version_bytes = (PROTOCOL_VERSION as u32).to_be_bytes();
    let mut hasher = Sha256::new();
    hasher.update(version_bytes);
    let hashed_version = hasher.finalize();

    let signing_key = SigningKey::<Sha256>::new(privkey.clone());
    let mut rng = OsRng;
    let signature = signing_key.sign_with_rng(&mut rng, &hashed_version);

    let signed_version = SignedVersion {
        version: PROTOCOL_VERSION as u32,
        signature: signature.to_vec(),
    };
    let signed_version_bytes = bincode::serialize(&signed_version)?;
    send_packet(&mut stream, &signed_version_bytes).await?;
    tracing::debug!("Sent signed protocol version: {}", PROTOCOL_VERSION);

    // 6. Display fingerprint and wait for confirmation
    to_app_tx
        .send(SessionEvent::ShowFingerprintVerification {
            fingerprint: host_fingerprint.clone(),
            peer_name: host.to_string(),
            chat_id,
        })
        .map_err(|e| anyhow!("Send error: {}", e))?;
        
    // Wait up to 5 minutes for user confirmation. If accepted -> proceed.
    // If explicitly rejected, timeout, or channel closed -> REJECT connection (security fix).
    match tokio::time::timeout(tokio::time::Duration::from_secs(300), async {
        confirm_rx.recv().await
    })
    .await
    {
        Ok(Some(true)) => {
            tracing::info!("User accepted fingerprint for chat {}", chat_id);
        }
        Ok(Some(false)) => {
            tracing::warn!("User rejected fingerprint for chat {}", chat_id);
            let _ = to_app_tx.send(SessionEvent::Error(
                "Fingerprint rejected by user".to_string(),
            ));
            return Err(anyhow!("Fingerprint rejected by user"));
        }
        Ok(None) => {
            let msg = "Confirmation channel closed - REJECTING connection for security";
            tracing::error!("{}", msg);
            let _ = to_app_tx.send(SessionEvent::Error(msg.to_string()));
            return Err(anyhow!("Fingerprint verification failed: channel closed"));
        }
        Err(_) => {
            let msg = "Fingerprint verification timed out (5 min) - REJECTING connection for security";
            tracing::error!("{}", msg);
            let _ = to_app_tx.send(SessionEvent::Error(msg.to_string()));
            return Err(anyhow!("Fingerprint verification timed out"));
        }
    }

    // 7. Send chat_id to host
    send_packet(&mut stream, chat_id.as_bytes()).await?;
    tracing::debug!("Sent chat_id to host: {}", chat_id);

    // 8. Receive host ephemeral public key
    let host_ephemeral_bytes = recv_packet(&mut stream).await?;
    let host_ephemeral_msg = ProtocolMessage::from_plain_bytes(&host_ephemeral_bytes)
        .ok_or_else(|| anyhow!("Failed to parse host ephemeral key"))?;

    let host_ephemeral_public = match host_ephemeral_msg {
        ProtocolMessage::EphemeralKey { public_key } => parse_x25519_public(&public_key)?,
        _ => return Err(anyhow!("Expected EphemeralKey message")),
    };
    tracing::debug!("Received host ephemeral public key");

    // 9. Generate ephemeral X25519 keypair for forward secrecy
    let (client_ephemeral_secret, client_ephemeral_public) = generate_ephemeral_keypair();
    tracing::debug!("Generated client ephemeral X25519 keypair");

    // 10. Send client ephemeral public key
    let client_ephemeral_msg = ProtocolMessage::EphemeralKey {
        public_key: client_ephemeral_public.as_bytes().to_vec(),
    };
    send_packet(&mut stream, &client_ephemeral_msg.to_plain_bytes()).await?;
    tracing::debug!("Sent client ephemeral public key");

    // 11. Derive session key using ECDH + HKDF
    let aes_key = Zeroizing::new(derive_session_key(client_ephemeral_secret, &host_ephemeral_public, HKDF_INFO));
    tracing::info!("Derived session key using X25519 ECDH + HKDF (forward secrecy enabled)");

    let cipher = AesCipher::new(&aes_key[..])?;

    // 12. Enter message loop
    to_app_tx
        .send(SessionEvent::Ready)
        .map_err(|e| anyhow!("Send error: {}", e))?;

    run_message_loop(stream, cipher, to_app_tx, from_app_rx).await
}


/// Main message loop: send and receive encrypted messages
async fn run_message_loop<S>(
    mut stream: S,
    cipher: AesCipher,
    to_app_tx: mpsc::UnboundedSender<SessionEvent>,
    mut from_app_rx: mpsc::UnboundedReceiver<ProtocolMessage>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
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
                            let _ = to_app_tx.send(SessionEvent::Error("Decryption failed!".to_string()));
                        }
                    }
                    Err(e) => {
                        let err_msg = format!("Network receive error: {}", e);
                        tracing::error!("{}", err_msg);
                        let _ = to_app_tx.send(SessionEvent::Error(err_msg));
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
                    let err_msg = format!("Network send error: {}", e);
                    tracing::error!("{}", err_msg);
                    let _ = to_app_tx.send(SessionEvent::Error(err_msg));
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
    use anyhow::Result;

    #[tokio::test]
    async fn test_full_handshake_with_forward_secrecy() -> Result<()> {
        let host_privkey = generate_rsa_keypair(RSA_KEY_BITS)?;
        let client_privkey = generate_rsa_keypair(RSA_KEY_BITS)?;

        let (mut host_stream, mut client_stream) = tokio::io::duplex(8192);

        // Host side
        let host_handle = tokio::spawn(async move {
            // 1. Exchange RSA pubkeys for identity
            let host_pub_pem = pem_encode_public(&RsaPublicKey::from(&host_privkey))?;
            send_packet(&mut host_stream, host_pub_pem.as_bytes()).await?;
            let client_pub_pem = recv_packet(&mut host_stream).await?;
            let _client_pubkey =
                pem_decode_public(&String::from_utf8(client_pub_pem)?)?;

            // 2. Exchange ephemeral keys
            let (host_ephemeral_secret, host_ephemeral_public) = generate_ephemeral_keypair();
            let host_ephemeral_msg = ProtocolMessage::EphemeralKey {
                public_key: host_ephemeral_public.as_bytes().to_vec(),
            };
            send_packet(&mut host_stream, &host_ephemeral_msg.to_plain_bytes()).await?;

            let client_ephemeral_bytes = recv_packet(&mut host_stream).await?;
            let client_ephemeral_msg =
                ProtocolMessage::from_plain_bytes(&client_ephemeral_bytes)
                    .ok_or_else(|| anyhow!("Host failed to parse client ephemeral key"))?;

            let client_ephemeral_public = match client_ephemeral_msg {
                ProtocolMessage::EphemeralKey { public_key } => parse_x25519_public(&public_key)?,
                _ => return Err(anyhow!("Host expected EphemeralKey message")),
            };

            // 3. Derive final key
            let host_aes_key =
                derive_session_key(host_ephemeral_secret, &client_ephemeral_public, HKDF_INFO);

            Ok(host_aes_key)
        });

        // Client side
        let client_handle = tokio::spawn(async move {
            // 1. Exchange RSA pubkeys for identity
            let _host_pub_pem = recv_packet(&mut client_stream).await?;
            let client_pub_pem = pem_encode_public(&RsaPublicKey::from(&client_privkey))?;
            send_packet(&mut client_stream, client_pub_pem.as_bytes()).await?;

            // 2. Exchange ephemeral keys
            let host_ephemeral_bytes = recv_packet(&mut client_stream).await?;
            let host_ephemeral_msg =
                ProtocolMessage::from_plain_bytes(&host_ephemeral_bytes)
                    .ok_or_else(|| anyhow!("Client failed to parse host ephemeral key"))?;
            let host_ephemeral_public = match host_ephemeral_msg {
                ProtocolMessage::EphemeralKey { public_key } => parse_x25519_public(&public_key)?,
                _ => return Err(anyhow!("Client expected EphemeralKey message")),
            };

            let (client_ephemeral_secret, client_ephemeral_public) = generate_ephemeral_keypair();
            let client_ephemeral_msg = ProtocolMessage::EphemeralKey {
                public_key: client_ephemeral_public.as_bytes().to_vec(),
            };
            send_packet(&mut client_stream, &client_ephemeral_msg.to_plain_bytes()).await?;

            // 3. Derive final key
            let client_aes_key = derive_session_key(
                client_ephemeral_secret,
                &host_ephemeral_public,
                HKDF_INFO,
            );
            Ok(client_aes_key)
        });

        let host_aes_res = host_handle.await?;
        let client_aes_res = client_handle.await?;

        let host_aes = host_aes_res?;
        let client_aes = client_aes_res?;

        // Keys should match
        assert_eq!(host_aes, client_aes);

        Ok(())
    }
}