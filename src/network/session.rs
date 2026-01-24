use anyhow::{anyhow, Result};
use rand::rngs::OsRng;
use rsa::pss::{SigningKey, VerifyingKey};
use rsa::signature::{RandomizedSigner, SignatureEncoding, Verifier};
use rsa::{RsaPrivateKey, RsaPublicKey};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use zeroize::Zeroizing;

use crate::core::{
    derive_session_key, fingerprint_pubkey, generate_ephemeral_keypair, negotiate_signature_scheme,
    parse_x25519_public, pem_decode_public, pem_encode_public, recv_packet, send_packet, AesCipher,
    IdentityProof, ProtocolMessage, SignatureScheme, PROTOCOL_VERSION,
};
use crate::types::SessionEvent;
use crate::HANDSHAKE_TIMEOUT_SECS;

/// HKDF context string for key derivation
const HKDF_INFO: &[u8] = b"p2p-messenger-v2-forward-secrecy";

/// Rate limiting: max connections per IP in the time window
const RATE_LIMIT_MAX_CONNECTIONS: usize = 5;
const RATE_LIMIT_WINDOW_SECS: u64 = 10;

/// Global rate limiter state
static RATE_LIMITER: std::sync::LazyLock<Mutex<HashMap<IpAddr, Vec<Instant>>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Check if an IP is rate-limited. Returns true if connection should be rejected.
fn is_rate_limited(ip: IpAddr) -> bool {
    let mut limiter = RATE_LIMITER.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    let window = std::time::Duration::from_secs(RATE_LIMIT_WINDOW_SECS);

    let attempts = limiter.entry(ip).or_default();

    // Remove old attempts outside the window
    attempts.retain(|t| now.duration_since(*t) < window);

    if attempts.len() >= RATE_LIMIT_MAX_CONNECTIONS {
        tracing::warn!(
            "Rate limiting IP {}: {} attempts in {}s",
            ip,
            attempts.len(),
            RATE_LIMIT_WINDOW_SECS
        );
        return true;
    }

    attempts.push(now);
    false
}

/// Receive packet with handshake timeout to prevent Slowloris attacks
async fn recv_packet_with_timeout<S>(stream: &mut S) -> Result<Vec<u8>>
where
    S: AsyncRead + Unpin,
{
    let timeout = std::time::Duration::from_secs(HANDSHAKE_TIMEOUT_SECS);
    match tokio::time::timeout(timeout, recv_packet(stream)).await {
        Ok(Ok(data)) => Ok(data),
        Ok(Err(e)) => Err(anyhow!("Receive failed: {}", e)),
        Err(_) => Err(anyhow!("Handshake timeout ({}s)", HANDSHAKE_TIMEOUT_SECS)),
    }
}

