use anyhow::{anyhow, Result};
use rsa::{PrivateKey as RsaPrivateKey, PublicKey as RsaPublicKey};
use sha2::{Digest, Sha256};
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use x25519_dalek::{EphemeralSecret, PublicKey as X25519PublicKey};
use zeroize::Zeroizing;

use crate::core::{
    cross_sign_ed25519, derive_ed25519_subkey, derive_session_key, ed25519_public_from_bytes,
    ed25519_signature_from_bytes, fingerprint_pubkey, generate_ephemeral_keypair,
    negotiate_signature_scheme, parse_x25519_public, pem_decode_public, pem_encode_public,
    recv_packet, rsa_sign_pss, rsa_verify_pss, send_packet, sign_ed25519, verify_ed25519,
    verify_ed25519_binding, AesCipher, Ed25519Proof, IdentityProof, NonceRole, ProtocolMessage,
    SignatureScheme, PROTOCOL_VERSION,
};
use crate::network::ratelimit::RateLimiter;
use crate::types::SessionEvent;
use crate::HANDSHAKE_TIMEOUT_SECS;

/// HKDF context string for key derivation
const HKDF_INFO: &[u8] = b"p2p-messenger-v2-forward-secrecy";

/// Rate limiting: max connections per IP in the time window
const RATE_LIMIT_MAX_CONNECTIONS: usize = 5;
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(10);

/// Global rate limiter. The shared [`RateLimiter`] ages entries out and caps how many
/// addresses it tracks, so a scan across a /64 cannot grow it without bound.
static RATE_LIMITER: std::sync::LazyLock<Mutex<RateLimiter>> = std::sync::LazyLock::new(|| {
    Mutex::new(RateLimiter::with_limits(
        RATE_LIMIT_MAX_CONNECTIONS,
        RATE_LIMIT_WINDOW,
    ))
});

/// Check if an IP is rate-limited. Returns true if connection should be rejected.
fn is_rate_limited(ip: IpAddr) -> bool {
    let mut limiter = RATE_LIMITER.lock().unwrap_or_else(|e| e.into_inner());
    let limited = limiter.check(ip);
    if limited {
        tracing::warn!(
            "Rate limiting IP {}: more than {} connections in {:?}",
            ip,
            RATE_LIMIT_MAX_CONNECTIONS,
            RATE_LIMIT_WINDOW
        );
    }
    limited
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

/// Bind the host listener on `port` (all interfaces). Separate from [`run_host_session`]
/// so a bind failure (port in use) surfaces before any chat or session state exists.
pub async fn bind_host_listener(port: u16) -> Result<TcpListener> {
    TcpListener::bind(("0.0.0.0", port))
        .await
        .map_err(|e| anyhow!("Could not listen on port {}: {}", port, e))
}

/// Handshakes the host runs at once; further connections are closed on arrival. It only
/// has to outlast connections that open and then say nothing.
pub const MAX_PENDING_HOST_HANDSHAKES: usize = 4;

/// One deadline for the whole incoming handshake plus password gate, so a peer that
/// sends each packet just in time cannot hold a slot.
pub const HOST_HANDSHAKE_DEADLINE: Duration = Duration::from_secs(20);

/// A wrong connection password. Typed so the accept loop can tell the user
/// about it without reporting every port scan.
#[derive(Debug)]
struct PasswordRefused;

impl std::fmt::Display for PasswordRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Incorrect connection password")
    }
}

impl std::error::Error for PasswordRefused {}

/// Everything the host does before the peer is shown to the user: the
/// handshake, then the optional password gate inside the tunnel.
async fn host_preamble<S>(
    stream: &mut S,
    privkey: &RsaPrivateKey,
    chat_id: uuid::Uuid,
    connection_password: Option<&str>,
) -> Result<EstablishedTunnel>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let tunnel = host_handshake(stream, privkey, chat_id).await?;
    host_password_gate(
        stream,
        &tunnel.cipher,
        &tunnel.transport_aad,
        tunnel.peer_version,
        connection_password,
    )
    .await?;
    Ok(tunnel)
}

/// The prompt that surfaces an incoming peer for verification.
fn new_connection_prompt(peer_label: String, tunnel: &EstablishedTunnel) -> SessionEvent {
    SessionEvent::NewConnection {
        peer_addr: peer_label,
        fingerprint: tunnel.peer_fingerprint.clone(),
        sas: crate::core::derive_sas(&tunnel.transport_aad),
        chat_id: tunnel.peer_chat_id,
    }
}

/// Run a host session on a listener from [`bind_host_listener`]: accept until one peer
/// completes the handshake (and password gate), then verify and run the message loop.
///
/// Handshakes run concurrently (at most [`MAX_PENDING_HOST_HANDSHAKES`], each under
/// [`HOST_HANDSHAKE_DEADLINE`]) with the per-IP rate limit applied before a slot is
/// spent, so one silent connection cannot close the port (audit NET-02). The listener
/// is dropped as soon as a peer wins so auto-rehost can bind again.
#[allow(clippy::too_many_arguments)]
pub async fn run_host_session(
    listener: TcpListener,
    privkey: RsaPrivateKey,
    to_app_tx: mpsc::UnboundedSender<SessionEvent>,
    from_app_rx: mpsc::UnboundedReceiver<ProtocolMessage>,
    file_rx: mpsc::Receiver<ProtocolMessage>,
    confirm_rx: mpsc::UnboundedReceiver<bool>,
    chat_id: uuid::Uuid,
    connection_password: Option<String>,
) -> Result<()> {
    // Report the port we actually bound (the caller may have asked for 0).
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    tracing::info!("Host listening on port {}", port);

    emit(&to_app_tx, SessionEvent::Listening { port })?;

    type Attempt = (TcpStream, std::net::SocketAddr, Result<EstablishedTunnel>);
    let mut pending: tokio::task::JoinSet<Attempt> = tokio::task::JoinSet::new();
    let (mut stream, peer_addr, tunnel) = loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, addr) = match accepted {
                    Ok(pair) => {
                        pair.0.set_nodelay(true).ok(); // Every frame is small and latency-bound; see NET-04 in `framing.rs`.
                        pair
                    }
                    Err(e) => {
                        // A transient EMFILE must not end hosting; back off
                        // briefly so it cannot spin either.
                        tracing::warn!("accept() failed: {}", e);
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                if is_rate_limited(addr.ip()) {
                    tracing::warn!("Rejecting connection from {} due to rate limiting", addr);
                    continue;
                }
                if pending.len() >= MAX_PENDING_HOST_HANDSHAKES {
                    tracing::warn!("Too many handshakes in progress; closing {}", addr);
                    continue;
                }
                let privkey = privkey.clone();
                let password = connection_password.clone();
                pending.spawn(async move {
                    let mut stream = stream;
                    let result = tokio::time::timeout(
                        HOST_HANDSHAKE_DEADLINE,
                        host_preamble(&mut stream, &privkey, chat_id, password.as_deref()),
                    )
                    .await
                    .unwrap_or_else(|_| {
                        Err(anyhow!(
                            "Handshake did not finish within {}s",
                            HOST_HANDSHAKE_DEADLINE.as_secs()
                        ))
                    });
                    (stream, addr, result)
                });
            }
            Some(done) = pending.join_next(), if !pending.is_empty() => {
                match done {
                    Ok((stream, addr, Ok(tunnel))) => break (stream, addr, tunnel),
                    Ok((_, addr, Err(e))) => {
                        if e.downcast_ref::<PasswordRefused>().is_some() {
                            let _ = to_app_tx.send(SessionEvent::Warning(format!(
                                "Refused a connection from {addr}: wrong connection password"
                            )));
                        }
                        tracing::info!("Incoming handshake from {} failed: {}", addr, e);
                    }
                    Err(e) => tracing::warn!("Handshake task failed: {}", e),
                }
            }
        }
    };

    // Release the port as soon as we have our peer so auto-rehost can bind; the other
    // half-finished handshakes are abandoned with it.
    drop(listener);
    pending.abort_all();
    tracing::info!("Client connected from {}", peer_addr);

    let prompt = new_connection_prompt(peer_addr.to_string(), &tunnel);
    verify_then_run(
        &mut stream,
        tunnel,
        prompt,
        chat_id,
        to_app_tx,
        from_app_rx,
        file_rx,
        confirm_rx,
    )
    .await
}

/// Try a TCP connection to each candidate in order, each bounded by
/// [`crate::CONNECT_ATTEMPT_TIMEOUT_SECS`] so one dead address cannot stall the fallback.
/// Emits a `Warning` between failed attempts. Returns the stream plus the winning target.
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
            Ok(Ok(stream)) => {
                stream.set_nodelay(true).ok(); // Every frame is small and latency-bound; see NET-04 in `framing.rs`.
                return Ok((stream, host.clone(), *port));
            }
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

