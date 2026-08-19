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
    cross_sign_ed25519, derive_ed25519_subkey, derive_session_key, ed25519_public_from_bytes,
    ed25519_signature_from_bytes, fingerprint_pubkey, generate_ephemeral_keypair,
    negotiate_signature_scheme, parse_x25519_public, pem_decode_public, pem_encode_public,
    recv_packet, send_packet, sign_ed25519, verify_ed25519, verify_ed25519_binding, AesCipher,
    IdentityProof, NonceRole, ProtocolMessage, SignatureScheme, PROTOCOL_VERSION,
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

fn labeled_aad(label: &[u8], transcript_hash: &[u8]) -> Vec<u8> {
    let mut aad = Vec::with_capacity(label.len() + 1 + transcript_hash.len());
    aad.extend_from_slice(label);
    aad.push(0);
    aad.extend_from_slice(transcript_hash);
    aad
}

/// Bind the host listener on `port` (all interfaces).
///
/// Separate from [`run_host_session`] on purpose: the caller must learn that a
/// bind failed (port already in use) **before** it creates any chat or session
/// state. When the bind lived inside the spawned session task its failure was
/// only logged, and the app was left showing a "Host on :port" conversation
/// that reported itself connected while nothing was listening at all.
pub async fn bind_host_listener(port: u16) -> Result<TcpListener> {
    TcpListener::bind(("0.0.0.0", port))
        .await
        .map_err(|e| anyhow!("Could not listen on port {}: {}", port, e))
}