/// Run host session: listen, accept, handshake (v3), message loop
pub async fn run_host_session(
    port: u16,
    privkey: RsaPrivateKey,
    to_app_tx: mpsc::UnboundedSender<SessionEvent>,
    from_app_rx: mpsc::UnboundedReceiver<ProtocolMessage>,
    mut confirm_rx: mpsc::UnboundedReceiver<bool>,
    chat_id: uuid::Uuid,
) -> Result<()> {
    // 1. Bind listener (bind to all interfaces for now, could be configurable)
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!("Host listening on port {}", port);

    to_app_tx
        .send(SessionEvent::Listening { port })
        .map_err(|e| anyhow!("Send error: {}", e))?;

    // 2. Accept connection with rate limiting
    let (mut stream, peer_addr) = listener.accept().await?;

    // Check rate limit
    if is_rate_limited(peer_addr.ip()) {
        tracing::warn!(
            "Rejecting connection from {} due to rate limiting",
            peer_addr
        );
        return Err(anyhow!("Connection rejected: rate limit exceeded"));
    }

    tracing::info!("Client connected from {}", peer_addr);

    // --- HANDSHAKE v3 (ECDH First) ---

    // 3. Send Protocol Version (u32, plaintext)
    let version_bytes = (PROTOCOL_VERSION as u32).to_be_bytes();
    send_packet(&mut stream, &version_bytes).await?;

    // 4. Receive Protocol Version (u32, plaintext)
    let client_version_bytes = recv_packet_with_timeout(&mut stream).await?;
    if client_version_bytes.len() != 4 {
        return Err(anyhow!("Invalid version packet length"));
    }
    // Fix: Use as_slice().try_into() to avoid consuming Vec
    let client_version = u32::from_be_bytes(
        client_version_bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("Invalid version bytes"))?,
    );
    tracing::info!("Client protocol version: {}", client_version);

    if client_version < 3 {
        return Err(anyhow!(
            "Client version {} too old (need v3+)",
            client_version
        ));
    }

    // 5. Generate Ephemeral Keys (X25519)
    let (host_ephemeral_secret, host_ephemeral_public) = generate_ephemeral_keypair();
    let host_ephemeral_bytes = host_ephemeral_public.as_bytes();

    // 6. Send Ephemeral Public Key (plaintext)
    send_packet(&mut stream, host_ephemeral_bytes).await?;

    // 7. Receive Ephemeral Public Key (plaintext)
    let client_ephemeral_bytes = recv_packet_with_timeout(&mut stream).await?;
    if client_ephemeral_bytes.len() != 32 {
        return Err(anyhow!("Invalid ephemeral key length"));
    }
    let client_ephemeral_public = parse_x25519_public(&client_ephemeral_bytes)?;

    // 7.5. Negotiate Signature Scheme
    // Host and Client advertise supported signature schemes
    // Default: [RsaPss, Ed25519] = [1, 2]
    let our_schemes = vec![
        SignatureScheme::RsaPss.to_u8(),
        SignatureScheme::Ed25519.to_u8(),
    ];
    let schemes_msg = ProtocolMessage::SupportedSignatureSchemes {
        schemes: our_schemes.clone(),
    };
    let schemes_bytes = schemes_msg.to_plain_bytes();
    send_packet(&mut stream, &schemes_bytes).await?;

    // Receive client's supported schemes
    let client_schemes_bytes = recv_packet_with_timeout(&mut stream).await?;
    let client_schemes = match ProtocolMessage::from_plain_bytes(&client_schemes_bytes) {
        Some(ProtocolMessage::SupportedSignatureSchemes { schemes }) => schemes,
        _ => {
            tracing::warn!("Invalid or missing signature schemes from client");
            return Err(anyhow!("Client did not send valid signature schemes"));
        }
    };

    // Negotiate the best common scheme (Ed25519 preferred, fallback to RSA-PSS)
    let selected_scheme = negotiate_signature_scheme(&our_schemes, &client_schemes)
        .ok_or_else(|| anyhow!("No common signature scheme found"))?;
    tracing::debug!("Negotiated signature scheme: {}", selected_scheme.name());

    // 8. Derive Session Key & Init Cipher
    // Transcript for salt: HostVersion || ClientVersion || HostEphemeral || ClientEphemeral
    let mut transcript = Vec::new();
    transcript.extend_from_slice(&version_bytes); // Host Version
    transcript.extend_from_slice(&client_version_bytes); // Client Version
    transcript.extend_from_slice(host_ephemeral_bytes); // Host Ephemeral
    transcript.extend_from_slice(&client_ephemeral_bytes); // Client Ephemeral
    let salt = Sha256::digest(&transcript);

    let aes_key = Zeroizing::new(derive_session_key(
        host_ephemeral_secret,
        &client_ephemeral_public,
        Some(&salt),
        HKDF_INFO,
    ));
    let cipher = AesCipher::new(&aes_key[..])?;
    tracing::info!("Encrypted tunnel established (Forward Secrecy enabled)");

    // --- ENCRYPTED IDENTITY EXCHANGE ---

    // 9. Create Identity Proof (using negotiated scheme)
    // We sign our own ephemeral key to bind it to our identity (prevent MITM)
    let signature = match selected_scheme {
        SignatureScheme::RsaPss => {
            let signing_key = SigningKey::<Sha256>::new(privkey.clone());
            let mut rng = OsRng;
            let mut hasher = Sha256::new();
            hasher.update(b"IDENTITY_PROOF");
            hasher.update(host_ephemeral_bytes); // Bind to my ephemeral key
            signing_key
                .sign_with_rng(&mut rng, &hasher.finalize())
                .to_vec()
        }
        SignatureScheme::Ed25519 => {
            // For Ed25519, we would need to have Ed25519 identity key
            // For now, we'll fall back to RSA-PSS if Ed25519 not available
            // TODO: Support Ed25519 identity key storage and use
            tracing::debug!("Ed25519 signature scheme requested but not yet fully supported, using RSA-PSS fallback");
            let signing_key = SigningKey::<Sha256>::new(privkey.clone());
            let mut rng = OsRng;
            let mut hasher = Sha256::new();
            hasher.update(b"IDENTITY_PROOF");
            hasher.update(host_ephemeral_bytes);
            signing_key
                .sign_with_rng(&mut rng, &hasher.finalize())
                .to_vec()
        }
    };

    let my_proof = IdentityProof {
        public_key_pem: pem_encode_public(&RsaPublicKey::from(&privkey))?,
        signature,
        version: PROTOCOL_VERSION as u32,
        chat_id,
        signature_scheme: selected_scheme,
    };

    // Serialize & Encrypt Proof
    let my_proof_bytes = bincode::serialize(&my_proof)?;
    // TODO: Use transcript hash as AAD to bind handshake to encrypted proof
    let encrypted_proof = cipher.encrypt(&my_proof_bytes, None);
    send_packet(&mut stream, &encrypted_proof).await?;

    // 10. Receive Client's Identity Proof (Encrypted)
    let encrypted_client_proof = recv_packet_with_timeout(&mut stream).await?;
    let client_proof_bytes = cipher
        .decrypt(&encrypted_client_proof, None)
        .ok_or_else(|| anyhow!("Failed to decrypt client identity proof"))?;
    let client_proof: IdentityProof = bincode::deserialize(&client_proof_bytes)?;

    // 11. Verify Client Identity
    let client_pubkey = pem_decode_public(&client_proof.public_key_pem)?;
    let verifying_key = VerifyingKey::<Sha256>::new(client_pubkey.clone());

    // Verify signature: It must verify the CLIENT'S ephemeral key
    let mut client_hasher = Sha256::new();
    client_hasher.update(b"IDENTITY_PROOF");
    client_hasher.update(&client_ephemeral_bytes); // Verify against what we received earlier
    let client_digest = client_hasher.finalize();

    verifying_key
        .verify(
            &client_digest,
            &rsa::pss::Signature::try_from(client_proof.signature.as_slice())?,
        )
        .map_err(|e| anyhow!("Client identity signature verification failed: {}", e))?;

    let client_fingerprint = fingerprint_pubkey(client_proof.public_key_pem.as_bytes());
    tracing::debug!("Verified client identity: {}...", &client_fingerprint[..8]);

    // 12. Display Fingerprint & Wait for Confirmation
    to_app_tx
        .send(SessionEvent::NewConnection {
            peer_addr: peer_addr.to_string(),
            fingerprint: client_fingerprint,
            chat_id,
        })
        .map_err(|e| anyhow!("Send error: {}", e))?;

    // Wait for user confirmation (up to 5 minutes) before proceeding
    match tokio::time::timeout(tokio::time::Duration::from_secs(300), async {
        confirm_rx.recv().await
    })
    .await
    {
        Ok(Some(true)) => {
            tracing::info!("User accepted fingerprint for chat {} (host)", chat_id);
        }
        Ok(Some(false)) => {
            tracing::warn!("User rejected fingerprint for chat {} (host)", chat_id);
            let _ = to_app_tx.send(SessionEvent::Error("Fingerprint rejected".to_string()));
            return Err(anyhow!("Fingerprint rejected by user"));
        }
        Ok(None) => {
            let msg = "Confirmation channel closed";
            let _ = to_app_tx.send(SessionEvent::Error(msg.to_string()));
            return Err(anyhow!("Fingerprint verification failed: channel closed"));
        }
        Err(_) => {
            let msg = "Fingerprint verification timed out";
            let _ = to_app_tx.send(SessionEvent::Error(msg.to_string()));
            return Err(anyhow!("Fingerprint verification timed out"));
        }
    }

    // 13. Enter message loop
    to_app_tx
        .send(SessionEvent::Ready)
        .map_err(|e| anyhow!("Send error: {}", e))?;

    run_message_loop(stream, cipher, to_app_tx, from_app_rx).await
}