/// Run the client session over the first `(host, port)` candidate that accepts a TCP
/// connection; once a stream is established the session lives or dies there.
#[allow(clippy::too_many_arguments)]
pub async fn run_client_session_multi(
    targets: &[(String, u16)],
    privkey: RsaPrivateKey,
    to_app_tx: mpsc::UnboundedSender<SessionEvent>,
    from_app_rx: mpsc::UnboundedReceiver<ProtocolMessage>,
    file_rx: mpsc::Receiver<ProtocolMessage>,
    confirm_rx: mpsc::UnboundedReceiver<bool>,
    chat_id: uuid::Uuid,
    connection_password: Option<String>,
) -> Result<()> {
    run_client_session_direct_or_relay(
        targets,
        None,
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

/// As [`run_client_session_multi`], with a relay route to fall back to when
/// **no** direct candidate accepts a connection — a contact's saved address
/// goes stale when the peer changes networks, and failing direct-only never
/// tried the route that would have worked (audit NET-12). Only a failure to
/// *open* a direct connection falls through: a session that connected and
/// then ended is not redialled through the relay. `relay` is
/// `(server, token)`.
#[allow(clippy::too_many_arguments)]
pub async fn run_client_session_direct_or_relay(
    targets: &[(String, u16)],
    relay: Option<(String, String)>,
    privkey: RsaPrivateKey,
    to_app_tx: mpsc::UnboundedSender<SessionEvent>,
    from_app_rx: mpsc::UnboundedReceiver<ProtocolMessage>,
    file_rx: mpsc::Receiver<ProtocolMessage>,
    confirm_rx: mpsc::UnboundedReceiver<bool>,
    chat_id: uuid::Uuid,
    connection_password: Option<String>,
) -> Result<()> {
    let (mut stream, host, port) = match connect_first_reachable(targets, &to_app_tx).await {
        Ok(connected) => connected,
        Err(direct) => {
            let Some((server, token)) = relay else {
                return Err(direct);
            };
            tracing::info!(error = %direct, relay = %server, "no direct route; falling back to the relay");
            let _ = to_app_tx.send(SessionEvent::Warning(format!(
                "Could not reach them directly ({direct}); trying the relay {server}…"
            )));
            return crate::network::relay::run_client_session_via_relay(
                &server,
                &token,
                privkey,
                to_app_tx,
                from_app_rx,
                file_rx,
                confirm_rx,
                chat_id,
                connection_password,
            )
            .await;
        }
    };
    tracing::info!("Connected to {}:{}", host, port);

    run_client_session_over_stream(
        &mut stream,
        format!("{}:{}", host, port),
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

/// A completed handshake: the peer's verified fingerprint and chat id plus the AEAD
/// tunnel. Callers apply their own trust policy and message loop on top.
pub struct EstablishedTunnel {
    pub peer_fingerprint: String,
    pub peer_chat_id: uuid::Uuid,
    pub cipher: AesCipher,
    pub transport_aad: Vec<u8>,
    /// Signature scheme negotiated for identity proofs; varies by peer version, so
    /// diagnostics report it.
    pub signature_scheme: SignatureScheme,
    /// Whether the client committed to its ephemeral key first (both peers v4+). Without
    /// it a MITM can grind keys until both SAS codes match, so the SAS is a weak check.
    pub sas_committed: bool,
    /// The protocol version the peer announced. The connection-password gate
    /// reads it: from [`PASSWORD_PROOF_VERSION`] the password is proved, never
    /// sent.
    pub peer_version: u32,
}

/// First protocol version whose client proves the connection password
/// instead of sending it (audit CRYPTO-05).
pub const PASSWORD_PROOF_VERSION: u32 = 5;

/// Domain separator for the connection-password proof.
const PASSWORD_PROOF_LABEL: &[u8] = b"p2pem-v5-connection-password-proof";

/// What a v5+ client sends in place of the connection password.
///
/// The gate runs before the user has compared the safety code, so the host it
/// talks to may not be the host at all. The password used to cross in clear
/// (inside the tunnel, but to whoever is on the other end of it): anyone
/// impersonating the host collected it from every client and used it on the
/// real one. This value is HKDF-SHA256 with the label and the handshake
/// transcript as salt and the password as input keying material — so an HMAC
/// keyed by the transcript — and only someone who knows the password *and*
/// shares this transcript can produce it. A man in the middle runs two
/// handshakes with two transcripts and cannot relay it; a fake host can only
/// try guesses offline. Frozen by a known-answer test.
pub fn password_proof(password: &str, transcript: &[u8]) -> [u8; 32] {
    let mut salt = Vec::with_capacity(PASSWORD_PROOF_LABEL.len() + transcript.len());
    salt.extend_from_slice(PASSWORD_PROOF_LABEL);
    salt.extend_from_slice(transcript);
    let hk = hkdf::Hkdf::<sha2::Sha256>::new(Some(&salt), password.as_bytes());
    let mut out = [0u8; 32];
    hk.expand(b"client", &mut out)
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    out
}

/// First protocol version in which the client commits to its ephemeral key.
pub const SAS_COMMITMENT_VERSION: u32 = 4;

/// Domain separator for the ephemeral-key commitment.
const COMMITMENT_LABEL: &[u8] = b"p2pem-v4-ephemeral-commitment";

/// The client's commitment to its ephemeral key, sent before it sees the host's, so a
/// MITM cannot grind keys until both victims' safety codes match (audit CRYPTO-02).
pub fn ephemeral_commitment(ephemeral: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(COMMITMENT_LABEL);
    h.update(ephemeral);
    h.finalize().into()
}

/// Check a revealed ephemeral key against the commitment received earlier.
fn check_commitment(commitment: &[u8], ephemeral: &[u8]) -> Result<()> {
    let expected = ephemeral_commitment(ephemeral);
    if commitment.len() != expected.len() || !bool::from(expected.ct_eq(commitment)) {
        return Err(anyhow!(
            "Client's ephemeral key does not match its commitment"
        ));
    }
    Ok(())
}

/// The warning shown before verifying a peer whose handshake had no
/// commitment (a pre-v4 peer, or someone stripping the version on the path).
pub const LEGACY_SAS_WARNING: &str = "This peer runs an older version, so its safety code is a \
weak check. Compare the full fingerprint (advanced) instead, or ask them to update.";

/// Sign our ephemeral key for the identity proof under the negotiated scheme: an RSA-PSS
/// signature, or a serialized [`Ed25519Proof`] (signature, subkey, and the subkey's
/// binding to this RSA identity). Shared by every handshake variant.
fn build_identity_signature(
    scheme: SignatureScheme,
    privkey: &RsaPrivateKey,
    ephemeral_bytes: &[u8],
) -> Result<Vec<u8>> {
    let mut hasher = Sha256::new();
    hasher.update(b"IDENTITY_PROOF");
    hasher.update(ephemeral_bytes);
    let digest = hasher.finalize();
    match scheme {
        SignatureScheme::RsaPss => {
            // The pre-hashed transcript digest is the PSS *message* (hashed
            // again inside), exactly as before; changing that breaks every peer.
            rsa_sign_pss(privkey, &digest)
        }
        SignatureScheme::Ed25519 => {
            let signing_key = derive_ed25519_subkey(privkey)?;
            let verifying = signing_key.verifying_key();
            // Packed into the signature field so `IdentityProof` keeps the
            // exact shape older peers expect.
            Ok(bincode::serialize(&Ed25519Proof {
                signature: sign_ed25519(&signing_key, &digest).to_bytes().to_vec(),
                public: verifying.to_bytes().to_vec(),
                binding: cross_sign_ed25519(privkey, &verifying)?,
            })?)
        }
    }
}

/// Decode a peer's `IdentityProof`, refusing trailing bytes. It is decoded before any
/// trust decision, and `proof || junk` must not decode to the same proof
/// (see [`crate::util::decode_exact`]).
fn decode_identity_proof(bytes: &[u8]) -> Result<IdentityProof> {
    crate::util::decode_exact(bytes)
}

/// Verify a peer's identity proof against the ephemeral key they sent. The identity is
/// always the RSA key in `public_key_pem` (what the fingerprint is derived from); an
/// Ed25519 proof is accepted only when its subkey is signed by that RSA identity.
fn verify_identity_proof(proof: &IdentityProof, peer_ephemeral_bytes: &[u8]) -> Result<()> {
    let peer_pubkey = pem_decode_public(&proof.public_key_pem)?;
    let mut hasher = Sha256::new();
    hasher.update(b"IDENTITY_PROOF");
    hasher.update(peer_ephemeral_bytes);
    let digest = hasher.finalize();

    match proof.signature_scheme {
        SignatureScheme::RsaPss => rsa_verify_pss(&peer_pubkey, &digest, &proof.signature)
            .map_err(|e| anyhow!("Identity signature verification failed: {}", e)),
        SignatureScheme::Ed25519 => {
            // The subkey blob is carried inside the signature field, so it is
            // just as peer-controlled as the proof around it.
            let blob: Ed25519Proof = crate::util::decode_exact(&proof.signature)
                .map_err(|e| anyhow!("Ed25519 proof is malformed or missing its subkey: {}", e))?;
            let ed_public = ed25519_public_from_bytes(&blob.public)?;
            // Bind first: an unbound subkey proves only that somebody holds
            // *some* Ed25519 key, which is not an identity.
            verify_ed25519_binding(&peer_pubkey, &ed_public, &blob.binding)?;
            let signature = ed25519_signature_from_bytes(&blob.signature)?;
            verify_ed25519(&ed_public, &digest, &signature)
                .map_err(|e| anyhow!("Identity signature verification failed: {}", e))
        }
    }
}

/// The signature schemes we offer, best first; RSA-PSS is kept for older peers.
const OFFERED_SCHEMES: [SignatureScheme; 2] = [SignatureScheme::Ed25519, SignatureScheme::RsaPss];

/// Our protocol version as it goes on the wire (plaintext, big-endian u32).
fn our_version_bytes() -> [u8; 4] {
    (PROTOCOL_VERSION as u32).to_be_bytes()
}

/// Whether this handshake uses the ephemeral-key commitment: both sides must
/// speak v4+ (each knows both versions after the version exchange).
fn uses_commitment(our_version: &[u8; 4], peer_version: u32) -> bool {
    u32::from_be_bytes(*our_version).min(peer_version) >= SAS_COMMITMENT_VERSION
}

/// The other side's name in error messages, from our role.
fn peer_role_name(us: NonceRole) -> &'static str {
    match us {
        NonceRole::Host => "Client",
        NonceRole::Client => "Host",
    }
}

/// Check the peer's plaintext version packet, refusing anything before v3.
fn check_peer_version(bytes: &[u8], us: NonceRole) -> Result<u32> {
    let raw: [u8; 4] = bytes
        .try_into()
        .map_err(|_| anyhow!("Invalid version packet length"))?;
    let version = u32::from_be_bytes(raw);
    if version < 3 {
        return Err(anyhow!(
            "{} version {} too old (need v3+)",
            peer_role_name(us),
            version
        ));
    }
    tracing::debug!("{} protocol version: {}", peer_role_name(us), version);
    Ok(version)
}

/// Receive the peer's X25519 ephemeral key, returning the raw bytes (they go
/// into the transcript and the identity proof) alongside the parsed key.
async fn recv_peer_ephemeral<S>(stream: &mut S) -> Result<(Vec<u8>, X25519PublicKey)>
where
    S: AsyncRead + Unpin,
{
    let bytes = recv_packet_with_timeout(stream).await?;
    if bytes.len() != crate::AES_KEY_SIZE {
        return Err(anyhow!("Invalid ephemeral key length"));
    }
    let public = parse_x25519_public(&bytes)?;
    Ok((bytes, public))
}

/// Exchange supported signature schemes and settle on one. The host speaks
/// first; the client answers.
async fn negotiate_schemes<S>(stream: &mut S, us: NonceRole) -> Result<SignatureScheme>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let ours: Vec<u8> = OFFERED_SCHEMES.iter().map(|s| s.to_u8()).collect();
    let offer = ProtocolMessage::SupportedSignatureSchemes {
        schemes: ours.clone(),
    }
    .to_plain_bytes();

    if us == NonceRole::Host {
        send_packet(stream, &offer).await?;
    }
    let theirs = match ProtocolMessage::from_plain_bytes(&recv_packet_with_timeout(stream).await?) {
        Some(ProtocolMessage::SupportedSignatureSchemes { schemes }) => schemes,
        _ => {
            return Err(anyhow!(
                "{} did not send valid signature schemes",
                peer_role_name(us)
            ))
        }
    };
    if us == NonceRole::Client {
        send_packet(stream, &offer).await?;
    }

    let selected = negotiate_signature_scheme(&ours, &theirs)
        .ok_or_else(|| anyhow!("No common signature scheme found"))?;
    tracing::debug!("Negotiated signature scheme: {}", selected.name());
    Ok(selected)
}

/// The keys a handshake produces before either identity is known.
struct TunnelKeys {
    cipher: AesCipher,
    identity_proof_aad: Vec<u8>,
    transport_aad: Vec<u8>,
}

/// Derive the session key and AADs from the ECDH result and the transcript. The
/// transcript is `HostVersion || ClientVersion || HostEphemeral || ClientEphemeral`
/// whoever computes it, which is why the arguments name roles, not "ours"/"theirs".
fn derive_tunnel_keys(
    our_secret: EphemeralSecret,
    their_public: &X25519PublicKey,
    host_version: &[u8],
    client_version: &[u8],
    host_ephemeral: &[u8],
    client_ephemeral: &[u8],
    us: NonceRole,
) -> Result<TunnelKeys> {
    let mut transcript = Vec::with_capacity(
        host_version.len() + client_version.len() + host_ephemeral.len() + client_ephemeral.len(),
    );
    transcript.extend_from_slice(host_version);
    transcript.extend_from_slice(client_version);
    transcript.extend_from_slice(host_ephemeral);
    transcript.extend_from_slice(client_ephemeral);
    let salt = Sha256::digest(&transcript);

    let aes_key = Zeroizing::new(derive_session_key(
        our_secret,
        their_public,
        Some(&salt),
        HKDF_INFO,
    )?);
    let keys = TunnelKeys {
        cipher: AesCipher::new_with_role(&aes_key[..], us)?,
        identity_proof_aad: labeled_aad(b"identity-proof", &salt),
        transport_aad: labeled_aad(b"transport", &salt),
    };
    tracing::info!("Encrypted tunnel established (Forward Secrecy enabled)");
    Ok(keys)
}

/// Sign our ephemeral key with our identity and send the proof, encrypted.
async fn send_identity_proof<S>(
    stream: &mut S,
    keys: &TunnelKeys,
    privkey: &RsaPrivateKey,
    our_ephemeral: &[u8],
    chat_id: uuid::Uuid,
    scheme: SignatureScheme,
) -> Result<()>
where
    S: AsyncWrite + Unpin,
{
    let proof = IdentityProof {
        public_key_pem: pem_encode_public(&RsaPublicKey::from(privkey))?,
        signature: build_identity_signature(scheme, privkey, our_ephemeral)?,
        version: PROTOCOL_VERSION as u32,
        chat_id,
        signature_scheme: scheme,
    };
    let encrypted = keys
        .cipher
        .encrypt(&bincode::serialize(&proof)?, Some(&keys.identity_proof_aad));
    send_packet(stream, &encrypted).await?;
    Ok(())
}

/// Receive, decrypt and verify the peer's identity proof against the
/// ephemeral key they sent. Returns the proof only once it has verified.
async fn recv_identity_proof<S>(
    stream: &mut S,
    keys: &TunnelKeys,
    peer_ephemeral: &[u8],
    scheme: SignatureScheme,
    us: NonceRole,
) -> Result<IdentityProof>
where
    S: AsyncRead + Unpin,
{
    let peer = peer_role_name(us);
    let encrypted = recv_packet_with_timeout(stream).await?;
    let bytes = keys
        .cipher
        .decrypt_from_peer(&encrypted, Some(&keys.identity_proof_aad))
        .ok_or_else(|| anyhow!("Failed to decrypt {} identity proof", peer.to_lowercase()))?;
    let proof = decode_identity_proof(&bytes)?;
    if proof.signature_scheme != scheme {
        return Err(anyhow!(
            "{} used unexpected signature scheme: expected {}, got {}",
            peer,
            scheme,
            proof.signature_scheme
        ));
    }
    // An Ed25519 proof is only accepted when its subkey is bound to the RSA identity.
    verify_identity_proof(&proof, peer_ephemeral)
        .map_err(|e| anyhow!("{} identity verification failed: {}", peer, e))?;
    Ok(proof)
}

impl TunnelKeys {
    fn established(
        self,
        peer: &IdentityProof,
        scheme: SignatureScheme,
        sas_committed: bool,
        peer_version: u32,
    ) -> Result<EstablishedTunnel> {
        // Parsed by the same strict decoder `verify_identity_proof` used, so
        // the fingerprint names exactly the key the signature was checked with.
        let peer_fingerprint = fingerprint_pubkey(peer.public_key_pem.as_bytes())?;
        tracing::debug!("Verified peer identity: {}...", &peer_fingerprint[..8]);
        Ok(EstablishedTunnel {
            peer_fingerprint,
            peer_chat_id: peer.chat_id,
            cipher: self.cipher,
            transport_aad: self.transport_aad,
            signature_scheme: scheme,
            sas_committed,
            peer_version,
        })
    }
}

/// Host side of the handshake: version exchange, X25519 ephemeral exchange, session-key
/// derivation, and the encrypted, transcript-bound identity-proof exchange. The caller
/// applies trust policy and runs the message loop. Wire order (the client mirrors it):
/// version, (v4: client commitment), ephemeral, schemes, proof.
pub async fn host_handshake<S>(
    stream: &mut S,
    privkey: &RsaPrivateKey,
    chat_id: uuid::Uuid,
) -> Result<EstablishedTunnel>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    host_handshake_as(stream, privkey, chat_id, our_version_bytes()).await
}

/// [`host_handshake`] announcing an arbitrary version — the real one in
/// production, an older one in compatibility tests.
async fn host_handshake_as<S>(
    stream: &mut S,
    privkey: &RsaPrivateKey,
    chat_id: uuid::Uuid,
    host_version: [u8; 4],
) -> Result<EstablishedTunnel>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    const US: NonceRole = NonceRole::Host;

    send_packet(stream, &host_version).await?;
    let client_version = recv_packet_with_timeout(stream).await?;
    let peer_version = check_peer_version(&client_version, US)?;
    let committed = uses_commitment(&host_version, peer_version);

    // v4: the client's commitment arrives before we reveal anything, so it
    // cannot choose its key with ours in hand.
    let commitment = if committed {
        Some(recv_packet_with_timeout(stream).await?)
    } else {
        None
    };

    let (host_secret, host_public) = generate_ephemeral_keypair();
    send_packet(stream, host_public.as_bytes()).await?;
    let (client_ephemeral, client_public) = recv_peer_ephemeral(stream).await?;
    if let Some(commitment) = commitment {
        check_commitment(&commitment, &client_ephemeral)?;
    }

    let scheme = negotiate_schemes(stream, US).await?;

    let keys = derive_tunnel_keys(
        host_secret,
        &client_public,
        &host_version,
        &client_version,
        host_public.as_bytes(),
        &client_ephemeral,
        US,
    )?;

    send_identity_proof(
        stream,
        &keys,
        privkey,
        host_public.as_bytes(),
        chat_id,
        scheme,
    )
    .await?;
    let client_proof = recv_identity_proof(stream, &keys, &client_ephemeral, scheme, US).await?;
    keys.established(&client_proof, scheme, committed, peer_version)
}