/// Run host session on an already-bound listener: accept, handshake (v3),
/// message loop. Use [`bind_host_listener`] to obtain the listener.
#[allow(clippy::too_many_arguments)]
pub async fn run_host_session(
    listener: TcpListener,
    privkey: RsaPrivateKey,
    to_app_tx: mpsc::UnboundedSender<SessionEvent>,
    from_app_rx: mpsc::UnboundedReceiver<ProtocolMessage>,
    file_rx: mpsc::Receiver<ProtocolMessage>,
    mut confirm_rx: mpsc::UnboundedReceiver<bool>,
    chat_id: uuid::Uuid,
    connection_password: Option<String>,
) -> Result<()> {
    // 1. Report the port we actually bound (the caller may have asked for 0).
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    tracing::info!("Host listening on port {}", port);

    to_app_tx
        .send(SessionEvent::Listening { port })
        .map_err(|e| anyhow!("Send error: {}", e))?;

    // 2. Accept connection with rate limiting
    let (mut stream, peer_addr) = listener.accept().await?;

    // Release the port the instant we have our peer. A session serves exactly
    // one connection, so holding the listener for the whole conversation only
    // ever did one thing: make the auto-rehost's bind fail with EADDRINUSE, so
    // the app silently stopped accepting anyone after the first peer while
    // still advertising the address as reachable.
    drop(listener);

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
    if client_ephemeral_bytes.len() != crate::AES_KEY_SIZE {
        return Err(anyhow!("Invalid ephemeral key length"));
    }
    let client_ephemeral_public = parse_x25519_public(&client_ephemeral_bytes)?;

    // 7.5. Negotiate Signature Scheme
    // Ed25519 first, RSA-PSS retained for peers that predate it —
    // `negotiate_signature_scheme` prefers Ed25519 when both sides offer it,
    // so an older peer that advertises only RSA-PSS still connects.
    let our_schemes = vec![
        SignatureScheme::Ed25519.to_u8(),
        SignatureScheme::RsaPss.to_u8(),
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

    // Negotiate the best common scheme supported by both peers.
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
    )?);
    let cipher = AesCipher::new_with_role(&aes_key[..], NonceRole::Host)?;
    let transcript_hash = salt.as_slice();
    let identity_proof_aad = labeled_aad(b"identity-proof", transcript_hash);
    let transport_aad = labeled_aad(b"transport", transcript_hash);
    tracing::info!("Encrypted tunnel established (Forward Secrecy enabled)");

    // --- ENCRYPTED IDENTITY EXCHANGE ---

    // 9. Create Identity Proof (using negotiated scheme)
    // We sign our own ephemeral key to bind it to our identity (prevent MITM)
    let IdentitySignature {
        signature,
        ed25519_public,
        ed25519_binding,
    } = build_identity_signature(selected_scheme, &privkey, host_ephemeral_bytes)?;

    let my_proof = IdentityProof {
        public_key_pem: pem_encode_public(&RsaPublicKey::from(&privkey))?,
        signature,
        version: PROTOCOL_VERSION as u32,
        chat_id,
        signature_scheme: selected_scheme,
        ed25519_public,
        ed25519_binding,
    };

    // Serialize & Encrypt Proof
    let my_proof_bytes = bincode::serialize(&my_proof)?;
    let encrypted_proof = cipher.encrypt(&my_proof_bytes, Some(&identity_proof_aad));
    send_packet(&mut stream, &encrypted_proof).await?;

    // 10. Receive Client's Identity Proof (Encrypted)
    let encrypted_client_proof = recv_packet_with_timeout(&mut stream).await?;
    let client_proof_bytes = cipher
        .decrypt(&encrypted_client_proof, Some(&identity_proof_aad))
        .ok_or_else(|| anyhow!("Failed to decrypt client identity proof"))?;
    let client_proof: IdentityProof = bincode::deserialize(&client_proof_bytes)?;
    if client_proof.signature_scheme != selected_scheme {
        return Err(anyhow!(
            "Client used unexpected signature scheme: expected {}, got {}",
            selected_scheme,
            client_proof.signature_scheme
        ));
    }

    // 11. Verify Client Identity
    // Verifies under whichever scheme the peer used; an Ed25519 proof is
    // only accepted when its subkey is bound to the RSA identity that the
    // fingerprint — and therefore TOFU — is derived from.
    verify_identity_proof(&client_proof, &client_ephemeral_bytes)
        .map_err(|e| anyhow!("Client identity verification failed: {}", e))?;

    let client_fingerprint = fingerprint_pubkey(client_proof.public_key_pem.as_bytes());
    tracing::debug!("Verified client identity: {}...", &client_fingerprint[..8]);

    // Optional connection-password gate (inside the encrypted, authenticated
    // tunnel, before the peer is surfaced for verification).
    if let Err(e) = host_password_gate(
        &mut stream,
        &cipher,
        &transport_aad,
        connection_password.as_deref(),
    )
    .await
    {
        let _ = to_app_tx.send(SessionEvent::Error(e.to_string()));
        return Err(e);
    }

    // 12. Display Fingerprint & Wait for Confirmation
    to_app_tx
        .send(SessionEvent::NewConnection {
            peer_addr: peer_addr.to_string(),
            fingerprint: client_fingerprint,
            sas: crate::core::derive_sas(&transport_aad),
            chat_id: client_proof.chat_id,
        })
        .map_err(|e| anyhow!("Send error: {}", e))?;

    // Wait for user confirmation (up to 30 minutes) before proceeding
    match tokio::time::timeout(tokio::time::Duration::from_secs(1800), async {
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

    run_message_loop(
        stream,
        cipher,
        transport_aad,
        to_app_tx,
        from_app_rx,
        file_rx,
    )
    .await
}

/// Run client session: connect, handshake (v3), message loop
#[allow(clippy::too_many_arguments)]
/// Attempt a TCP connection to each candidate in order, bounding every attempt
/// with [`crate::CONNECT_ATTEMPT_TIMEOUT_SECS`] so one dead address (which
/// would otherwise hang for the OS connect timeout) cannot stall the fallback
/// to the next. Emits a `Warning` event between failed attempts so the UI can
/// show progress. Returns the connected stream plus the winning target.
pub async fn connect_first_reachable(
    targets: &[(String, u16)],
    to_app_tx: &mpsc::UnboundedSender<SessionEvent>,
) -> Result<(TcpStream, String, u16)> {
    if targets.is_empty() {
        return Err(anyhow!("No candidate addresses to connect to"));
    }
    let timeout = std::time::Duration::from_secs(crate::CONNECT_ATTEMPT_TIMEOUT_SECS);
    let mut last_err: Option<anyhow::Error> = None;
    for (i, (host, port)) in targets.iter().enumerate() {
        match tokio::time::timeout(timeout, TcpStream::connect((host.as_str(), *port))).await {
            Ok(Ok(stream)) => return Ok((stream, host.clone(), *port)),
            Ok(Err(e)) => {
                tracing::warn!(host = %host, port = %port, error = %e, "Candidate address refused/unreachable");
                last_err = Some(e.into());
            }
            Err(_) => {
                tracing::warn!(host = %host, port = %port, "Candidate address timed out");
                last_err = Some(anyhow!(
                    "Connection to {}:{} timed out after {}s",
                    host,
                    port,
                    crate::CONNECT_ATTEMPT_TIMEOUT_SECS
                ));
            }
        }
        // More candidates to go: tell the UI we're falling back.
        if i + 1 < targets.len() {
            let (next_host, next_port) = &targets[i + 1];
            let _ = to_app_tx.send(SessionEvent::Warning(format!(
                "Could not reach {}:{}, trying {}:{}…",
                host, port, next_host, next_port
            )));
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("All candidate addresses failed")))
}

#[allow(clippy::too_many_arguments)]
pub async fn run_client_session(
    host: &str,
    port: u16,
    privkey: RsaPrivateKey,
    to_app_tx: mpsc::UnboundedSender<SessionEvent>,
    from_app_rx: mpsc::UnboundedReceiver<ProtocolMessage>,
    file_rx: mpsc::Receiver<ProtocolMessage>,
    confirm_rx: mpsc::UnboundedReceiver<bool>,
    chat_id: uuid::Uuid,
    connection_password: Option<String>,
) -> Result<()> {
    run_client_session_multi(
        &[(host.to_string(), port)],
        privkey,
        to_app_tx,
        from_app_rx,
        file_rx,
        confirm_rx,
        chat_id,
        connection_password,
    )
    .await
}

/// Try each `(host, port)` candidate in priority order (bounded per-attempt
/// timeout) and run the client session over the first that accepts the TCP
/// connection. Later candidates are only tried when earlier ones fail to
/// *connect* — once a stream is established the session lives or dies there.
#[allow(clippy::too_many_arguments)]
pub async fn run_client_session_multi(
    targets: &[(String, u16)],
    privkey: RsaPrivateKey,
    to_app_tx: mpsc::UnboundedSender<SessionEvent>,
    from_app_rx: mpsc::UnboundedReceiver<ProtocolMessage>,
    file_rx: mpsc::Receiver<ProtocolMessage>,
    mut confirm_rx: mpsc::UnboundedReceiver<bool>,
    chat_id: uuid::Uuid,
    connection_password: Option<String>,
) -> Result<()> {
    // 1. Connect to the first reachable candidate
    let (mut stream, host, port) = connect_first_reachable(targets, &to_app_tx).await?;
    tracing::info!("Connected to {}:{}", host, port);
    let host = host.as_str();

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
    if host_ephemeral_bytes.len() != crate::AES_KEY_SIZE {
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

    // Send our supported schemes (same as host: RSA-PSS only).
    // Ed25519 first, RSA-PSS retained for peers that predate it —
    // `negotiate_signature_scheme` prefers Ed25519 when both sides offer it,
    // so an older peer that advertises only RSA-PSS still connects.
    let our_schemes = vec![
        SignatureScheme::Ed25519.to_u8(),
        SignatureScheme::RsaPss.to_u8(),
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
    )?);
    let cipher = AesCipher::new_with_role(&aes_key[..], NonceRole::Client)?;
    let transcript_hash = salt.as_slice();
    let identity_proof_aad = labeled_aad(b"identity-proof", transcript_hash);
    let transport_aad = labeled_aad(b"transport", transcript_hash);
    tracing::info!("Encrypted tunnel established (Forward Secrecy enabled)");

    // --- ENCRYPTED IDENTITY EXCHANGE ---

    // 8. Receive Host Identity Proof (Encrypted)
    let encrypted_host_proof = recv_packet_with_timeout(&mut stream).await?;
    let host_proof_bytes = cipher
        .decrypt(&encrypted_host_proof, Some(&identity_proof_aad))
        .ok_or_else(|| anyhow!("Failed to decrypt host identity proof"))?;
    let host_proof: IdentityProof = bincode::deserialize(&host_proof_bytes)?;
    if host_proof.signature_scheme != selected_scheme {
        return Err(anyhow!(
            "Host used unexpected signature scheme: expected {}, got {}",
            selected_scheme,
            host_proof.signature_scheme
        ));
    }

    // 9. Verify Host Identity
    // Verifies under whichever scheme the peer used; an Ed25519 proof is
    // only accepted when its subkey is bound to the RSA identity that the
    // fingerprint — and therefore TOFU — is derived from.
    verify_identity_proof(&host_proof, &host_ephemeral_bytes)
        .map_err(|e| anyhow!("Host identity verification failed: {}", e))?;

    let host_fingerprint = fingerprint_pubkey(host_proof.public_key_pem.as_bytes());
    tracing::debug!("Verified host identity: {}...", &host_fingerprint[..8]);

    // 10. Create Identity Proof (using negotiated scheme)
    let IdentitySignature {
        signature,
        ed25519_public,
        ed25519_binding,
    } = build_identity_signature(selected_scheme, &privkey, client_ephemeral_bytes)?;

    let my_proof = IdentityProof {
        public_key_pem: pem_encode_public(&RsaPublicKey::from(&privkey))?,
        signature,
        version: PROTOCOL_VERSION as u32,
        chat_id,
        signature_scheme: selected_scheme,
        ed25519_public,
        ed25519_binding,
    };

    // Serialize & Encrypt Proof
    let my_proof_bytes = bincode::serialize(&my_proof)?;
    let encrypted_proof = cipher.encrypt(&my_proof_bytes, Some(&identity_proof_aad));
    send_packet(&mut stream, &encrypted_proof).await?;

    // Answer the host's optional connection-password gate before proceeding.
    if let Err(e) = client_password_gate(
        &mut stream,
        &cipher,
        &transport_aad,
        connection_password.as_deref(),
    )
    .await
    {
        let _ = to_app_tx.send(SessionEvent::Error(e.to_string()));
        return Err(e);
    }

    // 11. Display Fingerprint & Wait for Confirmation
    to_app_tx
        .send(SessionEvent::ShowFingerprintVerification {
            fingerprint: host_fingerprint.clone(),
            peer_name: host.to_string(),
            sas: crate::core::derive_sas(&transport_aad),
            chat_id,
        })
        .map_err(|e| anyhow!("Send error: {}", e))?;

    // Wait up to 30 minutes for user confirmation
    match tokio::time::timeout(tokio::time::Duration::from_secs(1800), async {
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

    run_message_loop(
        stream,
        cipher,
        transport_aad,
        to_app_tx,
        from_app_rx,
        file_rx,
    )
    .await
}

/// A completed Protocol v3 cryptographic handshake: the peer's verified identity
/// fingerprint and chat id, plus the established AEAD tunnel (cipher + transport
/// AAD). Callers apply their own trust policy (interactive TOFU for P2P, server
/// policy for the Party server) and then run whatever message loop rides on top.
pub struct EstablishedTunnel {
    pub peer_fingerprint: String,
    pub peer_chat_id: uuid::Uuid,
    pub cipher: AesCipher,
    pub transport_aad: Vec<u8>,
    /// Which signature scheme the two peers settled on for identity proofs.
    /// Worth surfacing rather than discarding: it is the one handshake choice
    /// that varies by peer version, so it is what a diagnostic report needs to
    /// answer "why did this connection behave differently from that one".
    pub signature_scheme: SignatureScheme,
}

/// Sign our ephemeral key for the identity proof under the negotiated scheme.
///
/// Returns the signature plus the Ed25519 material that has to travel with it
/// (the subkey and its binding to this RSA identity), which is `None` for
/// RSA-PSS. Shared by every handshake variant so all four sign identically.
struct IdentitySignature {
    signature: Vec<u8>,
    /// Present only for Ed25519 proofs.
    ed25519_public: Option<Vec<u8>>,
    ed25519_binding: Option<Vec<u8>>,
}

fn build_identity_signature(
    scheme: SignatureScheme,
    privkey: &RsaPrivateKey,
    ephemeral_bytes: &[u8],
) -> Result<IdentitySignature> {
    let mut hasher = Sha256::new();
    hasher.update(b"IDENTITY_PROOF");
    hasher.update(ephemeral_bytes);
    let digest = hasher.finalize();
    match scheme {
        SignatureScheme::RsaPss => {
            let signing_key = SigningKey::<Sha256>::new(privkey.clone());
            let mut rng = OsRng;
            Ok(IdentitySignature {
                signature: signing_key.sign_with_rng(&mut rng, &digest).to_vec(),
                ed25519_public: None,
                ed25519_binding: None,
            })
        }
        SignatureScheme::Ed25519 => {
            let signing_key = derive_ed25519_subkey(privkey)?;
            let verifying = signing_key.verifying_key();
            Ok(IdentitySignature {
                signature: sign_ed25519(&signing_key, &digest).to_bytes().to_vec(),
                ed25519_public: Some(verifying.to_bytes().to_vec()),
                ed25519_binding: Some(cross_sign_ed25519(privkey, &verifying)?),
            })
        }
    }
}

/// Verify a peer's identity proof against the ephemeral key they sent.
///
/// The identity is always the RSA key in `public_key_pem` — that is what the
/// fingerprint, and therefore TOFU, is derived from, and this step does not
/// change it. An Ed25519 proof is accepted only when the subkey it was signed
/// with is itself signed by that RSA identity, so trusting a fingerprint still
/// means trusting exactly one key.
fn verify_identity_proof(proof: &IdentityProof, peer_ephemeral_bytes: &[u8]) -> Result<()> {
    let peer_pubkey = pem_decode_public(&proof.public_key_pem)?;
    let mut hasher = Sha256::new();
    hasher.update(b"IDENTITY_PROOF");
    hasher.update(peer_ephemeral_bytes);
    let digest = hasher.finalize();

    match proof.signature_scheme {
        SignatureScheme::RsaPss => {
            let verifying_key = VerifyingKey::<Sha256>::new(peer_pubkey);
            verifying_key
                .verify(
                    &digest,
                    &rsa::pss::Signature::try_from(proof.signature.as_slice())?,
                )
                .map_err(|e| anyhow!("Identity signature verification failed: {}", e))
        }
        SignatureScheme::Ed25519 => {
            let (Some(ed_public), Some(binding)) = (&proof.ed25519_public, &proof.ed25519_binding)
            else {
                return Err(anyhow!(
                    "Ed25519 proof arrived without its subkey or its binding to the identity"
                ));
            };
            let ed_public = ed25519_public_from_bytes(ed_public)?;
            // Bind first: an unbound subkey proves only that somebody holds
            // *some* Ed25519 key, which is not an identity.
            verify_ed25519_binding(&peer_pubkey, &ed_public, binding)?;
            let signature = ed25519_signature_from_bytes(&proof.signature)?;
            verify_ed25519(&ed_public, &digest, &signature)
                .map_err(|e| anyhow!("Identity signature verification failed: {}", e))
        }
    }
}

/// Run the host side of the Protocol v3 handshake over `stream`: version exchange,
/// X25519 ephemeral key exchange, session-key derivation, and the encrypted,
/// transcript-bound identity-proof exchange. Returns the established tunnel and the
/// peer's verified identity. The caller is responsible for trust policy (TOFU /
/// server policy) and the message loop.
pub async fn host_handshake<S>(
    stream: &mut S,
    privkey: &RsaPrivateKey,
    chat_id: uuid::Uuid,
) -> Result<EstablishedTunnel>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let version_bytes = (PROTOCOL_VERSION as u32).to_be_bytes();
    send_packet(stream, &version_bytes).await?;

    let client_version_bytes = recv_packet_with_timeout(stream).await?;
    if client_version_bytes.len() != 4 {
        return Err(anyhow!("Invalid version packet length"));
    }
    let client_version = u32::from_be_bytes(
        client_version_bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("Invalid version bytes"))?,
    );
    if client_version < 3 {
        return Err(anyhow!(
            "Client version {} too old (need v3+)",
            client_version
        ));
    }

    let (host_ephemeral_secret, host_ephemeral_public) = generate_ephemeral_keypair();
    let host_ephemeral_bytes = host_ephemeral_public.as_bytes();
    send_packet(stream, host_ephemeral_bytes).await?;

    let client_ephemeral_bytes = recv_packet_with_timeout(stream).await?;
    if client_ephemeral_bytes.len() != crate::AES_KEY_SIZE {
        return Err(anyhow!("Invalid ephemeral key length"));
    }
    let client_ephemeral_public = parse_x25519_public(&client_ephemeral_bytes)?;

    // Ed25519 first, RSA-PSS retained for peers that predate it —
    // `negotiate_signature_scheme` prefers Ed25519 when both sides offer it,
    // so an older peer that advertises only RSA-PSS still connects.
    let our_schemes = vec![
        SignatureScheme::Ed25519.to_u8(),
        SignatureScheme::RsaPss.to_u8(),
    ];
    let schemes_msg = ProtocolMessage::SupportedSignatureSchemes {
        schemes: our_schemes.clone(),
    };
    send_packet(stream, &schemes_msg.to_plain_bytes()).await?;

    let client_schemes_bytes = recv_packet_with_timeout(stream).await?;
    let client_schemes = match ProtocolMessage::from_plain_bytes(&client_schemes_bytes) {
        Some(ProtocolMessage::SupportedSignatureSchemes { schemes }) => schemes,
        _ => return Err(anyhow!("Client did not send valid signature schemes")),
    };
    let selected_scheme = negotiate_signature_scheme(&our_schemes, &client_schemes)
        .ok_or_else(|| anyhow!("No common signature scheme found"))?;

    let mut transcript = Vec::new();
    transcript.extend_from_slice(&version_bytes);
    transcript.extend_from_slice(&client_version_bytes);
    transcript.extend_from_slice(host_ephemeral_bytes);
    transcript.extend_from_slice(&client_ephemeral_bytes);
    let salt = Sha256::digest(&transcript);

    let aes_key = Zeroizing::new(derive_session_key(
        host_ephemeral_secret,
        &client_ephemeral_public,
        Some(&salt),
        HKDF_INFO,
    )?);
    let cipher = AesCipher::new_with_role(&aes_key[..], NonceRole::Host)?;
    let transcript_hash = salt.as_slice();
    let identity_proof_aad = labeled_aad(b"identity-proof", transcript_hash);
    let transport_aad = labeled_aad(b"transport", transcript_hash);

    let IdentitySignature {
        signature,
        ed25519_public,
        ed25519_binding,
    } = build_identity_signature(selected_scheme, privkey, host_ephemeral_bytes)?;

    let my_proof = IdentityProof {
        public_key_pem: pem_encode_public(&RsaPublicKey::from(privkey))?,
        signature,
        version: PROTOCOL_VERSION as u32,
        chat_id,
        signature_scheme: selected_scheme,
        ed25519_public,
        ed25519_binding,
    };

    let my_proof_bytes = bincode::serialize(&my_proof)?;
    let encrypted_proof = cipher.encrypt(&my_proof_bytes, Some(&identity_proof_aad));
    send_packet(stream, &encrypted_proof).await?;

    let encrypted_client_proof = recv_packet_with_timeout(stream).await?;
    let client_proof_bytes = cipher
        .decrypt(&encrypted_client_proof, Some(&identity_proof_aad))
        .ok_or_else(|| anyhow!("Failed to decrypt client identity proof"))?;
    let client_proof: IdentityProof = bincode::deserialize(&client_proof_bytes)?;
    if client_proof.signature_scheme != selected_scheme {
        return Err(anyhow!(
            "Client used unexpected signature scheme: expected {}, got {}",
            selected_scheme,
            client_proof.signature_scheme
        ));
    }

    // Verifies under whichever scheme the peer used; an Ed25519 proof is only
    // accepted when its subkey is bound to the RSA identity that the
    // fingerprint — and therefore TOFU — is derived from.
    verify_identity_proof(&client_proof, &client_ephemeral_bytes)
        .map_err(|e| anyhow!("Client identity verification failed: {}", e))?;

    let client_fingerprint = fingerprint_pubkey(client_proof.public_key_pem.as_bytes());

    Ok(EstablishedTunnel {
        peer_fingerprint: client_fingerprint,
        peer_chat_id: client_proof.chat_id,
        cipher,
        transport_aad,
        signature_scheme: selected_scheme,
    })
}

/// Run the client side of the Protocol v3 handshake over `stream`. Mirror of
/// [`host_handshake`]; returns the established tunnel and the host's verified
/// identity (`peer_chat_id` is the host's advertised chat id).
pub async fn client_handshake<S>(
    stream: &mut S,
    privkey: &RsaPrivateKey,
    chat_id: uuid::Uuid,
) -> Result<EstablishedTunnel>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let host_version_bytes = recv_packet_with_timeout(stream).await?;
    if host_version_bytes.len() != 4 {
        return Err(anyhow!("Invalid version packet length"));
    }
    let host_version = u32::from_be_bytes(
        host_version_bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("Invalid version bytes"))?,
    );
    if host_version < 3 {
        return Err(anyhow!("Host version {} too old (need v3+)", host_version));
    }

    let version_bytes = (PROTOCOL_VERSION as u32).to_be_bytes();
    send_packet(stream, &version_bytes).await?;

    let host_ephemeral_bytes = recv_packet_with_timeout(stream).await?;
    if host_ephemeral_bytes.len() != crate::AES_KEY_SIZE {
        return Err(anyhow!("Invalid ephemeral key length"));
    }
    let host_ephemeral_public = parse_x25519_public(&host_ephemeral_bytes)?;

    let (client_ephemeral_secret, client_ephemeral_public) = generate_ephemeral_keypair();
    let client_ephemeral_bytes = client_ephemeral_public.as_bytes();
    send_packet(stream, client_ephemeral_bytes).await?;

    let host_schemes_bytes = recv_packet_with_timeout(stream).await?;
    let host_schemes = match ProtocolMessage::from_plain_bytes(&host_schemes_bytes) {
        Some(ProtocolMessage::SupportedSignatureSchemes { schemes }) => schemes,
        _ => return Err(anyhow!("Host did not send valid signature schemes")),
    };

    // Ed25519 first, RSA-PSS retained for peers that predate it —
    // `negotiate_signature_scheme` prefers Ed25519 when both sides offer it,
    // so an older peer that advertises only RSA-PSS still connects.
    let our_schemes = vec![
        SignatureScheme::Ed25519.to_u8(),
        SignatureScheme::RsaPss.to_u8(),
    ];
    let schemes_msg = ProtocolMessage::SupportedSignatureSchemes {
        schemes: our_schemes.clone(),
    };
    send_packet(stream, &schemes_msg.to_plain_bytes()).await?;

    let selected_scheme = negotiate_signature_scheme(&host_schemes, &our_schemes)
        .ok_or_else(|| anyhow!("No common signature scheme found"))?;

    let mut transcript = Vec::new();
    transcript.extend_from_slice(&host_version_bytes);
    transcript.extend_from_slice(&version_bytes);
    transcript.extend_from_slice(&host_ephemeral_bytes);
    transcript.extend_from_slice(client_ephemeral_bytes);
    let salt = Sha256::digest(&transcript);

    let aes_key = Zeroizing::new(derive_session_key(
        client_ephemeral_secret,
        &host_ephemeral_public,
        Some(&salt),
        HKDF_INFO,
    )?);
    let cipher = AesCipher::new_with_role(&aes_key[..], NonceRole::Client)?;
    let transcript_hash = salt.as_slice();
    let identity_proof_aad = labeled_aad(b"identity-proof", transcript_hash);
    let transport_aad = labeled_aad(b"transport", transcript_hash);

    let encrypted_host_proof = recv_packet_with_timeout(stream).await?;
    let host_proof_bytes = cipher
        .decrypt(&encrypted_host_proof, Some(&identity_proof_aad))
        .ok_or_else(|| anyhow!("Failed to decrypt host identity proof"))?;
    let host_proof: IdentityProof = bincode::deserialize(&host_proof_bytes)?;
    if host_proof.signature_scheme != selected_scheme {
        return Err(anyhow!(
            "Host used unexpected signature scheme: expected {}, got {}",
            selected_scheme,
            host_proof.signature_scheme
        ));
    }

    // Verifies under whichever scheme the peer used; an Ed25519 proof is only
    // accepted when its subkey is bound to the RSA identity that the
    // fingerprint — and therefore TOFU — is derived from.
    verify_identity_proof(&host_proof, &host_ephemeral_bytes)
        .map_err(|e| anyhow!("Host identity verification failed: {}", e))?;

    let host_fingerprint = fingerprint_pubkey(host_proof.public_key_pem.as_bytes());

    let IdentitySignature {
        signature,
        ed25519_public,
        ed25519_binding,
    } = build_identity_signature(selected_scheme, privkey, client_ephemeral_bytes)?;

    let my_proof = IdentityProof {
        public_key_pem: pem_encode_public(&RsaPublicKey::from(privkey))?,
        signature,
        version: PROTOCOL_VERSION as u32,
        chat_id,
        signature_scheme: selected_scheme,
        ed25519_public,
        ed25519_binding,
    };
    let my_proof_bytes = bincode::serialize(&my_proof)?;
    let encrypted_proof = cipher.encrypt(&my_proof_bytes, Some(&identity_proof_aad));
    send_packet(stream, &encrypted_proof).await?;

    Ok(EstablishedTunnel {
        peer_fingerprint: host_fingerprint,
        peer_chat_id: host_proof.chat_id,
        cipher,
        transport_aad,
        signature_scheme: selected_scheme,
    })
}