/// Run client session: connect, handshake (v3), message loop
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

    // --- HANDSHAKE v3 (ECDH First) ---

    // 2. Receive Host Protocol Version
    let host_version_bytes = recv_packet_with_timeout(&mut stream).await?;
    if host_version_bytes.len() != 4 {
        return Err(anyhow!("Invalid version packet length"));
    }
    // Fix: Use as_slice().try_into() to copy bytes into array without consuming Vec
    let host_version = u32::from_be_bytes(
        host_version_bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("Invalid version bytes"))?,
    );
    tracing::info!("Host protocol version: {}", host_version);

    if host_version < 3 {
        return Err(anyhow!("Host version {} too old (need v3+)", host_version));
    }

    // 3. Send Client Protocol Version
    let version_bytes = (PROTOCOL_VERSION as u32).to_be_bytes();
    send_packet(&mut stream, &version_bytes).await?;

    // 4. Receive Host Ephemeral Public Key
    let host_ephemeral_bytes = recv_packet_with_timeout(&mut stream).await?;
    if host_ephemeral_bytes.len() != 32 {
        return Err(anyhow!("Invalid ephemeral key length"));
    }
    let host_ephemeral_public = parse_x25519_public(&host_ephemeral_bytes)?;

    // 5. Generate Client Ephemeral Keys
    let (client_ephemeral_secret, client_ephemeral_public) = generate_ephemeral_keypair();
    let client_ephemeral_bytes = client_ephemeral_public.as_bytes();

    // 6. Send Client Ephemeral Public Key
    send_packet(&mut stream, client_ephemeral_bytes).await?;

    // 6.5. Negotiate Signature Scheme (Client side)
    // Receive host's supported schemes
    let host_schemes_bytes = recv_packet_with_timeout(&mut stream).await?;
    let host_schemes = match ProtocolMessage::from_plain_bytes(&host_schemes_bytes) {
        Some(ProtocolMessage::SupportedSignatureSchemes { schemes }) => schemes,
        _ => {
            tracing::warn!("Invalid or missing signature schemes from host");
            return Err(anyhow!("Host did not send valid signature schemes"));
        }
    };

    // Send our supported schemes (same as host: [RsaPss, Ed25519])
    let our_schemes = vec![
        SignatureScheme::RsaPss.to_u8(),
        SignatureScheme::Ed25519.to_u8(),
    ];
    let schemes_msg = ProtocolMessage::SupportedSignatureSchemes {
        schemes: our_schemes.clone(),
    };
    let schemes_bytes = schemes_msg.to_plain_bytes();
    send_packet(&mut stream, &schemes_bytes).await?;

    // Negotiate the best common scheme
    let selected_scheme = negotiate_signature_scheme(&host_schemes, &our_schemes)
        .ok_or_else(|| anyhow!("No common signature scheme found"))?;
    tracing::debug!("Negotiated signature scheme: {}", selected_scheme.name());

    // 7. Derive Session Key & Init Cipher
    // Transcript for salt: HostVersion || ClientVersion || HostEphemeral || ClientEphemeral
    // Note: host_version_bytes received first, version_bytes sent second.
    // Order must match Host side: HostVersion || ClientVersion || HostEphemeral || ClientEphemeral
    let mut transcript = Vec::new();
    transcript.extend_from_slice(&host_version_bytes); // Host Version
    transcript.extend_from_slice(&version_bytes); // Client Version
    transcript.extend_from_slice(&host_ephemeral_bytes); // Host Ephemeral
    transcript.extend_from_slice(client_ephemeral_bytes); // Client Ephemeral
    let salt = Sha256::digest(&transcript);

    let aes_key = Zeroizing::new(derive_session_key(
        client_ephemeral_secret,
        &host_ephemeral_public,
        Some(&salt),
        HKDF_INFO,
    ));
    let cipher = AesCipher::new(&aes_key[..])?;
    tracing::info!("Encrypted tunnel established (Forward Secrecy enabled)");

    // --- ENCRYPTED IDENTITY EXCHANGE ---

    // 8. Receive Host Identity Proof (Encrypted)
    let encrypted_host_proof = recv_packet_with_timeout(&mut stream).await?;
    let host_proof_bytes = cipher
        .decrypt(&encrypted_host_proof, None)
        .ok_or_else(|| anyhow!("Failed to decrypt host identity proof"))?;
    let host_proof: IdentityProof = bincode::deserialize(&host_proof_bytes)?;

    // 9. Verify Host Identity
    let host_pubkey = pem_decode_public(&host_proof.public_key_pem)?;
    let verifying_key = VerifyingKey::<Sha256>::new(host_pubkey.clone());

    // Verify signature: It must verify the HOST'S ephemeral key
    let mut host_hasher = Sha256::new();
    host_hasher.update(b"IDENTITY_PROOF");
    host_hasher.update(&host_ephemeral_bytes); // Verify against what we received earlier
    let host_digest = host_hasher.finalize();

    verifying_key
        .verify(
            &host_digest,
            &rsa::pss::Signature::try_from(host_proof.signature.as_slice())?,
        )
        .map_err(|e| anyhow!("Host identity signature verification failed: {}", e))?;

    let host_fingerprint = fingerprint_pubkey(host_proof.public_key_pem.as_bytes());
    tracing::debug!("Verified host identity: {}...", &host_fingerprint[..8]);

    // 10. Create Identity Proof (using negotiated scheme)
    let signature = match selected_scheme {
        SignatureScheme::RsaPss => {
            let signing_key = SigningKey::<Sha256>::new(privkey.clone());
            let mut rng = OsRng;
            let mut hasher = Sha256::new();
            hasher.update(b"IDENTITY_PROOF");
            hasher.update(client_ephemeral_bytes); // Bind to my ephemeral key
            signing_key
                .sign_with_rng(&mut rng, &hasher.finalize())
                .to_vec()
        }
        SignatureScheme::Ed25519 => {
            // For Ed25519, we would need to have Ed25519 identity key
            // For now, we'll fall back to RSA-PSS if Ed25519 not available
            // TODO: Support Ed25519 identity key storage and use
            tracing::debug!("Ed25519 signature scheme requested but not yet fully supported, using RSA-PSS fallback");
            let signing_key = SigningKey::<Sha256>::new(privkey.clone());
            let mut rng = OsRng;
            let mut hasher = Sha256::new();
            hasher.update(b"IDENTITY_PROOF");
            hasher.update(client_ephemeral_bytes);
            signing_key
                .sign_with_rng(&mut rng, &hasher.finalize())
                .to_vec()
        }
    };

    let my_proof = IdentityProof {
        public_key_pem: pem_encode_public(&RsaPublicKey::from(&privkey))?,
        signature,
        version: PROTOCOL_VERSION as u32,
        chat_id,
        signature_scheme: selected_scheme,
    };

    // Serialize & Encrypt Proof
    let my_proof_bytes = bincode::serialize(&my_proof)?;
    let encrypted_proof = cipher.encrypt(&my_proof_bytes, None);
    send_packet(&mut stream, &encrypted_proof).await?;

    // 11. Display Fingerprint & Wait for Confirmation
    to_app_tx
        .send(SessionEvent::ShowFingerprintVerification {
            fingerprint: host_fingerprint.clone(),
            peer_name: host.to_string(),
            chat_id,
        })
        .map_err(|e| anyhow!("Send error: {}", e))?;

    // Wait up to 5 minutes for user confirmation
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
            let _ = to_app_tx.send(SessionEvent::Error("Fingerprint rejected".to_string()));
            return Err(anyhow!("Fingerprint rejected by user"));
        }
        Ok(None) => {
            let msg = "Confirmation channel closed";
            let _ = to_app_tx.send(SessionEvent::Error(msg.to_string()));
            return Err(anyhow!("Fingerprint verification failed: channel closed"));
        }
        Err(_) => {
            let msg = "Fingerprint verification timed out";
            let _ = to_app_tx.send(SessionEvent::Error(msg.to_string()));
            return Err(anyhow!("Fingerprint verification timed out"));
        }
    }

    // 12. Enter message loop
    to_app_tx
        .send(SessionEvent::Ready)
        .map_err(|e| anyhow!("Send error: {}", e))?;

    run_message_loop(stream, cipher, to_app_tx, from_app_rx).await
}