/// Client side of the handshake; mirror of [`host_handshake`]. The host proves its
/// identity first, so the client verifies before revealing its own.
pub async fn client_handshake<S>(
    stream: &mut S,
    privkey: &RsaPrivateKey,
    chat_id: uuid::Uuid,
) -> Result<EstablishedTunnel>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    client_handshake_as(stream, privkey, chat_id, our_version_bytes()).await
}

/// [`client_handshake`] announcing an arbitrary version (see
/// [`host_handshake_as`]).
async fn client_handshake_as<S>(
    stream: &mut S,
    privkey: &RsaPrivateKey,
    chat_id: uuid::Uuid,
    client_version: [u8; 4],
) -> Result<EstablishedTunnel>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    const US: NonceRole = NonceRole::Client;

    let host_version = recv_packet_with_timeout(stream).await?;
    let peer_version = check_peer_version(&host_version, US)?;
    send_packet(stream, &client_version).await?;
    let committed = uses_commitment(&client_version, peer_version);

    let (client_secret, client_public) = generate_ephemeral_keypair();
    if committed {
        // Bind ourselves to this key before the host reveals its own.
        send_packet(stream, &ephemeral_commitment(client_public.as_bytes())).await?;
    }
    let (host_ephemeral, host_public) = recv_peer_ephemeral(stream).await?;
    send_packet(stream, client_public.as_bytes()).await?;

    let scheme = negotiate_schemes(stream, US).await?;

    let keys = derive_tunnel_keys(
        client_secret,
        &host_public,
        &host_version,
        &client_version,
        &host_ephemeral,
        client_public.as_bytes(),
        US,
    )?;

    let host_proof = recv_identity_proof(stream, &keys, &host_ephemeral, scheme, US).await?;
    send_identity_proof(
        stream,
        &keys,
        privkey,
        client_public.as_bytes(),
        chat_id,
        scheme,
    )
    .await?;
    keys.established(&host_proof, scheme, committed, peer_version)
}

/// Constant-time password comparison, so a wrong connection password cannot be
/// recovered via response timing. Both inputs are first reduced to 32 bytes by Argon2id
/// (minimum cost, fresh random salt per comparison), so the length never reaches a
/// branch; nothing relies on Argon2's cost, only on a salted fixed-length function
/// (not SHA-256: the wrong primitive for a password, and code scanning flags it).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use argon2::{Algorithm, Argon2, Params, Version};
    let salt: [u8; 16] = rand::random();
    let Ok(params) = Params::new(Params::MIN_M_COST, Params::MIN_T_COST, 1, Some(32)) else {
        return false;
    };
    let kdf = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let (mut ha, mut hb) = (Zeroizing::new([0u8; 32]), Zeroizing::new([0u8; 32]));
    if kdf.hash_password_into(a, &salt, &mut ha[..]).is_err()
        || kdf.hash_password_into(b, &salt, &mut hb[..]).is_err()
    {
        return false;
    }
    bool::from(ha.ct_eq(&hb[..]))
}

/// Host side of the optional connection-password gate, run inside the tunnel right after
/// the handshake and before TOFU. The host announces whether a password is required; the
/// client replies with it. A wrong password returns [`PasswordRefused`].
async fn host_password_gate<S>(
    stream: &mut S,
    cipher: &AesCipher,
    aad: &[u8],
    peer_version: u32,
    expected: Option<&str>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let flag = [u8::from(expected.is_some())];
    send_packet(stream, &cipher.encrypt(&flag, Some(aad))).await?;
    let resp_ct = recv_packet_with_timeout(stream).await?;
    let resp = cipher
        .decrypt_from_peer(&resp_ct, Some(aad))
        .ok_or_else(|| anyhow!("Failed to decrypt password response"))?;
    if let Some(pw) = expected {
        let admitted = if peer_version >= PASSWORD_PROOF_VERSION {
            constant_time_eq(&resp, &password_proof(pw, aad))
        } else {
            // A client from before the proof sends the password itself.
            // Accepting it exposes nobody but that client, which already
            // sends it to whoever answers.
            constant_time_eq(&resp, pw.as_bytes())
        };
        if !admitted {
            return Err(PasswordRefused.into());
        }
    }
    Ok(())
}

/// Client side of the connection-password gate (mirror of [`host_password_gate`]).
async fn client_password_gate<S>(
    stream: &mut S,
    cipher: &AesCipher,
    aad: &[u8],
    peer_version: u32,
    password: Option<&str>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let flag_ct = recv_packet_with_timeout(stream).await?;
    let flag = cipher
        .decrypt_from_peer(&flag_ct, Some(aad))
        .ok_or_else(|| anyhow!("Failed to decrypt password challenge"))?;
    let required = flag.first().copied() == Some(1);
    // Always reply (even empty) so the host's receive completes and both sides
    // converge on the same outcome.
    let proves = peer_version >= PASSWORD_PROOF_VERSION;
    let payload: Vec<u8> = match password {
        Some(p) if required && proves => password_proof(p, aad).to_vec(),
        // Nothing to say, or a host from before the proof, which wants the
        // password itself — before the user has checked who it is. Not sent:
        // anyone posing as this host would collect it (CRYPTO-05).
        _ => Vec::new(),
    };
    send_packet(stream, &cipher.encrypt(&payload, Some(aad))).await?;
    if required && password.is_none() {
        return Err(anyhow!("This host requires a connection password"));
    }
    if required && !proves {
        return Err(anyhow!(
            "This host runs an older version that asks for the connection password in a way \
             that would hand it to anyone posing as the host, so it was not sent. Update the \
             app on the host."
        ));
    }
    Ok(())
}

/// How long a handshaken peer waits for the user to compare the SAS.
const FINGERPRINT_CONFIRM_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Send an event the session cannot continue without the app hearing.
fn emit(to_app_tx: &mpsc::UnboundedSender<SessionEvent>, event: SessionEvent) -> Result<()> {
    to_app_tx
        .send(event)
        .map_err(|e| anyhow!("Send error: {}", e))
}

/// Report a failure to the app before returning it, so the UI says *why* the
/// connection went away rather than only that it did.
fn reported<T>(to_app_tx: &mpsc::UnboundedSender<SessionEvent>, result: Result<T>) -> Result<T> {
    if let Err(e) = &result {
        let _ = to_app_tx.send(SessionEvent::Error(e.to_string()));
    }
    result
}

/// Wait for the user's verdict on the peer's identity.
async fn await_user_confirmation(
    confirm_rx: &mut mpsc::UnboundedReceiver<bool>,
    to_app_tx: &mpsc::UnboundedSender<SessionEvent>,
    chat_id: uuid::Uuid,
) -> Result<()> {
    let (shown, error) =
        match tokio::time::timeout(FINGERPRINT_CONFIRM_TIMEOUT, confirm_rx.recv()).await {
            Ok(Some(true)) => {
                tracing::info!("User accepted fingerprint for chat {}", chat_id);
                return Ok(());
            }
            Ok(Some(false)) => ("Fingerprint rejected", "Fingerprint rejected by user"),
            Ok(None) => (
                "Confirmation channel closed",
                "Fingerprint verification failed: channel closed",
            ),
            Err(_) => (
                "Fingerprint verification timed out",
                "Fingerprint verification timed out",
            ),
        };
    tracing::warn!("{} (chat {})", error, chat_id);
    let _ = to_app_tx.send(SessionEvent::Error(shown.to_string()));
    Err(anyhow!(error))
}

/// Common to both roles after the handshake and password gate: warn if the peer's
/// safety code is weak, surface the peer for verification, wait for the user, then
/// run the message loop.
#[allow(clippy::too_many_arguments)]
async fn verify_then_run<S>(
    stream: &mut S,
    tunnel: EstablishedTunnel,
    prompt: SessionEvent,
    chat_id: uuid::Uuid,
    to_app_tx: mpsc::UnboundedSender<SessionEvent>,
    from_app_rx: mpsc::UnboundedReceiver<ProtocolMessage>,
    file_rx: mpsc::Receiver<ProtocolMessage>,
    mut confirm_rx: mpsc::UnboundedReceiver<bool>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    if !tunnel.sas_committed {
        let _ = to_app_tx.send(SessionEvent::Warning(LEGACY_SAS_WARNING.to_string()));
    }
    emit(&to_app_tx, prompt)?;
    await_user_confirmation(&mut confirm_rx, &to_app_tx, chat_id).await?;
    emit(&to_app_tx, SessionEvent::Ready)?;
    run_message_loop(
        stream,
        tunnel.cipher,
        tunnel.transport_aad,
        to_app_tx,
        from_app_rx,
        file_rx,
    )
    .await
}