/// Constant-time comparison so a wrong connection password cannot be recovered
/// byte-by-byte via response timing.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Host side of the optional connection-password gate, run over the established
/// tunnel right after the handshake (so it is confidential and transcript-bound)
/// and before the peer is surfaced for TOFU. The host announces whether a password
/// is required; the client replies with its (encrypted) password. A wrong password
/// returns an error and the caller drops the connection.
async fn host_password_gate<S>(
    stream: &mut S,
    cipher: &AesCipher,
    aad: &[u8],
    expected: Option<&str>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let flag = [u8::from(expected.is_some())];
    send_packet(stream, &cipher.encrypt(&flag, Some(aad))).await?;
    let resp_ct = recv_packet_with_timeout(stream).await?;
    let resp = cipher
        .decrypt(&resp_ct, Some(aad))
        .ok_or_else(|| anyhow!("Failed to decrypt password response"))?;
    if let Some(pw) = expected {
        if !constant_time_eq(&resp, pw.as_bytes()) {
            return Err(anyhow!("Incorrect connection password"));
        }
    }
    Ok(())
}

/// Client side of the connection-password gate (mirror of [`host_password_gate`]).
async fn client_password_gate<S>(
    stream: &mut S,
    cipher: &AesCipher,
    aad: &[u8],
    password: Option<&str>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let flag_ct = recv_packet_with_timeout(stream).await?;
    let flag = cipher
        .decrypt(&flag_ct, Some(aad))
        .ok_or_else(|| anyhow!("Failed to decrypt password challenge"))?;
    let required = flag.first().copied() == Some(1);
    // Always reply (even empty) so the host's receive completes and both sides
    // converge on the same outcome.
    let payload: Vec<u8> = if required {
        password.map(|p| p.as_bytes().to_vec()).unwrap_or_default()
    } else {
        Vec::new()
    };
    send_packet(stream, &cipher.encrypt(&payload, Some(aad))).await?;
    if required && password.is_none() {
        return Err(anyhow!("This host requires a connection password"));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn run_host_session_over_stream<S>(
    stream: &mut S,
    peer_label: String,
    privkey: RsaPrivateKey,
    to_app_tx: mpsc::UnboundedSender<SessionEvent>,
    from_app_rx: mpsc::UnboundedReceiver<ProtocolMessage>,
    file_rx: mpsc::Receiver<ProtocolMessage>,
    mut confirm_rx: mpsc::UnboundedReceiver<bool>,
    chat_id: uuid::Uuid,
    connection_password: Option<String>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let EstablishedTunnel {
        peer_fingerprint,
        peer_chat_id,
        cipher,
        transport_aad,
        signature_scheme: _,
    } = host_handshake(stream, &privkey, chat_id).await?;

    // Optional connection-password gate (inside the encrypted, authenticated tunnel).
    if let Err(e) = host_password_gate(
        stream,
        &cipher,
        &transport_aad,
        connection_password.as_deref(),
    )
    .await
    {
        let _ = to_app_tx.send(SessionEvent::Error(e.to_string()));
        return Err(e);
    }

    to_app_tx
        .send(SessionEvent::NewConnection {
            peer_addr: peer_label,
            fingerprint: peer_fingerprint,
            sas: crate::core::derive_sas(&transport_aad),
            chat_id: peer_chat_id,
        })
        .map_err(|e| anyhow!("Send error: {}", e))?;

    match tokio::time::timeout(tokio::time::Duration::from_secs(1800), async {
        confirm_rx.recv().await
    })
    .await
    {
        Ok(Some(true)) => {}
        Ok(Some(false)) => {
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

    to_app_tx
        .send(SessionEvent::Ready)
        .map_err(|e| anyhow!("Send error: {}", e))?;
    run_message_loop(
        stream,
        cipher,
        transport_aad,
        to_app_tx,
        from_app_rx,
        file_rx,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn run_client_session_over_stream<S>(
    stream: &mut S,
    peer_label: String,
    privkey: RsaPrivateKey,
    to_app_tx: mpsc::UnboundedSender<SessionEvent>,
    from_app_rx: mpsc::UnboundedReceiver<ProtocolMessage>,
    file_rx: mpsc::Receiver<ProtocolMessage>,
    mut confirm_rx: mpsc::UnboundedReceiver<bool>,
    chat_id: uuid::Uuid,
    connection_password: Option<String>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let peer_name = peer_label.clone();
    to_app_tx
        .send(SessionEvent::Connected { peer: peer_label })
        .map_err(|e| anyhow!("Send error: {}", e))?;

    let EstablishedTunnel {
        peer_fingerprint: host_fingerprint,
        peer_chat_id: _,
        cipher,
        transport_aad,
        signature_scheme: _,
    } = client_handshake(stream, &privkey, chat_id).await?;

    // Answer the host's optional connection-password gate before proceeding.
    if let Err(e) = client_password_gate(
        stream,
        &cipher,
        &transport_aad,
        connection_password.as_deref(),
    )
    .await
    {
        let _ = to_app_tx.send(SessionEvent::Error(e.to_string()));
        return Err(e);
    }

    to_app_tx
        .send(SessionEvent::ShowFingerprintVerification {
            fingerprint: host_fingerprint,
            peer_name,
            sas: crate::core::derive_sas(&transport_aad),
            chat_id,
        })
        .map_err(|e| anyhow!("Send error: {}", e))?;

    match tokio::time::timeout(tokio::time::Duration::from_secs(1800), async {
        confirm_rx.recv().await
    })
    .await
    {
        Ok(Some(true)) => {}
        Ok(Some(false)) => {
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

    to_app_tx
        .send(SessionEvent::Ready)
        .map_err(|e| anyhow!("Send error: {}", e))?;
    run_message_loop(
        stream,
        cipher,
        transport_aad,
        to_app_tx,
        from_app_rx,
        file_rx,
    )
    .await
}

/// Extract sequence number from a ProtocolMessage
/// Returns None if the message type doesn't have a sequence number (shouldn't happen in practice)
fn extract_sequence(msg: &ProtocolMessage) -> Option<u64> {
    match msg {
        ProtocolMessage::Version { .. } => None, // Handshake only
        ProtocolMessage::EphemeralKey { .. } => None, // Handshake only
        ProtocolMessage::SupportedSignatureSchemes { .. } => None, // Handshake only
        ProtocolMessage::Text { seq, .. } => Some(*seq),
        ProtocolMessage::TextChunk { seq, .. } => Some(*seq),
        ProtocolMessage::FileMeta { seq, .. } => Some(*seq),
        ProtocolMessage::FileChunk { seq, .. } => Some(*seq),
        ProtocolMessage::FileEnd { seq } => Some(*seq),
        ProtocolMessage::FileCancel { seq } => Some(*seq),
        ProtocolMessage::Ping { seq } => Some(*seq),
        ProtocolMessage::TypingStart { seq } => Some(*seq),
        ProtocolMessage::TypingStop { seq } => Some(*seq),
        ProtocolMessage::Rekey { seq, .. } => Some(*seq),
        ProtocolMessage::Ack { seq, .. } => Some(*seq),
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

/// Send a `Rekey` frame and rotate to the next session key.
///
/// Only ONE side of a session (the host) ever initiates a rekey. If both sides
/// could initiate, they might rekey in the same round trip — each applying its
/// own new key before the other's `Rekey` (encrypted under the *old* key)
/// arrives — and the next frame would be undecryptable, tearing the session
/// down. A single deterministic initiator makes that race impossible; the
/// other side rekeys only in response to a received `Rekey`.
async fn initiate_rekey<S>(
    stream: &mut S,
    cipher: &mut AesCipher,
    prev_cipher: &mut Option<AesCipher>,
    transport_aad: &[u8],
    sent_seq: &mut u64,
    messages_since_rekey: &mut u64,
    last_rekey_time: &mut Instant,
) -> Result<()>
where
    S: AsyncWrite + Unpin,
{
    use crate::core::{generate_rekey_nonce, rekey_session_key};
    let nonce = generate_rekey_nonce();
    *sent_seq += 1;
    let rekey_msg = ProtocolMessage::Rekey {
        nonce: nonce.to_vec(),
        seq: *sent_seq,
    };
    let encrypted = cipher.encrypt(&rekey_msg.to_plain_bytes(), Some(transport_aad));
    send_packet(stream, &encrypted).await?;
    // Apply the new key only after the frame is on the wire (encrypted under the
    // current key), so the peer can still decrypt the Rekey itself.
    let next_key = rekey_session_key(&cipher.get_current_key(), &nonce);
    let role = cipher.role();
    // Retain the outgoing side's *old* key for the receive path: the peer keeps
    // sending under it until it processes this Rekey, so frames encrypted with
    // the old key can still be in flight. The window closes as soon as a frame
    // decrypts under the new key (proof the peer has switched). See the receive
    // branch in run_message_loop.
    *prev_cipher = Some(cipher.clone());
    *cipher = AesCipher::new_with_role(&next_key, role)?;
    *messages_since_rekey = 0;
    *last_rekey_time = Instant::now();
    tracing::info!("Session key rotated (initiated rekey)");
    Ok(())
}

/// Main message loop: send and receive encrypted messages with replay protection
/// Encrypt and write one outbound application frame, first rotating the key if a
/// host-initiated rekey is due. Shared by the control lane and the bounded
/// file-data lane so both stamp the single monotonic outbound sequence and both
/// surface `FileSendComplete` on the final file frame.
#[allow(clippy::too_many_arguments)]
async fn send_outbound_frame<S>(
    stream: &mut S,
    cipher: &mut AesCipher,
    prev_cipher: &mut Option<AesCipher>,
    transport_aad: &[u8],
    to_app_tx: &mpsc::UnboundedSender<SessionEvent>,
    sent_seq: &mut u64,
    messages_since_rekey: &mut u64,
    last_rekey_time: &mut std::time::Instant,
    is_rekey_initiator: bool,
    rekey_message_count: u64,
    rekey_time_interval: std::time::Duration,
    mut msg: ProtocolMessage,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    // Host-only rekey initiation (see initiate_rekey): rotate the key before
    // sending this frame if we're due.
    let should_rekey = is_rekey_initiator
        && (*messages_since_rekey >= rekey_message_count
            || last_rekey_time.elapsed() >= rekey_time_interval);
    if should_rekey {
        initiate_rekey(
            stream,
            cipher,
            prev_cipher,
            transport_aad,
            sent_seq,
            messages_since_rekey,
            last_rekey_time,
        )
        .await
        .map_err(|e| anyhow!("Network send error (rekey): {}", e))?;
    }

    // Stamp the next monotonic transport sequence onto every outgoing frame. The
    // loop owns the outbound sequence space so application frames and interleaved
    // Rekey frames form one strictly-increasing stream the peer's replay check
    // accepts. Interleaving the two lanes is safe: each frame still gets the next
    // seq in the order it is written.
    *sent_seq += 1;
    msg.set_seq(*sent_seq);
    tracing::debug!("Sending message: {:?}", msg);

    let plaintext = msg.to_plain_bytes();
    let encrypted = cipher.encrypt(&plaintext, Some(transport_aad));
    send_packet(stream, &encrypted)
        .await
        .map_err(|e| anyhow!("Network send error: {}", e))?;

    *messages_since_rekey += 1;
    // Report the transport seq of load-bearing final frames back to the app:
    // FileEnd ("queueing is not delivery"), and the frame carrying an outgoing
    // text message (single frame, or the final chunk of a large one) so the app
    // can correlate the peer's delivery receipt (`Ack { acked_seq }`) back to
    // that message. Frames drain FIFO per lane, so order correlates.
    match &msg {
        ProtocolMessage::FileEnd { seq } => {
            let _ = to_app_tx.send(SessionEvent::FileSendComplete { seq: *seq });
        }
        ProtocolMessage::Text { seq, .. } => {
            let _ = to_app_tx.send(SessionEvent::TextSendComplete { seq: *seq });
        }
        ProtocolMessage::TextChunk {
            seq,
            chunk_index,
            total_chunks,
            ..
        } if chunk_index + 1 == *total_chunks => {
            let _ = to_app_tx.send(SessionEvent::TextSendComplete { seq: *seq });
        }
        _ => {}
    }
    Ok(())
}

async fn run_message_loop<S>(
    mut stream: S,
    mut cipher: AesCipher,
    transport_aad: Vec<u8>,
    to_app_tx: mpsc::UnboundedSender<SessionEvent>,
    mut from_app_rx: mpsc::UnboundedReceiver<ProtocolMessage>,
    mut file_rx: mpsc::Receiver<ProtocolMessage>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    const RECV_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300); // 5 minutes
                                                                                        // Keep-alive pings run well inside the peer's idle window (2 pings per
                                                                                        // 300 s), so a quiet-but-healthy session is never torn down. Before this,
                                                                                        // two connected peers who simply didn't type for five minutes were
                                                                                        // disconnected by each side's receive timeout.
    const KEEPALIVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(120);

    // Key rotation constants
    const REKEY_MESSAGE_COUNT: u64 = 100; // Rekey every 100 messages
    const REKEY_TIME_INTERVAL: std::time::Duration = std::time::Duration::from_secs(300); // 5 minutes

    // Test hooks: integration tests shrink these windows (seconds) to verify the
    // keep-alive behavior in wall-clock-realistic time. Never set in production.
    let recv_idle_timeout = duration_from_env("P2PEM_TEST_IDLE_TIMEOUT_SECS", RECV_IDLE_TIMEOUT);
    let keepalive_interval = duration_from_env("P2PEM_TEST_KEEPALIVE_SECS", KEEPALIVE_INTERVAL);

    // Track last valid sequence number to detect replays and out-of-order messages
    // This is enforced at the transport layer, not just in the app layer
    let mut last_valid_seq: u64 = 0;

    // Dedicated counter for outgoing messages (including Rekey messages)
    let mut sent_seq: u64 = 0;

    // Track messages since last rekey for key rotation
    let mut messages_since_rekey: u64 = 0;
    let mut last_rekey_time = std::time::Instant::now();

    // Only the host initiates rekeys, so the two peers can never rekey
    // simultaneously and desync their keys (the client rekeys only in response
    // to a received `Rekey`). Role is fixed for the life of the session and
    // preserved across rekeys, so this is stable.
    let is_rekey_initiator = cipher.role() == NonceRole::Host;

    // The key in force *before* the most recent rotation, kept for a bounded
    // window so frames the peer sent under the old key (still in flight when we
    // rotated) remain decryptable. Cleared the moment a frame decrypts under the
    // current key — proof the peer has switched and no old-key frames remain.
    let mut prev_cipher: Option<AesCipher> = None;

    // First tick only after a full interval (interval() would fire immediately).
    let mut keepalive = tokio::time::interval_at(
        tokio::time::Instant::now() + keepalive_interval,
        keepalive_interval,
    );
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            // Receive from network with timeout
            result = tokio::time::timeout(recv_idle_timeout, recv_packet(&mut stream)) => {
                match result {
                    Ok(Ok(encrypted)) => {
                        tracing::trace!("Received {} bytes encrypted", encrypted.len());

                        // Try the current key first. If it works, the peer has
                        // switched to it, so any retained previous key can be
                        // dropped (its in-flight window is closed). If it fails,
                        // fall back to the previous key for frames the peer sent
                        // under the old key before it processed our Rekey.
                        let decrypted = match cipher.decrypt(&encrypted, Some(&transport_aad)) {
                            Some(pt) => {
                                prev_cipher = None;
                                Some(pt)
                            }
                            None => prev_cipher
                                .as_ref()
                                .and_then(|pc| pc.decrypt(&encrypted, Some(&transport_aad))),
                        };

                        if let Some(plaintext) = decrypted {
                            tracing::trace!("Decrypted {} bytes", plaintext.len());

                            if let Some(msg) = ProtocolMessage::from_plain_bytes(&plaintext) {
                                tracing::debug!("Received message: {:?}", msg);

                                // --- TRANSPORT-LAYER REPLAY PROTECTION ---
                                // Extract sequence number from message and validate it
                                match validate_message_sequence(&msg, &mut last_valid_seq) {
                                    Ok(_) => {
                                        // Sequence is valid
                                        // Check if this is a rekey message and handle it
                                        if let ProtocolMessage::Rekey { nonce, .. } = &msg {
                                            // Handle rekeying
                                            use crate::core::rekey_session_key;

                                            let received_nonce: [u8; crate::REKEY_NONCE_SIZE] = nonce.as_slice().try_into()
                                                .map_err(|_| anyhow!("Invalid nonce length"))?;

                                            let current_key_bytes = cipher.get_current_key();
                                            let next_key = rekey_session_key(&current_key_bytes, &received_nonce);
                                            // Keep the old key briefly so any of our
                                            // own not-yet-acknowledged frames or a
                                            // reordered old-key frame still decrypt;
                                            // dropped on the first new-key frame.
                                            prev_cipher = Some(cipher.clone());
                                            cipher = AesCipher::new_with_role(&next_key, cipher.role())?;
                                            messages_since_rekey = 0;
                                            last_rekey_time = std::time::Instant::now();
                                            tracing::info!("Session key rotated (received Rekey message)");
                                            // Don't emit the Rekey message to the app
                                        } else if matches!(msg, ProtocolMessage::Ping { .. }) {
                                            // Keep-alive: transport-level plumbing that
                                            // only exists to defeat the idle timeout —
                                            // consume it here. (The app layer also
                                            // tolerates Ping for peers that still
                                            // forward it.)
                                            tracing::trace!("Keep-alive ping received");
                                        } else {
                                            // Track message count for rekeying
                                            messages_since_rekey += 1;

                                            // Emit the message
                                            if let Err(e) = to_app_tx.send(SessionEvent::MessageReceived(msg)) {
                                                tracing::error!("Failed to send MessageReceived event: {}", e);
                                                return Err(anyhow!("Event channel closed: {}", e));
                                            }
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
                            // Undecryptable under BOTH the current and the retained
                            // previous key over a reliable, ordered stream means the
                            // channel is genuinely desynced or tampered with — it
                            // cannot recover, so fail closed instead of spinning on
                            // every subsequent (also-undecryptable) packet until the
                            // idle timeout.
                            tracing::error!("Decryption failed - possible tampering or key mismatch!");
                            let _ = to_app_tx.send(SessionEvent::Error("Decryption failed!".to_string()));
                            break;
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
                        let err_msg = format!("Receive idle timeout ({}s)", recv_idle_timeout.as_secs());
                        tracing::warn!("{}", err_msg);
                        let _ = to_app_tx.send(SessionEvent::Error(err_msg));
                        break;
                    }
                }
            }

            // Send from the control lane: text, typing, and file-transfer
            // control frames (FileCancel). Unbounded, so an abort is never stuck
            // behind queued bulk data.
            Some(msg) = from_app_rx.recv() => {
                if let Err(e) = send_outbound_frame(
                    &mut stream, &mut cipher, &mut prev_cipher, &transport_aad,
                    &to_app_tx, &mut sent_seq, &mut messages_since_rekey,
                    &mut last_rekey_time, is_rekey_initiator,
                    REKEY_MESSAGE_COUNT, REKEY_TIME_INTERVAL, msg,
                ).await {
                    tracing::error!("{}", e);
                    let _ = to_app_tx.send(SessionEvent::Error(e.to_string()));
                    break;
                }
            }

            // Send from the bounded file-data lane: FileMeta/FileChunk/FileEnd.
            // The bounded capacity is what applies backpressure to the streaming
            // task, so a slow peer paces the disk reader instead of letting the
            // whole file pile up in memory.
            Some(msg) = file_rx.recv() => {
                if let Err(e) = send_outbound_frame(
                    &mut stream, &mut cipher, &mut prev_cipher, &transport_aad,
                    &to_app_tx, &mut sent_seq, &mut messages_since_rekey,
                    &mut last_rekey_time, is_rekey_initiator,
                    REKEY_MESSAGE_COUNT, REKEY_TIME_INTERVAL, msg,
                ).await {
                    tracing::error!("{}", e);
                    let _ = to_app_tx.send(SessionEvent::Error(e.to_string()));
                    break;
                }
            }

            // Keep-alive: ping on the shared outbound sequence so the peer's
            // receive-idle timer resets even when nobody is typing. If a rekey
            // is due, the host rotates here instead of pinging — otherwise a
            // silent-but-receiving host would never rekey (its send branch
            // never fires), quietly weakening forward secrecy. The Rekey frame
            // doubles as keep-alive traffic.
            _ = keepalive.tick() => {
                let rekey_due = is_rekey_initiator
                    && (messages_since_rekey >= REKEY_MESSAGE_COUNT
                        || last_rekey_time.elapsed() >= REKEY_TIME_INTERVAL);
                if rekey_due {
                    if let Err(e) = initiate_rekey(
                        &mut stream,
                        &mut cipher,
                        &mut prev_cipher,
                        &transport_aad,
                        &mut sent_seq,
                        &mut messages_since_rekey,
                        &mut last_rekey_time,
                    )
                    .await
                    {
                        let err_msg = format!("Network send error (rekey): {}", e);
                        tracing::error!("{}", err_msg);
                        let _ = to_app_tx.send(SessionEvent::Error(err_msg));
                        break;
                    }
                } else {
                    sent_seq += 1;
                    let ping = ProtocolMessage::Ping { seq: sent_seq };
                    let encrypted = cipher.encrypt(&ping.to_plain_bytes(), Some(&transport_aad));
                    if let Err(e) = send_packet(&mut stream, &encrypted).await {
                        let err_msg = format!("Network send error (keep-alive): {}", e);
                        tracing::error!("{}", err_msg);
                        let _ = to_app_tx.send(SessionEvent::Error(err_msg));
                        break;
                    }
                    tracing::trace!("Keep-alive ping sent");
                }
            }
        }
    }

    to_app_tx
        .send(SessionEvent::Disconnected)
        .map_err(|e| anyhow!("Send error: {}", e))?;

    Ok(())
}

/// Read a duration override (whole seconds) from an environment variable, for
/// integration tests that need realistic-wall-clock verification of the
/// keep-alive/idle behavior. Falls back to `default` when unset or invalid.
fn duration_from_env(var: &str, default: std::time::Duration) -> std::time::Duration {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(std::time::Duration::from_secs)
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{generate_rsa_keypair, IdentityProof};
    use crate::RSA_KEY_BITS;
    use anyhow::Result;
    use rand::RngCore;

    #[tokio::test]
    async fn initiate_rekey_frame_decryptable_by_peer_and_keys_agree() {
        // The initiator sends the Rekey encrypted under the CURRENT key and only
        // then rotates, so a peer still holding the old key can decrypt it and
        // derive the same next key. This is the invariant the deterministic
        // single-initiator design relies on.
        let key = [7u8; crate::AES_KEY_SIZE];
        let mut host = AesCipher::new_with_role(&key, NonceRole::Host).unwrap();
        let peer = AesCipher::new_with_role(&key, NonceRole::Client).unwrap();
        let aad = b"transport-aad".to_vec();

        let (mut a, mut b) = tokio::io::duplex(4096);
        let mut sent_seq = 5u64;
        let mut msgs = 9u64;
        let mut last = Instant::now();
        let mut prev: Option<AesCipher> = None;

        initiate_rekey(
            &mut a,
            &mut host,
            &mut prev,
            &aad,
            &mut sent_seq,
            &mut msgs,
            &mut last,
        )
        .await
        .unwrap();
        // The initiator retains the pre-rotation key for the in-flight window.
        assert!(prev.is_some(), "initiator must retain the previous key");

        // Peer reads the frame and decrypts it with the OLD key.
        let frame = recv_packet(&mut b).await.unwrap();
        let pt = peer
            .decrypt(&frame, Some(&aad))
            .expect("peer must decrypt the Rekey with the pre-rotation key");
        let nonce = match ProtocolMessage::from_plain_bytes(&pt).unwrap() {
            ProtocolMessage::Rekey { nonce, seq } => {
                assert_eq!(seq, 6, "rekey uses the next monotonic sequence");
                nonce
            }
            other => panic!("expected Rekey, got {:?}", other),
        };
        let n: [u8; crate::REKEY_NONCE_SIZE] = nonce.as_slice().try_into().unwrap();
        let peer_next = crate::core::rekey_session_key(&peer.get_current_key(), &n);

        // Both sides land on the same next key, and the initiator reset its
        // counters.
        assert_eq!(host.get_current_key(), peer_next);
        assert_eq!(sent_seq, 6);
        assert_eq!(msgs, 0);
    }

    #[tokio::test]
    async fn old_key_in_flight_frames_survive_a_rekey() {
        // Reproduces the race CodeRabbit flagged: after the initiator rotates,
        // a frame the peer sent under the OLD key (still in flight) must remain
        // decryptable via the retained previous key, and the window must close
        // once a NEW-key frame proves the peer has switched.
        let key = [3u8; crate::AES_KEY_SIZE];
        let mut host = AesCipher::new_with_role(&key, NonceRole::Host).unwrap();
        let client_old = AesCipher::new_with_role(&key, NonceRole::Client).unwrap();
        let aad = b"aad".to_vec();

        let (mut a, mut b) = tokio::io::duplex(4096);
        let mut sent_seq = 0u64;
        let mut msgs = 0u64;
        let mut last = Instant::now();
        let mut prev: Option<AesCipher> = None;

        initiate_rekey(
            &mut a,
            &mut host,
            &mut prev,
            &aad,
            &mut sent_seq,
            &mut msgs,
            &mut last,
        )
        .await
        .unwrap();
        let _ = recv_packet(&mut b).await.unwrap(); // drain the Rekey frame

        // Mirror of the receive branch's dual-key decrypt.
        let decrypt = |host: &AesCipher, prev: &mut Option<AesCipher>, frame: &[u8]| match host
            .decrypt(frame, Some(&aad))
        {
            Some(pt) => {
                *prev = None;
                Some(pt)
            }
            None => prev
                .as_ref()
                .and_then(|pc: &AesCipher| pc.decrypt(frame, Some(&aad))),
        };

        // Old-key in-flight frame: fails under the new key, decrypts via prev.
        let old_frame = client_old.encrypt(b"hello-old", Some(&aad));
        assert_eq!(
            decrypt(&host, &mut prev, &old_frame).as_deref(),
            Some(&b"hello-old"[..]),
            "old-key in-flight frame must decrypt via the retained key"
        );
        assert!(prev.is_some(), "prev retained while old-key frames arrive");

        // Peer has now switched: a new-key frame decrypts under the current key
        // and closes the window.
        let client_new =
            AesCipher::new_with_role(&host.get_current_key(), NonceRole::Client).unwrap();
        let new_frame = client_new.encrypt(b"hello-new", Some(&aad));
        assert_eq!(
            decrypt(&host, &mut prev, &new_frame).as_deref(),
            Some(&b"hello-new"[..])
        );
        assert!(prev.is_none(), "prev dropped once a new-key frame arrives");
    }

    #[tokio::test]
    async fn connect_first_reachable_falls_back_to_next_candidate() {
        // A live listener…
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let live_port = listener.local_addr().unwrap().port();
        // …and a dead candidate: bind-then-drop guarantees the port was free a
        // moment ago, so connecting to it gets an immediate RST (fast fail).
        let dead_port = {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap().port()
        };

        let (tx, mut rx) = mpsc::unbounded_channel();
        let targets = vec![
            ("127.0.0.1".to_string(), dead_port),
            ("127.0.0.1".to_string(), live_port),
        ];
        let (_stream, host, port) = connect_first_reachable(&targets, &tx)
            .await
            .expect("must fall back to the live candidate");
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, live_port);

        // A Warning must have been emitted for the failed first candidate.
        match rx.try_recv() {
            Ok(SessionEvent::Warning(msg)) => {
                assert!(msg.contains(&dead_port.to_string()));
                assert!(msg.contains(&live_port.to_string()));
            }
            other => panic!("expected Warning event, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn connect_first_reachable_errors_when_all_dead() {
        let dead_port = {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap().port()
        };
        let (tx, _rx) = mpsc::unbounded_channel();
        let targets = vec![("127.0.0.1".to_string(), dead_port)];
        assert!(connect_first_reachable(&targets, &tx).await.is_err());

        // Empty candidate list is an error, not a panic.
        assert!(connect_first_reachable(&[], &tx).await.is_err());
    }

    fn test_cipher() -> AesCipher {
        let mut key = [0u8; crate::AES_KEY_SIZE];
        rand::rngs::OsRng.fill_bytes(&mut key);
        AesCipher::new(&key).expect("random test key should be valid")
    }

    /// A frame that decrypts cleanly (tampering aside) must end the session rather
    /// than leaving the loop spinning on every subsequent packet: an AEAD failure
    /// over a reliable stream means the channel is desynced or tampered with.
    #[tokio::test]
    async fn decryption_failure_ends_the_session() {
        let cipher = test_cipher();
        let aad = b"transport".to_vec();
        let (loop_stream, mut peer_stream) = tokio::io::duplex(4096);

        let (to_app_tx, mut to_app_rx) = mpsc::unbounded_channel();
        // Keep the app->loop senders alive so the loop only exits via the network side.
        let (_from_app_tx, from_app_rx) = mpsc::unbounded_channel();
        let (_file_tx, file_rx) = mpsc::channel(1);

        let handle = tokio::spawn(async move {
            run_message_loop(loop_stream, cipher, aad, to_app_tx, from_app_rx, file_rx).await
        });

        // A well-formed packet whose contents cannot be authenticated under the key.
        send_packet(&mut peer_stream, &[0u8; 64]).await.unwrap();

        // The loop must surface the failure and terminate (not idle for minutes).
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), handle)
            .await
            .expect("loop must terminate promptly after a decryption failure");
        assert!(result.unwrap().is_ok());

        // It reported the failure before shutting down.
        let mut saw_error = false;
        while let Ok(ev) = to_app_rx.try_recv() {
            if matches!(ev, SessionEvent::Error(_) | SessionEvent::Disconnected) {
                saw_error = true;
            }
        }
        assert!(
            saw_error,
            "a decryption failure must be surfaced to the app"
        );

        // Keep the peer end alive until the assertions run.
        drop(peer_stream);
    }

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
            )
            .unwrap();
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
                ed25519_public: None,
                ed25519_binding: None,
            };
            let my_proof_bytes = bincode::serialize(&my_proof)?;
            let identity_proof_aad = labeled_aad(b"identity-proof", salt.as_slice());
            let encrypted_proof = cipher.encrypt(&my_proof_bytes, Some(&identity_proof_aad));
            send_packet(&mut host_stream, &encrypted_proof).await?;

            // 5. Recv Client Identity Proof (Encrypted)
            let encrypted_client_proof = recv_packet(&mut host_stream).await?;
            let client_proof_bytes = cipher
                .decrypt(&encrypted_client_proof, Some(&identity_proof_aad))
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
            )
            .unwrap();
            let cipher = AesCipher::new(&client_aes_key)?;

            // 4. Recv Host Identity Proof (Encrypted)
            let encrypted_host_proof = recv_packet(&mut client_stream).await?;
            let identity_proof_aad = labeled_aad(b"identity-proof", salt.as_slice());
            let host_proof_bytes = cipher
                .decrypt(&encrypted_host_proof, Some(&identity_proof_aad))
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
                ed25519_public: None,
                ed25519_binding: None,
            };
            let my_proof_bytes = bincode::serialize(&my_proof)?;
            let encrypted_proof = cipher.encrypt(&my_proof_bytes, Some(&identity_proof_aad));
            send_packet(&mut client_stream, &encrypted_proof).await?;

            Ok(client_aes_key)
        });

        let host_aes_res: Result<[u8; crate::AES_KEY_SIZE]> = host_handle.await.unwrap();
        let client_aes_res: Result<[u8; crate::AES_KEY_SIZE]> = client_handle.await.unwrap();

        let host_aes = host_aes_res.unwrap();
        let client_aes = client_aes_res.unwrap();

        // Keys should match
        assert_eq!(host_aes, client_aes);

        Ok(())
    }

    /// Two current peers must actually settle on Ed25519 — and the fingerprint
    /// they exchange must be unchanged, because every contact's TOFU trust is
    /// pinned to it and this migration step deliberately does not touch it.
    #[tokio::test]
    async fn a_live_handshake_negotiates_ed25519_without_changing_the_fingerprint() {
        let host_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let client_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let expected_host_fp = fingerprint_pubkey(
            pem_encode_public(&rsa::RsaPublicKey::from(&host_priv))
                .unwrap()
                .as_bytes(),
        );
        let expected_client_fp = fingerprint_pubkey(
            pem_encode_public(&rsa::RsaPublicKey::from(&client_priv))
                .unwrap()
                .as_bytes(),
        );

        let (mut host_stream, mut client_stream) = tokio::io::duplex(1 << 16);
        let hp = host_priv.clone();
        let host = tokio::spawn(async move {
            host_handshake(&mut host_stream, &hp, uuid::Uuid::new_v4()).await
        });
        let client = client_handshake(&mut client_stream, &client_priv, uuid::Uuid::new_v4())
            .await
            .expect("client handshake");
        let host = host.await.unwrap().expect("host handshake");

        assert_eq!(host.signature_scheme, SignatureScheme::Ed25519);
        assert_eq!(client.signature_scheme, SignatureScheme::Ed25519);
        assert_eq!(client.peer_fingerprint, expected_host_fp);
        assert_eq!(host.peer_fingerprint, expected_client_fp);
    }

    /// An Ed25519 proof is only worth anything with its binding: without one,
    /// the subkey proves that somebody holds *some* Ed25519 key, which is not
    /// an identity. Accepting it would let an attacker pair a stolen
    /// `public_key_pem` — and therefore a trusted fingerprint — with a signing
    /// key of their own.
    #[test]
    fn an_ed25519_proof_without_its_binding_is_rejected() {
        let identity = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let ephemeral = [7u8; 32];
        let IdentitySignature {
            signature,
            ed25519_public,
            ed25519_binding,
        } = build_identity_signature(SignatureScheme::Ed25519, &identity, &ephemeral).unwrap();
        let pem = pem_encode_public(&rsa::RsaPublicKey::from(&identity)).unwrap();

        let good = IdentityProof {
            public_key_pem: pem.clone(),
            signature: signature.clone(),
            version: PROTOCOL_VERSION as u32,
            chat_id: uuid::Uuid::new_v4(),
            signature_scheme: SignatureScheme::Ed25519,
            ed25519_public: ed25519_public.clone(),
            ed25519_binding: ed25519_binding.clone(),
        };
        assert!(verify_identity_proof(&good, &ephemeral).is_ok());

        // Stripped binding.
        let mut stripped = good.clone();
        stripped.ed25519_binding = None;
        assert!(verify_identity_proof(&stripped, &ephemeral).is_err());

        // Stripped subkey.
        let mut no_key = good.clone();
        no_key.ed25519_public = None;
        assert!(verify_identity_proof(&no_key, &ephemeral).is_err());

        // A subkey swapped for the attacker's, keeping the victim's identity
        // and their binding — the attack the binding exists to stop.
        let attacker = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let attacker_key = derive_ed25519_subkey(&attacker).unwrap();
        let mut swapped = good.clone();
        swapped.ed25519_public = Some(attacker_key.verifying_key().to_bytes().to_vec());
        swapped.signature = sign_ed25519(&attacker_key, &{
            let mut h = Sha256::new();
            h.update(b"IDENTITY_PROOF");
            h.update(ephemeral);
            h.finalize()
        })
        .to_bytes()
        .to_vec();
        assert!(
            verify_identity_proof(&swapped, &ephemeral).is_err(),
            "a subkey the identity never vouched for must be refused"
        );

        // Signing a different ephemeral key is still caught.
        assert!(verify_identity_proof(&good, &[8u8; 32]).is_err());
    }

    #[test]
    fn test_identity_proof_aad_binding_rejects_wrong_context() {
        let proof = IdentityProof {
            public_key_pem: "test-public-key".to_string(),
            signature: vec![1, 2, 3, 4],
            version: PROTOCOL_VERSION as u32,
            chat_id: uuid::Uuid::new_v4(),
            signature_scheme: SignatureScheme::RsaPss,
            ed25519_public: None,
            ed25519_binding: None,
        };
        let proof_bytes = bincode::serialize(&proof).unwrap();
        let cipher = test_cipher();

        let correct_aad = labeled_aad(b"identity-proof", b"transcript-a");
        let wrong_aad = labeled_aad(b"identity-proof", b"transcript-b");

        let encrypted = cipher.encrypt(&proof_bytes, Some(&correct_aad));
        assert!(
            cipher.decrypt(&encrypted, Some(&wrong_aad)).is_none(),
            "identity proof must not decrypt under a different transcript binding"
        );
        assert!(
            cipher.decrypt(&encrypted, None).is_none(),
            "identity proof must not decrypt when AAD is stripped"
        );
    }

    #[test]
    fn test_transport_aad_binding_rejects_wrong_context() {
        let cipher = test_cipher();
        let msg = ProtocolMessage::Text {
            text: "hello".to_string(),
            timestamp: 42,
            seq: 1,
        };
        let plaintext = msg.to_plain_bytes();
        let correct_aad = labeled_aad(b"transport", b"session-a");
        let wrong_aad = labeled_aad(b"transport", b"session-b");

        let encrypted = cipher.encrypt(&plaintext, Some(&correct_aad));
        assert!(
            cipher.decrypt(&encrypted, Some(&wrong_aad)).is_none(),
            "transport ciphertext must not decrypt under a different transcript binding"
        );
        assert!(
            cipher.decrypt(&encrypted, None).is_none(),
            "transport ciphertext must not decrypt when AAD is stripped"
        );
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

        let schemes_msg = ProtocolMessage::SupportedSignatureSchemes {
            schemes: vec![SignatureScheme::RsaPss.to_u8()],
        };
        assert!(validate_message_sequence(&schemes_msg, &mut last_valid_seq).is_ok());
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
        //
        // NOTE: Transport-layer replay protection is now implemented in Issue #6 Phase 2
    }
}