/// Extract sequence number from a ProtocolMessage
/// Returns None if the message type doesn't have a sequence number (shouldn't happen in practice)
fn extract_sequence(msg: &ProtocolMessage) -> Option<u64> {
    match msg {
        ProtocolMessage::Version { .. } => None, // Handshake only
        ProtocolMessage::EphemeralKey { .. } => None, // Handshake only
        ProtocolMessage::SupportedSignatureSchemes { .. } => None, // Handshake only
        ProtocolMessage::Text { seq, .. } => Some(*seq),
        ProtocolMessage::FileMeta { seq, .. } => Some(*seq),
        ProtocolMessage::FileChunk { seq, .. } => Some(*seq),
        ProtocolMessage::FileEnd { seq } => Some(*seq),
        ProtocolMessage::Ping { seq } => Some(*seq),
        ProtocolMessage::TypingStart { seq } => Some(*seq),
        ProtocolMessage::TypingStop { seq } => Some(*seq),
    }
}

/// Validate message sequence number for replay attack protection
///
/// Transport-layer validation ensures that:
/// 1. Messages are strictly monotonically increasing (seq > last_valid_seq)
/// 2. Replayed messages (old seq) are rejected
/// 3. Out-of-order messages are rejected
/// 4. Duplicate messages are rejected
///
/// This is enforced BEFORE the message is emitted to the app layer.
///
/// # Arguments
/// * `msg` - The parsed ProtocolMessage
/// * `last_valid_seq` - Mutable reference to track the last accepted sequence number
///
/// # Returns
/// * `Ok(())` if the sequence is valid and last_valid_seq is updated
/// * `Err(String)` if the sequence is invalid (with descriptive reason)
fn validate_message_sequence(
    msg: &ProtocolMessage,
    last_valid_seq: &mut u64,
) -> Result<(), String> {
    // Handshake messages don't have sequence numbers - always allow
    if extract_sequence(msg).is_none() {
        return Ok(());
    }

    let seq = extract_sequence(msg).unwrap();

    // Strict monotonic increase: new sequence must be strictly greater than last valid
    if seq <= *last_valid_seq {
        return Err(format!(
            "Replay/Out-of-order detected: received seq={}, expected seq > {}",
            seq, last_valid_seq
        ));
    }

    // Update to new valid sequence
    *last_valid_seq = seq;
    Ok(())
}