/// Host side of a P2P session over an already-connected stream (direct TCP, punched
/// socket or relay bridge): handshake, password gate, TOFU verification, message loop.
#[allow(clippy::too_many_arguments)]
pub async fn run_host_session_over_stream<S>(
    stream: &mut S,
    peer_label: String,
    privkey: RsaPrivateKey,
    to_app_tx: mpsc::UnboundedSender<SessionEvent>,
    from_app_rx: mpsc::UnboundedReceiver<ProtocolMessage>,
    file_rx: mpsc::Receiver<ProtocolMessage>,
    confirm_rx: mpsc::UnboundedReceiver<bool>,
    chat_id: uuid::Uuid,
    connection_password: Option<String>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let tunnel = host_handshake(stream, &privkey, chat_id).await?;
    // Optional password gate, inside the tunnel and before verification.
    reported(
        &to_app_tx,
        host_password_gate(
            stream,
            &tunnel.cipher,
            &tunnel.transport_aad,
            tunnel.peer_version,
            connection_password.as_deref(),
        )
        .await,
    )?;
    let prompt = new_connection_prompt(peer_label, &tunnel);
    verify_then_run(
        stream,
        tunnel,
        prompt,
        chat_id,
        to_app_tx,
        from_app_rx,
        file_rx,
        confirm_rx,
    )
    .await
}

/// Client side of a P2P session over an already-connected stream. Mirror of
/// [`run_host_session_over_stream`].
#[allow(clippy::too_many_arguments)]
pub async fn run_client_session_over_stream<S>(
    stream: &mut S,
    peer_label: String,
    privkey: RsaPrivateKey,
    to_app_tx: mpsc::UnboundedSender<SessionEvent>,
    from_app_rx: mpsc::UnboundedReceiver<ProtocolMessage>,
    file_rx: mpsc::Receiver<ProtocolMessage>,
    confirm_rx: mpsc::UnboundedReceiver<bool>,
    chat_id: uuid::Uuid,
    connection_password: Option<String>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    emit(
        &to_app_tx,
        SessionEvent::Connected {
            peer: peer_label.clone(),
        },
    )?;

    let tunnel = client_handshake(stream, &privkey, chat_id).await?;

    // Answer the host's optional connection-password gate before proceeding.
    reported(
        &to_app_tx,
        client_password_gate(
            stream,
            &tunnel.cipher,
            &tunnel.transport_aad,
            tunnel.peer_version,
            connection_password.as_deref(),
        )
        .await,
    )?;

    let prompt = SessionEvent::ShowFingerprintVerification {
        fingerprint: tunnel.peer_fingerprint.clone(),
        peer_name: peer_label,
        sas: crate::core::derive_sas(&tunnel.transport_aad),
        chat_id,
    };
    verify_then_run(
        stream,
        tunnel,
        prompt,
        chat_id,
        to_app_tx,
        from_app_rx,
        file_rx,
        confirm_rx,
    )
    .await
}

/// Transport-layer replay protection, enforced before a frame reaches the app: a
/// sequenced frame is accepted only if its seq is strictly greater than the last
/// accepted one (rejecting replays, duplicates and reordering), which it then becomes.
fn validate_message_sequence(
    msg: &ProtocolMessage,
    last_valid_seq: &mut u64,
) -> Result<(), String> {
    // Handshake messages carry no sequence number.
    let Some(seq) = msg.seq() else {
        return Ok(());
    };

    if seq <= *last_valid_seq {
        return Err(format!(
            "Replay/Out-of-order detected: received seq={}, expected seq > {}",
            seq, last_valid_seq
        ));
    }

    *last_valid_seq = seq;
    Ok(())
}

/// Send a `Rekey` frame and rotate to the next session key. Only the host initiates;
/// if both sides could, each might apply its new key before the other's old-key
/// `Rekey` arrives and the next frame would be undecryptable.
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
    // Keep the old key for receiving: the peer sends under it until it processes this
    // Rekey. Dropped once a frame decrypts under the new key (see `Transport::receive`).
    *prev_cipher = Some(cipher.clone());
    *cipher = AesCipher::new_with_role(&next_key, role)?;
    *messages_since_rekey = 0;
    *last_rekey_time = Instant::now();
    tracing::info!("Session key rotated (initiated rekey)");
    Ok(())
}

/// Rotate the session key after this many application frames…
const REKEY_MESSAGE_COUNT: u64 = 100;
/// …or after this long, whichever comes first.
const REKEY_TIME_INTERVAL: Duration = Duration::from_secs(300);

/// How long the peer may stay silent before the session is considered dead.
const RECV_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Keep-alive pings run well inside the peer's idle window, so a quiet but healthy
/// session is never torn down.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(120);

/// Inbound frames that may complete and wait while a write is blocked before the loop
/// stops reading (see [`write_while_reading`]); bounds a peer that never reads.
const MAX_FRAMES_HELD_DURING_WRITE: usize = 16;

/// What a received frame means for the loop.
enum Received {
    Continue,
    Stop,
}

/// Per-session state every frame passes through: the cipher (and the key retained across
/// a rotation), both sequence counters and the rekey schedule.
struct Transport<W> {
    writer: W,
    cipher: AesCipher,
    /// The key in force before the latest rotation, so frames the peer sent under it
    /// still decrypt. Cleared once a frame decrypts under the current key.
    prev_cipher: Option<AesCipher>,
    transport_aad: Vec<u8>,
    to_app_tx: mpsc::UnboundedSender<SessionEvent>,
    /// The outgoing sequence, stamped on every frame we send (Rekey included).
    sent_seq: u64,
    /// The last sequence accepted from the peer (replay protection).
    last_valid_seq: u64,
    messages_since_rekey: u64,
    last_rekey_time: Instant,
    /// Only the host initiates rekeys, so the peers never rekey simultaneously; the
    /// client rekeys only in response to a received `Rekey`.
    is_rekey_initiator: bool,
}

impl<W> Transport<W>
where
    W: AsyncWrite + Unpin + Send,
{
    fn new(
        writer: W,
        cipher: AesCipher,
        transport_aad: Vec<u8>,
        to_app_tx: mpsc::UnboundedSender<SessionEvent>,
    ) -> Self {
        Self {
            writer,
            is_rekey_initiator: cipher.role() == NonceRole::Host,
            cipher,
            prev_cipher: None,
            transport_aad,
            to_app_tx,
            sent_seq: 0,
            last_valid_seq: 0,
            messages_since_rekey: 0,
            last_rekey_time: Instant::now(),
        }
    }

    /// A rotation is due once enough frames or time have passed — and only
    /// once the previous one has *completed*: a frame from the peer has
    /// decrypted under the current key, which is what clears `prev_cipher`.
    /// Rotating again before that discards the one retained key the peer's
    /// in-flight frames still use, and the session dies on "Decryption
    /// failed" (CRYPTO-06; both directions count toward the threshold, so
    /// crossing traffic reached it within one round trip). The peer's
    /// keep-alive bounds how long a rotation can be held back.
    fn rekey_due(&self) -> bool {
        self.is_rekey_initiator
            && self.prev_cipher.is_none()
            && (self.messages_since_rekey >= REKEY_MESSAGE_COUNT
                || self.last_rekey_time.elapsed() >= REKEY_TIME_INTERVAL)
    }

    async fn rekey(&mut self) -> Result<()> {
        initiate_rekey(
            &mut self.writer,
            &mut self.cipher,
            &mut self.prev_cipher,
            &self.transport_aad,
            &mut self.sent_seq,
            &mut self.messages_since_rekey,
            &mut self.last_rekey_time,
        )
        .await
        .map_err(|e| anyhow!("Network send error (rekey): {}", e))
    }

    /// Encrypt and write one outbound frame, first rotating the key if a rekey is due.
    /// Shared by the control and file lanes, so both stamp one monotonic sequence.
    async fn send(&mut self, mut msg: ProtocolMessage) -> Result<()> {
        if self.rekey_due() {
            self.rekey().await?;
        }

        self.sent_seq += 1;
        msg.set_seq(self.sent_seq);
        tracing::debug!("Sending message: {:?}", msg);

        let encrypted = self
            .cipher
            .encrypt(&msg.to_plain_bytes(), Some(&self.transport_aad));
        send_packet(&mut self.writer, &encrypted)
            .await
            .map_err(|e| anyhow!("Network send error: {}", e))?;
        self.messages_since_rekey += 1;

        // Report the wire seq of FileEnd and of the frame completing an outgoing text
        // (single frame or final chunk), so the app can correlate delivery receipts
        // (`Ack { acked_seq }`). Frames drain FIFO per lane, so order correlates.
        let confirmed = match msg {
            ProtocolMessage::FileEnd { seq } => Some(SessionEvent::FileSendComplete { seq }),
            ProtocolMessage::Text { seq, .. } => Some(SessionEvent::TextSendComplete { seq }),
            ProtocolMessage::TextChunk {
                seq,
                chunk_index,
                total_chunks,
                ..
            } if chunk_index + 1 == total_chunks => Some(SessionEvent::TextSendComplete { seq }),
            _ => None,
        };
        if let Some(event) = confirmed {
            let _ = self.to_app_tx.send(event);
        }
        Ok(())
    }

    /// Keep-alive ping on the shared outbound sequence. If a rekey is due the host
    /// rotates instead (the Rekey frame doubles as keep-alive), or a host that only
    /// receives would never rekey.
    async fn keep_alive(&mut self) -> Result<()> {
        if self.rekey_due() {
            return self.rekey().await;
        }
        self.sent_seq += 1;
        let ping = ProtocolMessage::Ping { seq: self.sent_seq };
        let encrypted = self
            .cipher
            .encrypt(&ping.to_plain_bytes(), Some(&self.transport_aad));
        send_packet(&mut self.writer, &encrypted)
            .await
            .map_err(|e| anyhow!("Network send error (keep-alive): {}", e))?;
        tracing::trace!("Keep-alive ping sent");
        Ok(())
    }

    /// One frame off the wire: decrypt, check its sequence, and either act on
    /// it here (`Rekey`, `Ping`) or hand it to the app.
    fn receive(&mut self, encrypted: &[u8]) -> Result<Received> {
        // Try the current key first; success proves the peer switched, so drop the
        // previous key. Otherwise fall back to it for frames sent before our Rekey.
        let decrypted = match self
            .cipher
            .decrypt_from_peer(encrypted, Some(&self.transport_aad))
        {
            Some(plaintext) => {
                self.prev_cipher = None;
                Some(plaintext)
            }
            None => self
                .prev_cipher
                .as_ref()
                .and_then(|pc| pc.decrypt_from_peer(encrypted, Some(&self.transport_aad))),
        };
        let Some(plaintext) = decrypted else {
            // Undecryptable under both keys over an ordered stream: desynced or
            // tampered, and unrecoverable, so fail closed.
            tracing::error!("Decryption failed - possible tampering or key mismatch!");
            let _ = self
                .to_app_tx
                .send(SessionEvent::Error("Decryption failed!".to_string()));
            return Ok(Received::Stop);
        };
        let Some(msg) = ProtocolMessage::from_plain_bytes(&plaintext) else {
            tracing::warn!("Failed to parse message from {} bytes", plaintext.len());
            return Ok(Received::Continue);
        };
        tracing::debug!("Received message: {:?}", msg);

        // Replay protection: drop replayed, duplicated or reordered frames.
        if let Err(e) = validate_message_sequence(&msg, &mut self.last_valid_seq) {
            tracing::warn!("Rejecting message due to invalid sequence: {}", e);
            return Ok(Received::Continue);
        }

        match msg {
            // Only the host rotates; obeying a client's Rekey would let it force
            // rotations and desync the session (EXT-01). Ignore it, keep the session.
            ProtocolMessage::Rekey { .. } if self.is_rekey_initiator => {
                tracing::warn!("Ignoring Rekey from a peer that is not the initiator");
            }
            // Not surfaced to the app.
            ProtocolMessage::Rekey { nonce, .. } => {
                use crate::core::rekey_session_key;
                let received_nonce: [u8; crate::REKEY_NONCE_SIZE] = nonce
                    .as_slice()
                    .try_into()
                    .map_err(|_| anyhow!("Invalid nonce length"))?;
                let next_key = rekey_session_key(&self.cipher.get_current_key(), &received_nonce);
                // Keep the old key briefly so a reordered old-key frame still
                // decrypts; dropped on the first new-key frame.
                self.prev_cipher = Some(self.cipher.clone());
                self.cipher = AesCipher::new_with_role(&next_key, self.cipher.role())?;
                self.messages_since_rekey = 0;
                self.last_rekey_time = Instant::now();
                tracing::info!("Session key rotated (received Rekey message)");
            }
            // Keep-alive only; not surfaced to the app.
            ProtocolMessage::Ping { .. } => tracing::trace!("Keep-alive ping received"),
            msg => {
                self.messages_since_rekey += 1;
                self.to_app_tx
                    .send(SessionEvent::MessageReceived(msg))
                    .map_err(|e| anyhow!("Event channel closed: {}", e))?;
            }
        }
        Ok(Received::Continue)
    }
}