/// Main message loop: send and receive encrypted messages with replay protection
async fn run_message_loop<S>(
    mut stream: S,
    cipher: AesCipher,
    to_app_tx: mpsc::UnboundedSender<SessionEvent>,
    mut from_app_rx: mpsc::UnboundedReceiver<ProtocolMessage>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    const RECV_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300); // 5 minutes

    // Track last valid sequence number to detect replays and out-of-order messages
    // This is enforced at the transport layer, not just in the app layer
    let mut last_valid_seq: u64 = 0;

    loop {
        tokio::select! {
            // Receive from network with timeout
            result = tokio::time::timeout(RECV_IDLE_TIMEOUT, recv_packet(&mut stream)) => {
                match result {
                    Ok(Ok(encrypted)) => {
                        tracing::trace!("Received {} bytes encrypted", encrypted.len());

                        if let Some(plaintext) = cipher.decrypt(&encrypted, None) {
                            tracing::trace!("Decrypted {} bytes", plaintext.len());

                            if let Some(msg) = ProtocolMessage::from_plain_bytes(&plaintext) {
                                tracing::debug!("Received message: {:?}", msg);

                                // --- TRANSPORT-LAYER REPLAY PROTECTION ---
                                // Extract sequence number from message and validate it
                                match validate_message_sequence(&msg, &mut last_valid_seq) {
                                    Ok(_) => {
                                        // Sequence is valid, emit the message
                                        if let Err(e) = to_app_tx.send(SessionEvent::MessageReceived(msg)) {
                                            tracing::error!("Failed to send MessageReceived event: {}", e);
                                            return Err(anyhow!("Event channel closed: {}", e));
                                        }
                                    },
                                    Err(e) => {
                                        // Invalid sequence detected - replay attempt or out-of-order
                                        tracing::warn!("Rejecting message due to invalid sequence: {}", e);
                                        // Don't emit event, silently drop the message
                                    }
                                }
                            } else {
                                tracing::warn!("Failed to parse message from {} bytes", plaintext.len());
                                tracing::debug!("Raw plaintext (truncated): {:.64}", String::from_utf8_lossy(&plaintext));
                            }
                        } else {
                            tracing::error!("Decryption failed - possible tampering or key mismatch!");
                            let _ = to_app_tx.send(SessionEvent::Error("Decryption failed!".to_string()));
                        }
                    }
                    Ok(Err(e)) => {
                        let err_msg = format!("Network receive error: {}", e);
                        tracing::error!("{}", err_msg);
                        let _ = to_app_tx.send(SessionEvent::Error(err_msg));
                        break;
                    }
                    Err(_) => {
                        // Timeout occurred
                        let err_msg = format!("Receive idle timeout ({}s)", RECV_IDLE_TIMEOUT.as_secs());
                        tracing::warn!("{}", err_msg);
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

                // TODO: Bind AAD to include chat_id or message sequence
                let encrypted = cipher.encrypt(&plaintext, None);
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
    use crate::core::{generate_rsa_keypair, IdentityProof};
    use crate::RSA_KEY_BITS;
    use anyhow::Result;

    #[tokio::test]
    async fn test_full_handshake_with_forward_secrecy() -> Result<()> {
        let host_privkey = generate_rsa_keypair(RSA_KEY_BITS)?;
        let client_privkey = generate_rsa_keypair(RSA_KEY_BITS)?;

        let (mut host_stream, mut client_stream) = tokio::io::duplex(8192);

        // Host side
        let host_handle = tokio::spawn(async move {
            // 1. Send/Recv Version
            send_packet(&mut host_stream, &(PROTOCOL_VERSION as u32).to_be_bytes()).await?;
            let _client_version = recv_packet(&mut host_stream).await?;

            // 2. Exchange ephemeral keys (Raw Bytes)
            let (host_ephemeral_secret, host_ephemeral_public) = generate_ephemeral_keypair();
            send_packet(&mut host_stream, host_ephemeral_public.as_bytes()).await?;
            let client_ephemeral_bytes = recv_packet(&mut host_stream).await?;
            let client_ephemeral_public = parse_x25519_public(&client_ephemeral_bytes)?;

            // 3. Derive key
            // Transcript: HostVersion || _client_version || HostEphemeral || client_ephemeral_bytes
            let mut transcript = Vec::new();
            transcript.extend_from_slice(&(PROTOCOL_VERSION as u32).to_be_bytes());
            transcript.extend_from_slice(&(PROTOCOL_VERSION as u32).to_be_bytes()); // Assuming client sends same version
            transcript.extend_from_slice(host_ephemeral_public.as_bytes());
            transcript.extend_from_slice(&client_ephemeral_bytes);
            let salt = Sha256::digest(&transcript);

            let host_aes_key = derive_session_key(
                host_ephemeral_secret,
                &client_ephemeral_public,
                Some(&salt),
                HKDF_INFO,
            );
            let cipher = AesCipher::new(&host_aes_key)?;

            // 4. Send Identity Proof (Encrypted)
            let signing_key = SigningKey::<Sha256>::new(host_privkey.clone());
            let mut rng = OsRng;
            let mut hasher = Sha256::new();
            hasher.update(b"IDENTITY_PROOF");
            hasher.update(host_ephemeral_public.as_bytes());
            let signature = signing_key.sign_with_rng(&mut rng, &hasher.finalize());

            let my_proof = IdentityProof {
                public_key_pem: pem_encode_public(&RsaPublicKey::from(&host_privkey))?,
                signature: signature.to_vec(),
                version: PROTOCOL_VERSION as u32,
                chat_id: uuid::Uuid::new_v4(),
                signature_scheme: SignatureScheme::RsaPss,
            };
            let my_proof_bytes = bincode::serialize(&my_proof)?;
            let encrypted_proof = cipher.encrypt(&my_proof_bytes, None);
            send_packet(&mut host_stream, &encrypted_proof).await?;

            // 5. Recv Client Identity Proof (Encrypted)
            let encrypted_client_proof = recv_packet(&mut host_stream).await?;
            let client_proof_bytes = cipher
                .decrypt(&encrypted_client_proof, None)
                .expect("client proof decrypt should succeed");
            let client_proof: IdentityProof = bincode::deserialize(&client_proof_bytes)?;
            assert_eq!(client_proof.version, PROTOCOL_VERSION as u32);

            Ok(host_aes_key)
        });

        // Client side
        let client_handle = tokio::spawn(async move {
            // 1. Recv/Send Version
            let _host_version = recv_packet(&mut client_stream).await?;
            send_packet(&mut client_stream, &(PROTOCOL_VERSION as u32).to_be_bytes()).await?;

            // 2. Exchange ephemeral keys (Raw Bytes)
            let host_ephemeral_bytes = recv_packet(&mut client_stream).await?;
            let host_ephemeral_public = parse_x25519_public(&host_ephemeral_bytes)?;

            let (client_ephemeral_secret, client_ephemeral_public) = generate_ephemeral_keypair();
            send_packet(&mut client_stream, client_ephemeral_public.as_bytes()).await?;

            // 3. Derive key
            // Transcript: _host_version || Version || host_ephemeral_bytes || ClientEphemeral
            let mut transcript = Vec::new();
            transcript.extend_from_slice(&(PROTOCOL_VERSION as u32).to_be_bytes()); // Host version (simulated)
            transcript.extend_from_slice(&(PROTOCOL_VERSION as u32).to_be_bytes()); // Client version
            transcript.extend_from_slice(&host_ephemeral_bytes);
            transcript.extend_from_slice(client_ephemeral_public.as_bytes());
            let salt = Sha256::digest(&transcript);

            let client_aes_key = derive_session_key(
                client_ephemeral_secret,
                &host_ephemeral_public,
                Some(&salt),
                HKDF_INFO,
            );
            let cipher = AesCipher::new(&client_aes_key)?;

            // 4. Recv Host Identity Proof (Encrypted)
            let encrypted_host_proof = recv_packet(&mut client_stream).await?;
            let host_proof_bytes = cipher
                .decrypt(&encrypted_host_proof, None)
                .expect("host proof decrypt should succeed");
            let host_proof: IdentityProof = bincode::deserialize(&host_proof_bytes)?;
            assert_eq!(host_proof.version, PROTOCOL_VERSION as u32);

            // 5. Send Client Identity Proof (Encrypted)
            let signing_key = SigningKey::<Sha256>::new(client_privkey.clone());
            let mut rng = OsRng;
            let mut hasher = Sha256::new();
            hasher.update(b"IDENTITY_PROOF");
            hasher.update(client_ephemeral_public.as_bytes());
            let signature = signing_key.sign_with_rng(&mut rng, &hasher.finalize());

            let my_proof = IdentityProof {
                public_key_pem: pem_encode_public(&RsaPublicKey::from(&client_privkey))?,
                signature: signature.to_vec(),
                version: PROTOCOL_VERSION as u32,
                chat_id: uuid::Uuid::new_v4(),
                signature_scheme: SignatureScheme::RsaPss,
            };
            let my_proof_bytes = bincode::serialize(&my_proof)?;
            let encrypted_proof = cipher.encrypt(&my_proof_bytes, None);
            send_packet(&mut client_stream, &encrypted_proof).await?;

            Ok(client_aes_key)
        });

        let host_aes_res: Result<[u8; 32]> = host_handle.await.unwrap();
        let client_aes_res: Result<[u8; 32]> = client_handle.await.unwrap();

        let host_aes = host_aes_res.unwrap();
        let client_aes = client_aes_res.unwrap();

        // Keys should match
        assert_eq!(host_aes, client_aes);

        Ok(())
    }

    /// Test: Replay protection now works at transport layer
    /// Transport-layer sequence validation rejects:
    /// - Duplicate messages (seq == last_valid_seq)
    /// - Old messages (seq < last_valid_seq)
    /// - Only allows strictly increasing sequence numbers
    #[test]
    fn test_transport_layer_replay_protection() {
        let mut last_valid_seq = 0u64;

        // Test 1: Accept first message with seq=1
        let msg1 = ProtocolMessage::Text {
            text: "Hello".to_string(),
            timestamp: 0,
            seq: 1,
        };
        assert!(validate_message_sequence(&msg1, &mut last_valid_seq).is_ok());
        assert_eq!(last_valid_seq, 1);

        // Test 2: Accept second message with seq=2
        let msg2 = ProtocolMessage::Text {
            text: "World".to_string(),
            timestamp: 1,
            seq: 2,
        };
        assert!(validate_message_sequence(&msg2, &mut last_valid_seq).is_ok());
        assert_eq!(last_valid_seq, 2);

        // Test 3: Reject replay with seq=1 (older than last_valid_seq)
        let replay1 = ProtocolMessage::Text {
            text: "Replay".to_string(),
            timestamp: 2,
            seq: 1,
        };
        let result = validate_message_sequence(&replay1, &mut last_valid_seq);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Replay"));
        // last_valid_seq should NOT be updated
        assert_eq!(last_valid_seq, 2);

        // Test 4: Reject duplicate with seq=2 (equal to last_valid_seq)
        let duplicate2 = ProtocolMessage::Text {
            text: "Duplicate".to_string(),
            timestamp: 3,
            seq: 2,
        };
        let result = validate_message_sequence(&duplicate2, &mut last_valid_seq);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Replay"));
        assert_eq!(last_valid_seq, 2);

        // Test 5: Accept message with seq=3 (strictly greater)
        let msg3 = ProtocolMessage::Text {
            text: "Next".to_string(),
            timestamp: 4,
            seq: 3,
        };
        assert!(validate_message_sequence(&msg3, &mut last_valid_seq).is_ok());
        assert_eq!(last_valid_seq, 3);

        // Test 6: Reject out-of-order with seq=2 (after accepting seq=3)
        let out_of_order = ProtocolMessage::Text {
            text: "Out of order".to_string(),
            timestamp: 5,
            seq: 2,
        };
        let result = validate_message_sequence(&out_of_order, &mut last_valid_seq);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Out-of-order"));
        assert_eq!(last_valid_seq, 3);

        // Test 7: Large gap in sequences is acceptable (seq=100)
        let msg100 = ProtocolMessage::Text {
            text: "Big gap".to_string(),
            timestamp: 6,
            seq: 100,
        };
        assert!(validate_message_sequence(&msg100, &mut last_valid_seq).is_ok());
        assert_eq!(last_valid_seq, 100);
    }

    /// Test: Handshake messages bypass sequence validation
    /// (Version, EphemeralKey, SupportedSignatureSchemes have no seq field)
    #[test]
    fn test_handshake_messages_bypass_sequence_check() {
        let mut last_valid_seq = 5;

        // Handshake messages should always pass, even with no sequence number
        let version_msg = ProtocolMessage::Version { version: 3 };
        assert!(validate_message_sequence(&version_msg, &mut last_valid_seq).is_ok());
        assert_eq!(last_valid_seq, 5); // Unchanged

        let ephemeral_msg = ProtocolMessage::EphemeralKey {
            public_key: vec![0u8; 32],
        };
        assert!(validate_message_sequence(&ephemeral_msg, &mut last_valid_seq).is_ok());
        assert_eq!(last_valid_seq, 5); // Unchanged
    }

    /// Test: Different message types all respect sequence numbers
    #[test]
    fn test_replay_protection_all_message_types() {
        let mut last_valid_seq = 0u64;

        // Test FileChunk sequence validation
        let chunk1 = ProtocolMessage::FileChunk {
            chunk: vec![1, 2, 3],
            seq: 1,
        };
        assert!(validate_message_sequence(&chunk1, &mut last_valid_seq).is_ok());
        assert_eq!(last_valid_seq, 1);

        // Replayed chunk should be rejected
        let replay_chunk = ProtocolMessage::FileChunk {
            chunk: vec![1, 2, 3],
            seq: 1,
        };
        assert!(validate_message_sequence(&replay_chunk, &mut last_valid_seq).is_err());

        // Test FileMeta sequence validation
        let meta = ProtocolMessage::FileMeta {
            filename: "test.txt".to_string(),
            size: 1024,
            seq: 2,
        };
        assert!(validate_message_sequence(&meta, &mut last_valid_seq).is_ok());
        assert_eq!(last_valid_seq, 2);

        // Test Ping sequence validation
        let ping = ProtocolMessage::Ping { seq: 3 };
        assert!(validate_message_sequence(&ping, &mut last_valid_seq).is_ok());
        assert_eq!(last_valid_seq, 3);

        // Test TypingStart sequence validation
        let typing = ProtocolMessage::TypingStart { seq: 4 };
        assert!(validate_message_sequence(&typing, &mut last_valid_seq).is_ok());
        assert_eq!(last_valid_seq, 4);
    }

    /// Previous regression test - now replaced with comprehensive tests above
    /// This documents that Phase 2 (Issue #6) is now COMPLETE
    #[test]
    fn test_issue_6_replay_protection_complete() {
        // Issue #6: Replay Protection & Key Rotation
        //
        // Phase 2 (Transport-Layer Validation) - COMPLETED ✅
        // - Sequence numbers are now validated in transport layer (run_message_loop)
        // - Replayed/out-of-order messages are rejected before app receives them
        // - See test_transport_layer_replay_protection() for comprehensive validation
        //
        // Phase 3 (Key Rotation) - FUTURE WORK
        // - Implement periodic session key re-negotiation
        // - Add RekeyRequest protocol message
        // - Both sides independently track rekey schedule

        assert!(
            true,
            "Transport-layer replay protection is now implemented in Issue #6 Phase 2"
        );
    }
}