/// Read one frame, owning the read half and handing it back with the result, so one
/// read can stay in flight across `select!` iterations.
async fn read_frame<R>(mut reader: R) -> (R, std::io::Result<Vec<u8>>)
where
    R: AsyncRead + Unpin,
{
    let result = recv_packet(&mut reader).await;
    (reader, result)
}

/// Drive `write` to completion while keeping the in-flight read moving, queueing any
/// frame that completes meanwhile in `inbox`. A write blocks once the peer's buffer is
/// full, and the peer drains it only by reading; if both stop reading while writing
/// (crossing transfers), both deadlock. [`MAX_FRAMES_HELD_DURING_WRITE`] bounds what a
/// peer that never reads can make us hold; reading also stops after a read error.
async fn write_while_reading<W, F, R>(
    write: W,
    pending_read: &mut std::pin::Pin<Box<F>>,
    restart_read: impl Fn(R) -> F,
    inbox: &mut std::collections::VecDeque<std::io::Result<Vec<u8>>>,
) -> W::Output
where
    W: std::future::Future,
    F: std::future::Future<Output = (R, std::io::Result<Vec<u8>>)>,
{
    tokio::pin!(write);
    loop {
        let reading =
            inbox.len() < MAX_FRAMES_HELD_DURING_WRITE && !matches!(inbox.back(), Some(Err(_)));
        let reader = tokio::select! {
            biased;
            out = &mut write => return out,
            (reader, frame) = pending_read.as_mut(), if reading => {
                inbox.push_back(frame);
                reader
            }
        };
        *pending_read = Box::pin(restart_read(reader));
    }
}

/// Everything the message loop can be woken by.
enum LoopEvent {
    Frame(std::io::Result<Vec<u8>>),
    Send(ProtocolMessage),
    KeepAlive,
    IdleTimeout,
}

/// Main message loop: send and receive encrypted messages with replay
/// protection, keep-alives and key rotation, until either side goes away.
async fn run_message_loop<S>(
    stream: S,
    cipher: AesCipher,
    transport_aad: Vec<u8>,
    to_app_tx: mpsc::UnboundedSender<SessionEvent>,
    mut from_app_rx: mpsc::UnboundedReceiver<ProtocolMessage>,
    mut file_rx: mpsc::Receiver<ProtocolMessage>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    // Test hooks (seconds) so integration tests can exercise keep-alive quickly.
    let recv_idle_timeout = duration_from_env("P2PEM_TEST_IDLE_TIMEOUT_SECS", RECV_IDLE_TIMEOUT);
    let keepalive_interval = duration_from_env("P2PEM_TEST_KEEPALIVE_SECS", KEEPALIVE_INTERVAL);

    // First tick only after a full interval (interval() would fire immediately).
    let mut keepalive = tokio::time::interval_at(
        tokio::time::Instant::now() + keepalive_interval,
        keepalive_interval,
    );
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // The in-flight read is kept alive ACROSS loop iterations: `select!` drops losing
    // futures and `recv_packet` is not cancel-safe (length prefix, then body), so a
    // fresh read per iteration discarded half-read bytes whenever a send won the race
    // and the next read parsed ciphertext as a length. Splitting the stream lets one
    // read stay pending (and keep moving during writes, see `write_while_reading`).
    let (reader, writer) = tokio::io::split(stream);
    let mut transport = Transport::new(writer, cipher, transport_aad, to_app_tx.clone());
    let mut pending_read = Box::pin(read_frame(reader));
    let mut inbox = std::collections::VecDeque::new();

    // Silence from the peer, measured from the last frame *received*. It used to
    // be a timeout around each individual read, restarted on every iteration —
    // so our own sends and keep-alive ticks kept re-arming it, and a peer that
    // had gone away without closing the socket was never detected.
    //
    // Until the peer's FIRST frame, the limit is its whole confirmation window
    // plus one idle window. Both sides enter this loop only after their own
    // user accepts the safety code, and the other may still be reading the
    // prompt — it sends nothing until it has answered. Arming the plain idle
    // timer here ended the session whenever two people compared the code more
    // than five minutes apart (audit NET-10). A peer that rejects closes the
    // socket, which is seen at once; one that vanished is still bounded.
    let idle = tokio::time::sleep(FINGERPRINT_CONFIRM_TIMEOUT + recv_idle_timeout);
    tokio::pin!(idle);

    loop {
        // Frames that completed during a write are handled first, in order.
        let event = match inbox.pop_front() {
            Some(frame) => LoopEvent::Frame(frame),
            None => {
                let mut finished = None;
                let event = tokio::select! {
                    (reader, frame) = &mut pending_read => {
                        finished = Some(reader);
                        LoopEvent::Frame(frame)
                    }
                    _ = &mut idle => LoopEvent::IdleTimeout,
                    // Control lane (text, typing, FileCancel): unbounded, so an
                    // abort is never stuck behind queued bulk data.
                    Some(msg) = from_app_rx.recv() => LoopEvent::Send(msg),
                    // Bounded file-data lane: its capacity applies backpressure so
                    // a slow peer paces the disk reader.
                    Some(msg) = file_rx.recv() => LoopEvent::Send(msg),
                    _ = keepalive.tick() => LoopEvent::KeepAlive,
                };
                if let Some(reader) = finished {
                    pending_read = Box::pin(read_frame(reader));
                }
                event
            }
        };

        let written = match event {
            LoopEvent::Frame(Ok(encrypted)) => {
                idle.as_mut()
                    .reset(tokio::time::Instant::now() + recv_idle_timeout);
                match transport.receive(&encrypted)? {
                    Received::Continue => continue,
                    Received::Stop => break,
                }
            }
            LoopEvent::Frame(Err(e)) => {
                let err_msg = format!("Network receive error: {}", e);
                tracing::error!("{}", err_msg);
                let _ = to_app_tx.send(SessionEvent::Error(err_msg));
                break;
            }
            LoopEvent::IdleTimeout => {
                let err_msg = format!("Receive idle timeout ({}s)", recv_idle_timeout.as_secs());
                tracing::warn!("{}", err_msg);
                let _ = to_app_tx.send(SessionEvent::Error(err_msg));
                break;
            }
            LoopEvent::Send(msg) => {
                write_while_reading(
                    transport.send(msg),
                    &mut pending_read,
                    read_frame,
                    &mut inbox,
                )
                .await
            }
            LoopEvent::KeepAlive => {
                write_while_reading(
                    transport.keep_alive(),
                    &mut pending_read,
                    read_frame,
                    &mut inbox,
                )
                .await
            }
        };
        if let Err(e) = written {
            tracing::error!("{}", e);
            let _ = to_app_tx.send(SessionEvent::Error(e.to_string()));
            break;
        }
    }

    emit(&to_app_tx, SessionEvent::Disconnected)?;

    Ok(())
}

/// Read a duration override (whole seconds) from an environment variable, falling back
/// to `default` when unset or invalid. Test hook, read in debug builds only: in a
/// release binary it would let the environment shorten a real session's timeouts.
fn duration_from_env(var: &str, default: std::time::Duration) -> std::time::Duration {
    if !cfg!(debug_assertions) {
        return default;
    }
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

    fn pem_of(key: &RsaPrivateKey) -> String {
        pem_encode_public(&RsaPublicKey::from(key)).unwrap()
    }

    /// The version goes on the wire as a plaintext big-endian u32, and it is
    /// the one thing an older peer reads before deciding how to talk to us.
    /// Two peers built from the same code agree on any value, so only a known
    /// answer catches it drifting: big-endian, high bytes zero, equal to
    /// `PROTOCOL_VERSION` (bump that, not this encoding, to change it).
    /// The plaintext version check is the first thing either side does, and
    /// the downgrade floor: anything before v3 (no ECDH-first handshake) is
    /// refused, naming the other side from our role, while v3 and the current
    /// version pass. A packet that is not exactly four bytes is refused too.
    #[test]
    fn peers_before_v3_are_refused_and_named() {
        let packet = |v: u32| v.to_be_bytes();
        for (us, them) in [(NonceRole::Host, "Client"), (NonceRole::Client, "Host")] {
            let err = check_peer_version(&packet(2), us).unwrap_err().to_string();
            assert_eq!(err, format!("{them} version 2 too old (need v3+)"));
            assert_eq!(check_peer_version(&packet(3), us).unwrap(), 3);
            let current = PROTOCOL_VERSION as u32;
            assert_eq!(check_peer_version(&packet(current), us).unwrap(), current);
            for bad in [&[0u8, 0, 3][..], &[0, 0, 0, 3, 0]] {
                assert!(check_peer_version(bad, us).is_err(), "{bad:?} accepted");
            }
        }
    }

    #[test]
    fn our_version_goes_on_the_wire_as_a_big_endian_u32() {
        let v = PROTOCOL_VERSION as u32;
        assert!(v < 256, "update this known answer for a version over 255");
        assert_eq!(our_version_bytes(), [0, 0, 0, v as u8]);
    }

    fn identity_proof(pem: String, signature: Vec<u8>, scheme: SignatureScheme) -> IdentityProof {
        IdentityProof {
            public_key_pem: pem,
            signature,
            version: PROTOCOL_VERSION as u32,
            chat_id: uuid::Uuid::new_v4(),
            signature_scheme: scheme,
        }
    }

    fn test_cipher() -> AesCipher {
        AesCipher::new(&rand::random::<[u8; crate::AES_KEY_SIZE]>()).unwrap()
    }

    fn text(seq: u64) -> ProtocolMessage {
        ProtocolMessage::Text {
            text: "hello".into(),
            timestamp: 0,
            seq,
        }
    }

    /// Run [`initiate_rekey`] on `cipher`; returns the Rekey frame, the retained key
    /// and the updated `(sent_seq, messages_since_rekey)`.
    async fn rekey(
        cipher: &mut AesCipher,
        aad: &[u8],
        mut sent_seq: u64,
        mut msgs: u64,
    ) -> (Vec<u8>, Option<AesCipher>, u64, u64) {
        let (mut a, mut b) = tokio::io::duplex(4096);
        let mut prev = None;
        let mut last = Instant::now();
        initiate_rekey(
            &mut a,
            cipher,
            &mut prev,
            aad,
            &mut sent_seq,
            &mut msgs,
            &mut last,
        )
        .await
        .unwrap();
        (recv_packet(&mut b).await.unwrap(), prev, sent_seq, msgs)
    }

    /// A port that was free a moment ago, so connecting to it fails fast.
    async fn dead_port() -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap().port()
    }

    #[test]
    fn constant_time_eq_matches_plain_equality() {
        assert!(constant_time_eq(b"", b""));
        assert!(constant_time_eq(b"correct horse", b"correct horse"));
        assert!(!constant_time_eq(b"correct horse", b"correct horsf"));
        // Different lengths compare unequal on the same path as a same-length mismatch.
        assert!(!constant_time_eq(b"short", b"considerably longer"));
        assert!(!constant_time_eq(b"considerably longer", b"short"));
        assert!(!constant_time_eq(b"", b"x"));
        assert!(!constant_time_eq(b"secret", b"secretsauce"));
    }

    /// The Rekey goes out under the CURRENT key and only then rotates, so a peer
    /// still on the old key decrypts it and derives the same next key.
    #[tokio::test]
    async fn initiate_rekey_frame_decryptable_by_peer_and_keys_agree() {
        let key = [7u8; crate::AES_KEY_SIZE];
        let mut host = AesCipher::new_with_role(&key, NonceRole::Host).unwrap();
        let peer = AesCipher::new_with_role(&key, NonceRole::Client).unwrap();
        let aad = b"transport-aad".to_vec();

        let (frame, prev, sent_seq, msgs) = rekey(&mut host, &aad, 5, 9).await;
        assert!(prev.is_some(), "initiator must retain the previous key");

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

        assert_eq!(host.get_current_key(), peer_next);
        assert_eq!(sent_seq, 6);
        assert_eq!(msgs, 0);
    }

    /// NET-01: one key and AAD serve both directions, so a frame of ours echoed back
    /// on the path must not be accepted as the peer's.
    #[test]
    fn a_reflected_frame_is_not_accepted_as_the_peers() {
        let key: [u8; 32] = rand::random();
        let aad = b"transport-aad".to_vec();
        let ours = AesCipher::new_with_role(&key, NonceRole::Client).unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut transport = Transport::new(tokio::io::sink(), ours.clone(), aad.clone(), tx);

        let echoed = text(7);
        let frame = ours.encrypt(&echoed.to_plain_bytes(), Some(&aad));
        assert!(matches!(transport.receive(&frame).unwrap(), Received::Stop));
        while let Ok(event) = rx.try_recv() {
            assert!(
                !matches!(event, SessionEvent::MessageReceived(_)),
                "our own frame was surfaced as the peer's: {event:?}"
            );
        }

        // The genuine peer's frame still goes through.
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut transport = Transport::new(tokio::io::sink(), ours, aad.clone(), tx);
        let peer = AesCipher::new_with_role(&key, NonceRole::Host).unwrap();
        let frame = peer.encrypt(&echoed.to_plain_bytes(), Some(&aad));
        assert!(matches!(
            transport.receive(&frame).unwrap(),
            Received::Continue
        ));
        assert!(matches!(
            rx.try_recv().unwrap(),
            SessionEvent::MessageReceived(ProtocolMessage::Text { .. })
        ));
    }

    /// CRYPTO-06: the host does not start a rotation while the previous one
    /// is open — it still holds the old key because no frame from the peer
    /// has decrypted under the new one. The first such frame releases it, so
    /// rotations are held back, never skipped.
    #[tokio::test]
    async fn a_rekey_waits_until_the_peer_has_switched_to_the_last_one() {
        let key: [u8; 32] = rand::random();
        let aad = b"transport-aad".to_vec();
        let host = AesCipher::new_with_role(&key, NonceRole::Host).unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut transport = Transport::new(tokio::io::sink(), host, aad.clone(), tx);

        transport.messages_since_rekey = REKEY_MESSAGE_COUNT;
        assert!(transport.rekey_due(), "the threshold makes a rotation due");
        transport.rekey().await.unwrap();
        assert!(transport.prev_cipher.is_some(), "the old key is retained");

        transport.messages_since_rekey = REKEY_MESSAGE_COUNT;
        assert!(
            !transport.rekey_due(),
            "a second rotation started before the peer switched would drop the key its in-flight frames use"
        );

        // A frame from the client under the new key: the client has switched.
        let client =
            AesCipher::new_with_role(&transport.cipher.get_current_key(), NonceRole::Client)
                .unwrap();
        let ping = ProtocolMessage::Ping { seq: 1 };
        let frame = client.encrypt(&ping.to_plain_bytes(), Some(&aad));
        assert!(matches!(
            transport.receive(&frame).unwrap(),
            Received::Continue
        ));
        assert!(transport.prev_cipher.is_none());
        assert!(transport.rekey_due(), "the held rotation is released");
    }

    /// When a rotation is due, from both directions: a fresh host transport is
    /// not (rotating every frame would be its own failure), nor one message
    /// short of `REKEY_MESSAGE_COUNT`; the count or `REKEY_TIME_INTERVAL`
    /// elapsed makes it due; and the client, which never initiates, never is.
    #[test]
    fn a_rekey_is_due_by_count_or_by_time_and_only_on_the_host() {
        let key: [u8; 32] = rand::random();
        let aad = b"transport-aad".to_vec();
        let (tx, _rx) = mpsc::unbounded_channel();
        let host = AesCipher::new_with_role(&key, NonceRole::Host).unwrap();
        let mut transport = Transport::new(tokio::io::sink(), host, aad.clone(), tx.clone());
        assert!(!transport.rekey_due(), "a fresh session rotates at once");
        transport.messages_since_rekey = REKEY_MESSAGE_COUNT - 1;
        assert!(!transport.rekey_due(), "rotated a message early");
        transport.messages_since_rekey = REKEY_MESSAGE_COUNT;
        assert!(transport.rekey_due());

        transport.messages_since_rekey = 0;
        let Some(long_ago) = std::time::Instant::now().checked_sub(REKEY_TIME_INTERVAL) else {
            return; // a machine up for less than the interval cannot express it
        };
        transport.last_rekey_time = long_ago;
        assert!(
            transport.rekey_due(),
            "the time interval does not trigger a rotation"
        );

        let client = AesCipher::new_with_role(&key, NonceRole::Client).unwrap();
        let mut transport = Transport::new(tokio::io::sink(), client, aad, tx);
        transport.messages_since_rekey = REKEY_MESSAGE_COUNT;
        transport.last_rekey_time = long_ago;
        assert!(!transport.rekey_due(), "the client initiated a rotation");
    }

    /// Every frame sent counts toward the rotation threshold. Received frames
    /// count too, which is why the end-to-end rekey tests could not tell: a
    /// sender whose own frames did not count still rotated on the peer's.
    #[tokio::test]
    async fn each_frame_sent_counts_toward_the_next_rekey() {
        let key: [u8; 32] = rand::random();
        let (tx, _rx) = mpsc::unbounded_channel();
        let client = AesCipher::new_with_role(&key, NonceRole::Client).unwrap();
        let mut transport = Transport::new(tokio::io::sink(), client, b"aad".to_vec(), tx);
        for sent in 1..=3 {
            transport
                .send(ProtocolMessage::Ping { seq: 0 })
                .await
                .unwrap();
            assert_eq!(transport.messages_since_rekey, sent);
        }
    }

    /// EXT-01: a client's `Rekey` must not move the host's key.
    #[test]
    fn the_host_ignores_a_rekey_from_the_client() {
        let key: [u8; 32] = rand::random();
        let aad = b"transport-aad".to_vec();
        let host = AesCipher::new_with_role(&key, NonceRole::Host).unwrap();
        let client = AesCipher::new_with_role(&key, NonceRole::Client).unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut transport = Transport::new(tokio::io::sink(), host, aad.clone(), tx);

        let nonce: [u8; crate::REKEY_NONCE_SIZE] = rand::random();
        let rekey = ProtocolMessage::Rekey {
            seq: 1,
            nonce: nonce.to_vec(),
        };
        let frame = client.encrypt(&rekey.to_plain_bytes(), Some(&aad));
        assert!(matches!(
            transport.receive(&frame).unwrap(),
            Received::Continue
        ));
        assert_eq!(transport.cipher.get_current_key(), key);
        assert!(transport.prev_cipher.is_none());
    }

    /// After a rotation, a frame the peer sent under the OLD key still decrypts via
    /// the retained key, and the window closes once a NEW-key frame arrives.
    #[tokio::test]
    async fn old_key_in_flight_frames_survive_a_rekey() {
        let key = [3u8; crate::AES_KEY_SIZE];
        let mut host = AesCipher::new_with_role(&key, NonceRole::Host).unwrap();
        let client_old = AesCipher::new_with_role(&key, NonceRole::Client).unwrap();
        let aad = b"aad".to_vec();
        let (_, mut prev, _, _) = rekey(&mut host, &aad, 0, 0).await;

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

        let old_frame = client_old.encrypt(b"hello-old", Some(&aad));
        assert_eq!(
            decrypt(&host, &mut prev, &old_frame).as_deref(),
            Some(&b"hello-old"[..]),
            "old-key in-flight frame must decrypt via the retained key"
        );
        assert!(prev.is_some(), "prev retained while old-key frames arrive");

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
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let live_port = listener.local_addr().unwrap().port();
        let dead_port = dead_port().await;

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

        match rx.try_recv() {
            Ok(SessionEvent::Warning(msg)) => {
                assert!(msg.contains(&dead_port.to_string()));
                assert!(msg.contains(&live_port.to_string()));
            }
            other => panic!("expected Warning event, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn connect_first_reachable_disables_nagle() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (events, _rx) = mpsc::unbounded_channel();
        let (stream, _, _) = connect_first_reachable(&[("127.0.0.1".to_string(), port)], &events)
            .await
            .unwrap();
        assert!(
            stream.nodelay().unwrap(),
            "NET-04: Nagle left on a session socket"
        );
        let reusable = crate::network::punch::connect_reusable("127.0.0.1", port)
            .await
            .unwrap();
        assert!(
            reusable.nodelay().unwrap(),
            "NET-04: Nagle left on the relay socket"
        );
    }

    #[tokio::test]
    async fn connect_first_reachable_errors_when_all_dead() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let targets = vec![("127.0.0.1".to_string(), dead_port().await)];
        assert!(connect_first_reachable(&targets, &tx).await.is_err());
        assert!(connect_first_reachable(&[], &tx).await.is_err());
    }

    /// An AEAD failure over a reliable stream means desync or tampering: the session
    /// must end promptly and report it rather than spin on every later packet.
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
        send_packet(&mut peer_stream, &[0u8; 64]).await.unwrap();

        let result = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("loop must terminate promptly after a decryption failure");
        assert!(result.unwrap().is_ok());

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
        drop(peer_stream);
    }

    /// Dead-peer detection, which nothing tested: once the peer has spoken,
    /// silence longer than the idle window ends the session with an error the
    /// app can show — a peer that vanished without closing its socket (a
    /// laptop lid, a dropped Wi-Fi) must not leave a conversation reading as
    /// connected. The longer first-frame window (NET-10) must not leak past
    /// the first frame. Timers shrunk with the debug-only hooks.
    #[tokio::test]
    async fn a_peer_that_goes_silent_after_speaking_is_detected() {
        struct Restore;
        impl Drop for Restore {
            fn drop(&mut self) {
                std::env::remove_var("P2PEM_TEST_IDLE_TIMEOUT_SECS");
                std::env::remove_var("P2PEM_TEST_KEEPALIVE_SECS");
            }
        }
        let _restore = Restore;
        std::env::set_var("P2PEM_TEST_IDLE_TIMEOUT_SECS", "2");
        std::env::set_var("P2PEM_TEST_KEEPALIVE_SECS", "60");

        let key: [u8; 32] = rand::random();
        let aad = b"transport".to_vec();
        let ours = AesCipher::new_with_role(&key, NonceRole::Host).unwrap();
        let theirs = AesCipher::new_with_role(&key, NonceRole::Client).unwrap();
        let (loop_stream, mut peer_stream) = tokio::io::duplex(4096);
        let (to_app_tx, mut to_app_rx) = mpsc::unbounded_channel();
        let (_from_app_tx, from_app_rx) = mpsc::unbounded_channel();
        let (_file_tx, file_rx) = mpsc::channel(1);
        let loop_aad = aad.clone();
        let handle = tokio::spawn(async move {
            run_message_loop(loop_stream, ours, loop_aad, to_app_tx, from_app_rx, file_rx).await
        });

        // The peer speaks once, then says nothing more — socket still open.
        let ping = ProtocolMessage::Ping { seq: 1 };
        send_packet(
            &mut peer_stream,
            &theirs.encrypt(&ping.to_plain_bytes(), Some(&aad)),
        )
        .await
        .unwrap();

        let started = std::time::Instant::now();
        tokio::time::timeout(std::time::Duration::from_secs(20), handle)
            .await
            .expect("a silent peer was never detected")
            .unwrap()
            .unwrap();
        assert!(
            started.elapsed() >= std::time::Duration::from_millis(1500),
            "the session ended before the idle window ran out"
        );
        let mut idle_reported = false;
        while let Ok(ev) = to_app_rx.try_recv() {
            if let SessionEvent::Error(e) = ev {
                idle_reported |= e.contains("idle timeout");
            }
        }
        assert!(
            idle_reported,
            "the idle timeout was not reported to the app"
        );
        drop(peer_stream);
    }

    /// A hand-rolled v3 exchange: both sides derive the same key from the transcript
    /// `HostVersion || ClientVersion || HostEphemeral || ClientEphemeral`, and each
    /// decrypts the other's identity proof under the identity-proof AAD.
    #[tokio::test]
    async fn test_full_handshake_with_forward_secrecy() -> Result<()> {
        fn derive(
            secret: EphemeralSecret,
            theirs: &X25519PublicKey,
            host_eph: &[u8],
            client_eph: &[u8],
        ) -> Result<([u8; crate::AES_KEY_SIZE], AesCipher, Vec<u8>)> {
            let v = our_version_bytes();
            let salt = Sha256::digest([&v[..], &v[..], host_eph, client_eph].concat());
            let key = derive_session_key(secret, theirs, Some(&salt), HKDF_INFO)?;
            Ok((
                key,
                AesCipher::new(&key)?,
                labeled_aad(b"identity-proof", &salt),
            ))
        }
        fn proof_bytes(privkey: &RsaPrivateKey, ephemeral: &[u8]) -> Result<Vec<u8>> {
            let scheme = SignatureScheme::RsaPss;
            let sig = build_identity_signature(scheme, privkey, ephemeral)?;
            Ok(bincode::serialize(&identity_proof(
                pem_of(privkey),
                sig,
                scheme,
            ))?)
        }

        let host_priv = generate_rsa_keypair(RSA_KEY_BITS)?;
        let client_priv = generate_rsa_keypair(RSA_KEY_BITS)?;
        let (mut hs, mut cs) = tokio::io::duplex(8192);

        let host = tokio::spawn(async move {
            send_packet(&mut hs, &our_version_bytes()).await?;
            recv_packet(&mut hs).await?;
            let (secret, public) = generate_ephemeral_keypair();
            send_packet(&mut hs, public.as_bytes()).await?;
            let client_eph = recv_packet(&mut hs).await?;
            let their = parse_x25519_public(&client_eph)?;
            let (key, cipher, aad) = derive(secret, &their, public.as_bytes(), &client_eph)?;
            let proof = proof_bytes(&host_priv, public.as_bytes())?;
            send_packet(&mut hs, &cipher.encrypt(&proof, Some(&aad))).await?;
            let client_proof = cipher
                .decrypt(&recv_packet(&mut hs).await?, Some(&aad))
                .expect("client proof decrypt should succeed");
            assert_eq!(
                decode_identity_proof(&client_proof)?.version,
                PROTOCOL_VERSION as u32
            );
            Ok::<_, anyhow::Error>(key)
        });

        let client = tokio::spawn(async move {
            recv_packet(&mut cs).await?;
            send_packet(&mut cs, &our_version_bytes()).await?;
            let host_eph = recv_packet(&mut cs).await?;
            let their = parse_x25519_public(&host_eph)?;
            let (secret, public) = generate_ephemeral_keypair();
            send_packet(&mut cs, public.as_bytes()).await?;
            let (key, cipher, aad) = derive(secret, &their, &host_eph, public.as_bytes())?;
            let host_proof = cipher
                .decrypt(&recv_packet(&mut cs).await?, Some(&aad))
                .expect("host proof decrypt should succeed");
            assert_eq!(
                decode_identity_proof(&host_proof)?.version,
                PROTOCOL_VERSION as u32
            );
            let proof = proof_bytes(&client_priv, public.as_bytes())?;
            send_packet(&mut cs, &cipher.encrypt(&proof, Some(&aad))).await?;
            Ok::<_, anyhow::Error>(key)
        });

        assert_eq!(host.await.unwrap()?, client.await.unwrap()?);
        Ok(())
    }

    /// Two current peers settle on Ed25519, and the fingerprint (what TOFU pins) is
    /// still that of the RSA identity.
    #[tokio::test]
    async fn a_live_handshake_negotiates_ed25519_without_changing_the_fingerprint() {
        let host_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let client_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let expected_host_fp = fingerprint_pubkey(pem_of(&host_priv).as_bytes()).unwrap();
        let expected_client_fp = fingerprint_pubkey(pem_of(&client_priv).as_bytes()).unwrap();

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

    /// The proof is what a v5 client and host both compute; if either side's
    /// label, salt or info drifted, every password-protected connection would
    /// fail. Pinned by a known answer computed with Python's `hmac` (HKDF by
    /// hand), not by comparing the function with itself.
    #[test]
    fn the_password_proof_matches_its_known_answer() {
        let transcript: Vec<u8> = (0u8..32).collect();
        assert_eq!(
            hex::encode(password_proof("correct horse battery staple", &transcript)),
            "ee5b734bce4cb7dec82ea3decfcf34c7f5bfc1473b1b254d91638b2021da9151"
        );
        assert_ne!(
            password_proof("correct horse battery staple", &transcript[1..]),
            password_proof("correct horse battery staple", &transcript),
            "bound to the transcript"
        );
    }

    /// Both halves of a handshake, with the streams kept for what follows it.
    async fn tunnels_with_streams(
        host_version: u32,
        client_version: u32,
    ) -> (
        (EstablishedTunnel, tokio::io::DuplexStream),
        (EstablishedTunnel, tokio::io::DuplexStream),
    ) {
        let host_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let client_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let (mut host_stream, mut client_stream) = tokio::io::duplex(1 << 16);
        let host = tokio::spawn(async move {
            let t = host_handshake_as(
                &mut host_stream,
                &host_priv,
                uuid::Uuid::new_v4(),
                host_version.to_be_bytes(),
            )
            .await
            .unwrap();
            (t, host_stream)
        });
        let client = client_handshake_as(
            &mut client_stream,
            &client_priv,
            uuid::Uuid::new_v4(),
            client_version.to_be_bytes(),
        )
        .await
        .unwrap();
        (host.await.unwrap(), (client, client_stream))
    }

    /// The host's side of the gate, played by hand: ask for a password and
    /// return exactly what the client sent back.
    async fn ask_for_password(
        tunnel: &EstablishedTunnel,
        stream: &mut tokio::io::DuplexStream,
    ) -> Vec<u8> {
        send_packet(
            stream,
            &tunnel.cipher.encrypt(&[1], Some(&tunnel.transport_aad)),
        )
        .await
        .unwrap();
        let ct = recv_packet_with_timeout(stream).await.unwrap();
        tunnel
            .cipher
            .decrypt_from_peer(&ct, Some(&tunnel.transport_aad))
            .unwrap()
    }

    /// CRYPTO-05: the gate runs before the user has compared the safety code,
    /// so whoever answers may be posing as the host. It used to receive the
    /// password in clear. Now it receives a proof bound to its own transcript:
    /// no password, nothing it can replay to the real host.
    #[tokio::test]
    async fn a_host_nobody_has_verified_never_receives_the_password() {
        let password = "a password worth stealing";
        let v = PROTOCOL_VERSION as u32;
        let ((fake, mut fake_stream), (client, mut client_stream)) =
            tunnels_with_streams(v, v).await;
        let gate = tokio::spawn(async move {
            client_password_gate(
                &mut client_stream,
                &client.cipher,
                &client.transport_aad,
                client.peer_version,
                Some(password),
            )
            .await
        });
        let got = ask_for_password(&fake, &mut fake_stream).await;
        gate.await.unwrap().unwrap();

        assert!(
            !got.windows(password.len())
                .any(|w| w == password.as_bytes()),
            "the password crossed the wire"
        );
        assert_eq!(got, password_proof(password, &fake.transport_aad));
        // And it is useless on any other connection.
        let ((other, _), _) = tunnels_with_streams(v, v).await;
        assert_ne!(got, password_proof(password, &other.transport_aad));
    }

    /// A host from before the proof asks for the password itself. Sending it
    /// would undo the fix — a fake host need only claim to be old — so the
    /// client refuses, sends nothing, and says why.
    #[tokio::test]
    async fn an_older_host_asking_for_the_password_is_not_given_it() {
        let ((old_host, mut host_stream), (client, mut client_stream)) =
            tunnels_with_streams(4, PROTOCOL_VERSION as u32).await;
        assert_eq!(client.peer_version, 4);
        let gate = tokio::spawn(async move {
            client_password_gate(
                &mut client_stream,
                &client.cipher,
                &client.transport_aad,
                client.peer_version,
                Some("secret-secret"),
            )
            .await
        });
        let got = ask_for_password(&old_host, &mut host_stream).await;
        let err = gate.await.unwrap().unwrap_err().to_string();
        assert!(got.is_empty(), "something was sent: {got:?}");
        assert!(err.contains("Update the app on the host"), "{err}");
    }

    /// The host side: a current client proves, an older one still sends the
    /// password itself and is still let in, and a wrong answer either way is
    /// refused.
    #[tokio::test]
    async fn the_host_admits_the_right_password_from_old_and_new_clients_only() {
        let password = "the right password";
        for (client_version, right, wrong) in [
            (
                PROTOCOL_VERSION as u32,
                None,                           // the proof, computed below from the transcript
                b"the right password".to_vec(), // plaintext is not a proof
            ),
            (
                4,
                Some(password.as_bytes().to_vec()),
                b"a wrong password".to_vec(),
            ),
        ] {
            for answer_right in [true, false] {
                let ((host, mut host_stream), (client, mut client_stream)) =
                    tunnels_with_streams(PROTOCOL_VERSION as u32, client_version).await;
                let answer = if answer_right {
                    right
                        .clone()
                        .unwrap_or_else(|| password_proof(password, &client.transport_aad).to_vec())
                } else {
                    wrong.clone()
                };
                let client_side = tokio::spawn(async move {
                    let _flag = recv_packet_with_timeout(&mut client_stream).await.unwrap();
                    send_packet(
                        &mut client_stream,
                        &client.cipher.encrypt(&answer, Some(&client.transport_aad)),
                    )
                    .await
                    .unwrap();
                });
                let res = host_password_gate(
                    &mut host_stream,
                    &host.cipher,
                    &host.transport_aad,
                    host.peer_version,
                    Some(password),
                )
                .await;
                client_side.await.unwrap();
                assert_eq!(
                    res.is_ok(),
                    answer_right,
                    "client v{client_version}, right answer: {answer_right}"
                );
            }
        }
    }

    /// Run both handshake halves announcing the given versions.
    async fn handshake_pair(
        host_version: u32,
        client_version: u32,
    ) -> (Result<EstablishedTunnel>, Result<EstablishedTunnel>) {
        let host_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let client_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let (mut host_stream, mut client_stream) = tokio::io::duplex(1 << 16);
        let host = tokio::spawn(async move {
            host_handshake_as(
                &mut host_stream,
                &host_priv,
                uuid::Uuid::new_v4(),
                host_version.to_be_bytes(),
            )
            .await
        });
        let client = client_handshake_as(
            &mut client_stream,
            &client_priv,
            uuid::Uuid::new_v4(),
            client_version.to_be_bytes(),
        )
        .await;
        (host.await.unwrap(), client)
    }

    /// CRYPTO-02: two current peers use the commitment, and agree on the SAS.
    #[tokio::test]
    async fn current_peers_commit_to_their_ephemeral_keys() {
        let (host, client) = handshake_pair(PROTOCOL_VERSION as u32, PROTOCOL_VERSION as u32).await;
        let (host, client) = (host.unwrap(), client.unwrap());
        assert!(host.sas_committed && client.sas_committed);
        assert_eq!(
            crate::core::derive_sas(&host.transport_aad),
            crate::core::derive_sas(&client.transport_aad)
        );
    }

    /// A v3 peer on either side still connects, without the commitment, which both
    /// sides report so the UI can warn that the code is weak.
    #[tokio::test]
    async fn a_v3_peer_still_connects_but_without_the_commitment() {
        for (h, c) in [(3, PROTOCOL_VERSION as u32), (PROTOCOL_VERSION as u32, 3)] {
            let (host, client) = handshake_pair(h, c).await;
            let (host, client) = (host.unwrap(), client.unwrap());
            assert!(
                !host.sas_committed && !client.sas_committed,
                "host v{h}, client v{c}"
            );
            assert_eq!(
                crate::core::derive_sas(&host.transport_aad),
                crate::core::derive_sas(&client.transport_aad)
            );
        }
    }

    /// CRYPTO-02, the half the UI sees: the weak-code warning is sent before the
    /// verification prompt for a peer without the commitment, and never for a
    /// current one (a warning on every connection would teach people to ignore
    /// it). The user rejects at the prompt, so the message loop never starts.
    #[tokio::test]
    async fn only_an_uncommitted_peer_gets_the_weak_code_warning() {
        for (peer_version, warned) in [(PROTOCOL_VERSION as u32, false), (3, true)] {
            let (host, _client) = handshake_pair(PROTOCOL_VERSION as u32, peer_version).await;
            let tunnel = host.unwrap();
            let (to_app_tx, mut to_app_rx) = mpsc::unbounded_channel();
            let (_from_app_tx, from_app_rx) = mpsc::unbounded_channel();
            let (_file_tx, file_rx) = mpsc::channel(1);
            let (confirm_tx, confirm_rx) = mpsc::unbounded_channel();
            confirm_tx.send(false).unwrap();
            let (mut stream, _other) = tokio::io::duplex(64);
            let prompt = SessionEvent::Ready; // stands in for the prompt
            let result = verify_then_run(
                &mut stream,
                tunnel,
                prompt,
                uuid::Uuid::new_v4(),
                to_app_tx,
                from_app_rx,
                file_rx,
                confirm_rx,
            )
            .await;
            assert!(result.is_err(), "a rejected peer was let through");
            let events: Vec<_> = std::iter::from_fn(|| to_app_rx.try_recv().ok()).collect();
            let warnings: Vec<_> = events
                .iter()
                .filter(|e| matches!(e, SessionEvent::Warning(w) if w == LEGACY_SAS_WARNING))
                .collect();
            assert_eq!(
                warnings.len(),
                usize::from(warned),
                "peer v{peer_version}: {events:?}"
            );
            if warned {
                assert!(
                    matches!(events.first(), Some(SessionEvent::Warning(_))),
                    "the warning must come before the prompt: {events:?}"
                );
            }
        }
    }

    /// A client revealing a key other than the one it committed to (a MITM shopping
    /// for a matching code) is refused before any key is derived.
    #[tokio::test]
    async fn a_client_that_breaks_its_commitment_is_refused() {
        let host_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let (mut host_stream, mut wire) = tokio::io::duplex(1 << 16);
        let host = tokio::spawn(async move {
            host_handshake(&mut host_stream, &host_priv, uuid::Uuid::new_v4()).await
        });

        let _host_version = recv_packet(&mut wire).await.unwrap();
        send_packet(&mut wire, &our_version_bytes()).await.unwrap();
        let (_, committed_key) = generate_ephemeral_keypair();
        send_packet(&mut wire, &ephemeral_commitment(committed_key.as_bytes()))
            .await
            .unwrap();
        let _host_ephemeral = recv_packet(&mut wire).await.unwrap();
        let (_, other_key) = generate_ephemeral_keypair();
        send_packet(&mut wire, other_key.as_bytes()).await.unwrap();

        let Err(err) = host.await.unwrap() else {
            panic!("a broken commitment must be refused");
        };
        assert!(err.to_string().contains("commitment"), "{err}");
    }

    /// The commitment is frozen wire format.
    #[test]
    fn the_commitment_format_is_frozen() {
        let c = ephemeral_commitment(&[0u8; 32]);
        let mut h = Sha256::new();
        h.update(b"p2pem-v4-ephemeral-commitment");
        h.update([0u8; 32]);
        assert_eq!(c.as_slice(), h.finalize().as_slice());
        assert!(check_commitment(&c, &[0u8; 32]).is_ok());
        assert!(check_commitment(&c, &[1u8; 32]).is_err());
        assert!(check_commitment(&c[..31], &[0u8; 32]).is_err());
    }

    /// Without a valid binding an Ed25519 subkey is not an identity: accepting it
    /// would pair a stolen `public_key_pem` (a trusted fingerprint) with the
    /// attacker's signing key.
    #[test]
    fn an_ed25519_proof_without_a_valid_binding_is_rejected() {
        let identity = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let ephemeral = [7u8; 32];
        let signature =
            build_identity_signature(SignatureScheme::Ed25519, &identity, &ephemeral).unwrap();
        let pem = pem_of(&identity);
        let proof = |blob: &Ed25519Proof| {
            let sig = bincode::serialize(blob).unwrap();
            identity_proof(pem.clone(), sig, SignatureScheme::Ed25519)
        };

        let good = identity_proof(pem.clone(), signature.clone(), SignatureScheme::Ed25519);
        assert!(verify_identity_proof(&good, &ephemeral).is_ok());

        let blob: Ed25519Proof = bincode::deserialize(&signature).unwrap();

        let mut stripped = blob.clone();
        stripped.binding = Vec::new();
        assert!(verify_identity_proof(&proof(&stripped), &ephemeral).is_err());

        // The attacker's subkey with the victim's identity and binding.
        let attacker = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let attacker_key = derive_ed25519_subkey(&attacker).unwrap();
        let mut digest = Sha256::new();
        digest.update(b"IDENTITY_PROOF");
        digest.update(ephemeral);
        let mut swapped = blob.clone();
        swapped.public = attacker_key.verifying_key().to_bytes().to_vec();
        swapped.signature = sign_ed25519(&attacker_key, &digest.finalize())
            .to_bytes()
            .to_vec();
        assert!(
            verify_identity_proof(&proof(&swapped), &ephemeral).is_err(),
            "a subkey the identity never vouched for must be refused"
        );

        // A binding made by somebody else's identity.
        let mut foreign = blob.clone();
        foreign.binding = cross_sign_ed25519(&attacker, &attacker_key.verifying_key()).unwrap();
        assert!(verify_identity_proof(&proof(&foreign), &ephemeral).is_err());

        // Signing a different ephemeral key is still caught.
        assert!(verify_identity_proof(&good, &[8u8; 32]).is_err());
    }

    /// bincode decodes positionally, so a field appended to `IdentityProof` breaks
    /// every older peer (`#[serde(default)]` does not help); this caught exactly that
    /// against a released 1.16.0 server, hence the Ed25519 material rides in
    /// `signature`.
    #[test]
    fn the_identity_proof_wire_shape_is_unchanged_for_older_peers() {
        #[derive(serde::Deserialize)]
        #[allow(dead_code)]
        struct AsOlderPeersSeeIt {
            public_key_pem: String,
            signature: Vec<u8>,
            version: u32,
            chat_id: uuid::Uuid,
            signature_scheme: SignatureScheme,
        }

        for scheme in [SignatureScheme::RsaPss, SignatureScheme::Ed25519] {
            let identity = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
            let signature = build_identity_signature(scheme, &identity, &[3u8; 32]).unwrap();
            let bytes =
                bincode::serialize(&identity_proof(pem_of(&identity), signature, scheme)).unwrap();
            assert!(
                bincode::deserialize::<AsOlderPeersSeeIt>(&bytes).is_ok(),
                "{scheme} proof no longer decodes against the released field layout"
            );
        }
    }

    /// `proof || junk` must not decode to `proof`, while the encoder's own output
    /// still parses (a compatibility check as much as a rejection check).
    #[test]
    fn an_identity_proof_with_trailing_bytes_is_refused() {
        let proof = identity_proof(
            "test-public-key".into(),
            vec![1, 2, 3, 4],
            SignatureScheme::RsaPss,
        );
        let bytes = bincode::serialize(&proof).unwrap();

        let decoded = decode_identity_proof(&bytes).expect("the encoder's own output must parse");
        assert_eq!(decoded.public_key_pem, proof.public_key_pem);
        assert_eq!(decoded.chat_id, proof.chat_id);
        assert_eq!(decoded.signature, proof.signature);

        let mut padded = bytes.clone();
        padded.extend_from_slice(b"appended by someone on the path");
        assert!(
            decode_identity_proof(&padded).is_err(),
            "trailing bytes must not decode to the same proof"
        );
    }

    /// Identity-proof and transport ciphertexts are bound to their transcript AAD.
    #[test]
    fn test_aad_binding_rejects_wrong_context() {
        let proof = identity_proof(
            "test-public-key".into(),
            vec![1, 2, 3, 4],
            SignatureScheme::RsaPss,
        );
        let cases = [
            (&b"identity-proof"[..], bincode::serialize(&proof).unwrap()),
            (&b"transport"[..], text(1).to_plain_bytes()),
        ];
        let cipher = test_cipher();
        for (label, plaintext) in cases {
            let encrypted = cipher.encrypt(&plaintext, Some(&labeled_aad(label, b"session-a")));
            assert!(
                cipher
                    .decrypt(&encrypted, Some(&labeled_aad(label, b"session-b")))
                    .is_none(),
                "must not decrypt under a different transcript binding"
            );
            assert!(
                cipher.decrypt(&encrypted, None).is_none(),
                "must not decrypt when AAD is stripped"
            );
        }
    }

    /// Only strictly increasing sequences pass: duplicates, replays and reordering
    /// are refused without moving the mark; gaps are fine.
    #[test]
    fn test_transport_layer_replay_protection() {
        let mut last = 0u64;
        // (seq, expected error text, last accepted afterwards)
        let cases = [
            (1, None, 1),
            (2, None, 2),
            (1, Some("Replay"), 2),
            (2, Some("Replay"), 2),
            (3, None, 3),
            (2, Some("Out-of-order"), 3),
            (100, None, 100),
        ];
        for (seq, err, after) in cases {
            let result = validate_message_sequence(&text(seq), &mut last);
            match err {
                None => assert!(result.is_ok(), "seq {seq}"),
                Some(e) => assert!(result.unwrap_err().contains(e), "seq {seq}"),
            }
            assert_eq!(last, after);
        }
    }

    /// Handshake messages carry no seq and pass without moving the mark.
    #[test]
    fn test_handshake_messages_bypass_sequence_check() {
        let mut last = 5;
        for msg in [
            ProtocolMessage::Version { version: 3 },
            ProtocolMessage::EphemeralKey {
                public_key: vec![0u8; 32],
            },
            ProtocolMessage::SupportedSignatureSchemes {
                schemes: vec![SignatureScheme::RsaPss.to_u8()],
            },
        ] {
            assert!(validate_message_sequence(&msg, &mut last).is_ok());
            assert_eq!(last, 5);
        }
    }

    #[test]
    fn test_replay_protection_all_message_types() {
        let chunk = |seq| ProtocolMessage::FileChunk {
            chunk: vec![1, 2, 3],
            seq,
        };
        let meta = ProtocolMessage::FileMeta {
            filename: "test.txt".to_string(),
            size: 1024,
            seq: 2,
        };
        let mut last = 0u64;
        assert!(validate_message_sequence(&chunk(1), &mut last).is_ok());
        assert_eq!(last, 1);
        assert!(validate_message_sequence(&chunk(1), &mut last).is_err());
        for (msg, seq) in [
            (meta, 2),
            (ProtocolMessage::Ping { seq: 3 }, 3),
            (ProtocolMessage::TypingStart { seq: 4 }, 4),
        ] {
            assert!(validate_message_sequence(&msg, &mut last).is_ok());
            assert_eq!(last, seq);
        }
    }

    /// A PEM with the victim's key in a preamble and the attacker's in the block,
    /// signed by the attacker, must not verify (it would fingerprint as the victim).
    #[test]
    fn an_identity_proof_with_a_smuggled_second_key_is_rejected() {
        let victim = RsaPublicKey::from(&generate_rsa_keypair(2048).unwrap());
        let attacker_priv = generate_rsa_keypair(2048).unwrap();
        let crafted = format!(
            "x{}\n{}",
            pem_encode_public(&victim).unwrap().trim_end(),
            pem_of(&attacker_priv)
        );
        let ephemeral = [7u8; 32];
        for scheme in [SignatureScheme::RsaPss, SignatureScheme::Ed25519] {
            let sig = build_identity_signature(scheme, &attacker_priv, &ephemeral).unwrap();
            let proof = identity_proof(crafted.clone(), sig, scheme);
            assert!(
                verify_identity_proof(&proof, &ephemeral).is_err(),
                "{scheme} proof with two keys must not verify"
            );
        }
    }
}
