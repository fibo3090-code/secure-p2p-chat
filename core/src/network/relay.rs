//! Relay rendezvous with TCP hole punching and a bridged fallback.
//!
//! A host registers a random token; a joiner presents the same token. When both
//! sides are punch-capable, the server sends each of them the other's public
//! endpoint (observed from the control connection) plus self-reported LAN
//! candidates, and the peers attempt a direct TCP hole punch (`punch.rs`). Only
//! if that fails — hostile NATs, CGNAT, filtered networks — does the server
//! fall back to what it always did: pair the two control connections and
//! forward the (already end-to-end encrypted) session bytes between them.
//!
//! Wire compatibility: the control protocol is bincode enums, so new variants
//! are only appended. A legacy peer registering with the old `Host`/`Join`
//! variants simply gets the bridged path; a new client talking to a legacy
//! server detects the dropped connection and re-registers in legacy mode.
//!
//! Trust model is unchanged: the token (and the hello tag derived from it) is
//! rendezvous pairing, not authentication. Whether punched or bridged, the
//! socket carries the same v3 handshake — ECDH, identity proof, TOFU — and the
//! relay never holds key material.

use anyhow::{anyhow, bail, Result};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, Mutex};

use crate::core::{recv_packet, send_packet};
use crate::network::punch::{attempt_punch, parse_candidates, PunchRole, MAX_PUNCH_CANDIDATES};
use crate::network::ratelimit::RateLimiter;

const RELAY_WAIT_TIMEOUT: Duration = Duration::from_secs(300);
const RELAY_TOKEN_BYTES: usize = 16;
/// How long a freshly accepted connection has to send its `Host`/`Join` frame.
///
/// Without this a peer could connect and simply never speak: `recv_relay_message`
/// would await forever, holding a task and a socket for the life of the process.
/// That is the cheapest possible denial of service against a public rendezvous.
const RELAY_HELLO_TIMEOUT: Duration = Duration::from_secs(15);
/// Most rendezvous slots the server will hold at once. Each pending entry parks
/// a task and a socket for up to [`RELAY_WAIT_TIMEOUT`], so an uncapped map is
/// an uncapped memory cost paid by anyone who can open connections.
const MAX_PENDING_RENDEZVOUS: usize = 1024;
/// Connections one address may open to the relay within [`RELAY_RATE_WINDOW`].
/// More generous than the community server's: a legitimate client reconnects
/// per rendezvous attempt, and punch retries reuse the control connection.
const RELAY_MAX_CONNECTIONS_PER_IP: usize = 20;
const RELAY_RATE_WINDOW: Duration = Duration::from_secs(30);
/// How long the server waits for both peers' punch outcomes before assuming
/// the worst and bridging. Must exceed the client-side punch budget plus the
/// selection grace, with margin for slow links.
const PUNCH_REPORT_TIMEOUT: Duration = Duration::from_secs(20);
/// Longest candidate string the server will forward (abuse guard).
const MAX_CANDIDATE_CHARS: usize = 64;

/// Setting this environment variable (to any non-empty value) disables hole
/// punching and forces the bridged relay path — useful for debugging and for
/// networks where punch traffic is unwelcome.
pub const NO_HOLEPUNCH_ENV: &str = "P2PEM_NO_HOLEPUNCH";

#[derive(Debug, Serialize, Deserialize)]
enum RelayRequest {
    /// Legacy (pre-punch) registrations: always bridged. Kept first — bincode
    /// encodes the variant index, so existing peers stay wire-compatible.
    Host {
        token: String,
    },
    Join {
        token: String,
    },
    /// Punch-capable registrations (appended variants).
    HostV2 {
        token: String,
        punch: PunchCaps,
    },
    JoinV2 {
        token: String,
        punch: PunchCaps,
    },
}

/// What a punch-capable client tells the rendezvous about itself. The public
/// endpoint is *not* self-reported — the server observes it from the control
/// connection's source address, which is exactly the NAT mapping the punch
/// will reuse.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PunchCaps {
    /// LAN candidates as `ip:port`, already carrying the punch source port.
    local_addrs: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
enum RelayResponse {
    Waiting,
    Paired,
    Error(String),
    /// Both sides are punch-capable: attempt a direct connection to these
    /// candidates, then report a [`PunchOutcome`]. Appended variant.
    PunchStart {
        peer_public: String,
        peer_locals: Vec<String>,
    },
}

/// Client → server report closing a punch round.
#[derive(Debug, Serialize, Deserialize)]
struct PunchOutcome {
    success: bool,
}

struct PendingRelay {
    created_at: Instant,
    rendezvous_tx: oneshot::Sender<(TcpStream, Option<PunchCaps>)>,
}

type PendingMap = Arc<Mutex<HashMap<String, PendingRelay>>>;

/// How the rendezvous ended up connecting the two peers.
#[derive(Debug)]
enum RelayStream {
    /// Hole punch succeeded: a direct socket to the peer; the relay is out of
    /// the path entirely.
    Direct(TcpStream),
    /// Bridged through the relay server (legacy peers, punch failure, or
    /// punching disabled).
    Relayed(TcpStream),
}

pub fn generate_relay_token() -> String {
    let mut bytes = [0u8; RELAY_TOKEN_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn punching_disabled_by_env() -> bool {
    std::env::var_os(NO_HOLEPUNCH_ENV).is_some_and(|v| !v.is_empty())
}

pub async fn run_relay_server(port: u16) -> Result<()> {
    run_relay_server_with_wait_timeout(port, RELAY_WAIT_TIMEOUT).await
}

/// [`run_relay_server`] with a caller-chosen rendezvous lifetime.
///
/// The only reason this exists is testability: [`RELAY_WAIT_TIMEOUT`] is five
/// minutes, and the expiry behaviour it governs — a stale token being refused,
/// and an abandoned slot being swept so it cannot lock the table — is worth a
/// test that does not take five minutes to run. Production always uses the
/// constant.
pub async fn run_relay_server_with_wait_timeout(port: u16, wait_timeout: Duration) -> Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
    // Same hardening the community server's accept loop has: an unlimited
    // connection rate against a public rendezvous is an unlimited task and
    // memory cost, and every accepted socket parks a slot until it times out.
    let rate_limiter = Arc::new(Mutex::new(RateLimiter::with_limits(
        RELAY_MAX_CONNECTIONS_PER_IP,
        RELAY_RATE_WINDOW,
    )));
    tracing::info!("Relay server listening on port {}", port);

    loop {
        // A failed `accept` is almost always transient (the peer went away
        // between the SYN and our accept, or we momentarily ran out of file
        // descriptors). Propagating it with `?` would take the whole rendezvous
        // down for the life of the process over a single hiccup.
        let (stream, addr) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(e) => {
                tracing::warn!(error = %e, "relay accept failed; continuing to listen");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        if rate_limiter.lock().await.check(addr.ip()) {
            tracing::warn!(%addr, "refusing relay connection: rate limit exceeded");
            continue;
        }
        tracing::info!("Relay client connected from {}", addr);
        let pending = pending.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_relay_connection(stream, pending, wait_timeout).await {
                tracing::warn!("Relay connection failed: {}", e);
            }
        });
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn run_host_session_via_relay(
    relay_server: &str,
    token: &str,
    privkey: rsa::RsaPrivateKey,
    to_app_tx: tokio::sync::mpsc::UnboundedSender<crate::types::SessionEvent>,
    from_app_rx: tokio::sync::mpsc::UnboundedReceiver<crate::core::ProtocolMessage>,
    file_rx: tokio::sync::mpsc::Receiver<crate::core::ProtocolMessage>,
    confirm_rx: tokio::sync::mpsc::UnboundedReceiver<bool>,
    chat_id: uuid::Uuid,
) -> Result<()> {
    let (mut stream, peer_label) = label_stream(
        connect_to_relay(relay_server, token, true).await?,
        relay_server,
    );
    crate::network::session::run_host_session_over_stream(
        &mut stream,
        peer_label,
        privkey,
        to_app_tx,
        from_app_rx,
        file_rx,
        confirm_rx,
        chat_id,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn run_client_session_via_relay(
    relay_server: &str,
    token: &str,
    privkey: rsa::RsaPrivateKey,
    to_app_tx: tokio::sync::mpsc::UnboundedSender<crate::types::SessionEvent>,
    from_app_rx: tokio::sync::mpsc::UnboundedReceiver<crate::core::ProtocolMessage>,
    file_rx: tokio::sync::mpsc::Receiver<crate::core::ProtocolMessage>,
    confirm_rx: tokio::sync::mpsc::UnboundedReceiver<bool>,
    chat_id: uuid::Uuid,
) -> Result<()> {
    let (mut stream, peer_label) = label_stream(
        connect_to_relay(relay_server, token, false).await?,
        relay_server,
    );
    crate::network::session::run_client_session_over_stream(
        &mut stream,
        peer_label,
        privkey,
        to_app_tx,
        from_app_rx,
        file_rx,
        confirm_rx,
        chat_id,
        None,
    )
    .await
}

/// `p2p:<addr>` for a punched direct socket, `relay:<server>` when bridged.
/// UIs key off these prefixes for the transport badge and toasts.
fn label_stream(stream: RelayStream, relay_server: &str) -> (TcpStream, String) {
    match stream {
        RelayStream::Direct(s) => {
            let label = s
                .peer_addr()
                .map(|a| format!("p2p:{a}"))
                .unwrap_or_else(|_| "p2p:unknown".to_string());
            (s, label)
        }
        RelayStream::Relayed(s) => (s, format!("relay:{relay_server}")),
    }
}

async fn connect_to_relay(relay_server: &str, token: &str, as_host: bool) -> Result<RelayStream> {
    connect_to_relay_with(relay_server, token, as_host, !punching_disabled_by_env()).await
}

/// Errors from one registration attempt, split by whether re-registering in
/// legacy mode could help.
enum AttemptError {
    /// The server dropped us before answering at all — the signature of a
    /// pre-punch relay that could not parse the new registration variant.
    LegacyServer(anyhow::Error),
    Fatal(anyhow::Error),
}

impl AttemptError {
    fn into_inner(self) -> anyhow::Error {
        match self {
            AttemptError::LegacyServer(e) | AttemptError::Fatal(e) => e,
        }
    }
}

async fn connect_to_relay_with(
    relay_server: &str,
    token: &str,
    as_host: bool,
    try_punch: bool,
) -> Result<RelayStream> {
    validate_token(token)?;
    let (host, port) = crate::util::parse_host_port(relay_server, Some(crate::PORT_DEFAULT))?;

    match relay_attempt(&host, port, token, as_host, try_punch).await {
        Ok(stream) => Ok(stream),
        Err(AttemptError::LegacyServer(e)) if try_punch => {
            tracing::info!(
                error = %e,
                "relay dropped the punch-capable registration; retrying in legacy mode"
            );
            relay_attempt(&host, port, token, as_host, false)
                .await
                .map_err(AttemptError::into_inner)
        }
        Err(e) => Err(e.into_inner()),
    }
}

async fn relay_attempt(
    host: &str,
    port: u16,
    token: &str,
    as_host: bool,
    try_punch: bool,
) -> Result<RelayStream, AttemptError> {
    // Reuse-enabled dial: the punch phase re-binds this connection's local
    // port, which requires the reuse flags to be set on it from the start.
    let mut stream = crate::network::punch::connect_reusable(host, port)
        .await
        .map_err(AttemptError::Fatal)?;
    let local_port = stream
        .local_addr()
        .map(|a| a.port())
        .map_err(|e| AttemptError::Fatal(e.into()))?;

    let request = build_request(token, as_host, try_punch, local_port);
    send_relay_message(&mut stream, &request)
        .await
        .map_err(AttemptError::Fatal)?;

    let mut got_response = false;
    loop {
        let response = match recv_relay_message::<RelayResponse>(&mut stream).await {
            Ok(response) => {
                got_response = true;
                response
            }
            // A drop before any response means the server never understood the
            // registration; anything later is a real failure.
            Err(e) if !got_response && try_punch => return Err(AttemptError::LegacyServer(e)),
            Err(e) => return Err(AttemptError::Fatal(e)),
        };
        match response {
            RelayResponse::Waiting => continue,
            RelayResponse::Paired => {
                tracing::info!("Relay session paired (bridged) via {}:{}", host, port);
                return Ok(RelayStream::Relayed(stream));
            }
            RelayResponse::Error(message) => {
                return Err(AttemptError::Fatal(anyhow!(
                    "Relay refused connection: {}",
                    message
                )))
            }
            RelayResponse::PunchStart {
                peer_public,
                peer_locals,
            } => {
                let role = if as_host {
                    PunchRole::Host
                } else {
                    PunchRole::Joiner
                };
                let candidates = parse_candidates(&peer_public, &peer_locals);
                match attempt_punch(role, local_port, &candidates, token).await {
                    Ok(direct) => {
                        // Best-effort: the server only uses this to skip the
                        // bridge; the direct socket is already confirmed.
                        let _ =
                            send_relay_message(&mut stream, &PunchOutcome { success: true }).await;
                        tracing::info!(
                            peer = %direct.peer_addr().map(|a| a.to_string()).unwrap_or_default(),
                            "hole punch succeeded; relay is out of the path"
                        );
                        return Ok(RelayStream::Direct(direct));
                    }
                    Err(e) => {
                        tracing::info!(error = %e, "hole punch failed; falling back to bridged relay");
                        send_relay_message(&mut stream, &PunchOutcome { success: false })
                            .await
                            .map_err(AttemptError::Fatal)?;
                        // The server answers with Paired and bridges.
                    }
                }
            }
        }
    }
}

fn build_request(token: &str, as_host: bool, try_punch: bool, local_port: u16) -> RelayRequest {
    if !try_punch {
        return if as_host {
            RelayRequest::Host {
                token: token.to_string(),
            }
        } else {
            RelayRequest::Join {
                token: token.to_string(),
            }
        };
    }
    // LAN candidate: lets two peers behind the *same* NAT punch directly even
    // when the router doesn't hairpin its own external address.
    let mut local_addrs = Vec::new();
    if let Some(ip) = crate::util::primary_local_ipv4() {
        local_addrs.push(crate::util::format_host_port(&ip, local_port));
    }
    let punch = PunchCaps { local_addrs };
    if as_host {
        RelayRequest::HostV2 {
            token: token.to_string(),
            punch,
        }
    } else {
        RelayRequest::JoinV2 {
            token: token.to_string(),
            punch,
        }
    }
}

async fn handle_relay_connection(
    mut stream: TcpStream,
    pending: PendingMap,
    wait_timeout: Duration,
) -> Result<()> {
    // Bounded: a client that connects and then says nothing must not hold this
    // task and socket open indefinitely.
    let hello = tokio::time::timeout(
        RELAY_HELLO_TIMEOUT,
        recv_relay_message::<RelayRequest>(&mut stream),
    )
    .await
    .map_err(|_| {
        anyhow!(
            "relay client sent no request within {}s",
            RELAY_HELLO_TIMEOUT.as_secs()
        )
    })??;
    match hello {
        RelayRequest::Host { token } => host_flow(stream, pending, token, None, wait_timeout).await,
        RelayRequest::HostV2 { token, punch } => {
            host_flow(
                stream,
                pending,
                token,
                Some(sanitize_caps(punch)),
                wait_timeout,
            )
            .await
        }
        RelayRequest::Join { token } => join_flow(stream, pending, token, None, wait_timeout).await,
        RelayRequest::JoinV2 { token, punch } => {
            join_flow(
                stream,
                pending,
                token,
                Some(sanitize_caps(punch)),
                wait_timeout,
            )
            .await
        }
    }
}

/// Bound peer-supplied candidate lists before forwarding them: the server
/// never parses these, so cap count and length instead.
fn sanitize_caps(mut caps: PunchCaps) -> PunchCaps {
    caps.local_addrs.retain(|a| a.len() <= MAX_CANDIDATE_CHARS);
    caps.local_addrs.truncate(MAX_PUNCH_CANDIDATES);
    caps
}

async fn host_flow(
    mut stream: TcpStream,
    pending: PendingMap,
    token: String,
    host_caps: Option<PunchCaps>,
    wait_timeout: Duration,
) -> Result<()> {
    validate_token(&token)?;

    let (rendezvous_tx, rendezvous_rx) = oneshot::channel();
    {
        let mut guard = pending.lock().await;
        // Expired slots are only removed by their own timeout task, so sweep
        // them here too before deciding the map is full — otherwise a burst of
        // abandoned hosts would lock out legitimate ones for five minutes.
        guard.retain(|_, entry| entry.created_at.elapsed() <= wait_timeout);
        if guard.len() >= MAX_PENDING_RENDEZVOUS {
            send_relay_message(
                &mut stream,
                &RelayResponse::Error("Relay is at capacity, try again shortly".to_string()),
            )
            .await?;
            bail!("relay rendezvous table is full ({MAX_PENDING_RENDEZVOUS} slots)");
        }
        if guard.contains_key(&token) {
            send_relay_message(
                &mut stream,
                &RelayResponse::Error("Relay token already in use".to_string()),
            )
            .await?;
            bail!("Relay token already in use");
        }
        guard.insert(
            token.clone(),
            PendingRelay {
                created_at: Instant::now(),
                rendezvous_tx,
            },
        );
    }

    send_relay_message(&mut stream, &RelayResponse::Waiting).await?;

    let (mut peer_stream, joiner_caps) =
        match tokio::time::timeout(wait_timeout, rendezvous_rx).await {
            Ok(Ok(paired)) => paired,
            Ok(Err(_)) => bail!("Relay joiner dropped before pairing"),
            Err(_) => {
                let mut guard = pending.lock().await;
                guard.remove(&token);
                send_relay_message(
                    &mut stream,
                    &RelayResponse::Error("Relay wait timed out".to_string()),
                )
                .await?;
                bail!("Relay wait timed out");
            }
        };

    // Punch phase: only when both sides registered as punch-capable. Any
    // hiccup degrades to the bridge — never to a failed pairing.
    if let (Some(host_caps), Some(joiner_caps)) = (host_caps, joiner_caps) {
        match coordinate_punch(&mut stream, &mut peer_stream, host_caps, joiner_caps).await {
            Ok(true) => {
                tracing::info!("Relay peers hole punched a direct connection");
                return Ok(());
            }
            Ok(false) => tracing::debug!("Hole punch failed on at least one side; bridging"),
            Err(e) => tracing::debug!(error = %e, "Punch coordination failed; bridging"),
        }
    }

    send_relay_message(&mut stream, &RelayResponse::Paired).await?;
    send_relay_message(&mut peer_stream, &RelayResponse::Paired).await?;
    copy_bidirectional(&mut stream, &mut peer_stream).await?;
    Ok(())
}

async fn join_flow(
    mut stream: TcpStream,
    pending: PendingMap,
    token: String,
    joiner_caps: Option<PunchCaps>,
    wait_timeout: Duration,
) -> Result<()> {
    validate_token(&token)?;
    let pending_entry = {
        let mut guard = pending.lock().await;
        match guard.remove(&token) {
            Some(entry) => entry,
            None => {
                send_relay_message(
                    &mut stream,
                    &RelayResponse::Error("Unknown relay token".to_string()),
                )
                .await?;
                bail!("Unknown relay token");
            }
        }
    };

    if pending_entry.created_at.elapsed() > wait_timeout {
        send_relay_message(
            &mut stream,
            &RelayResponse::Error("Relay token expired".to_string()),
        )
        .await?;
        bail!("Relay token expired");
    }

    // Acknowledge the join before handing the socket to the host. This is what
    // lets a punch-capable joiner tell a *new* relay (that later drops the
    // connection because the host vanished at the handoff) apart from a *legacy*
    // relay that closed because it could not parse the `JoinV2` frame: only the
    // legacy server stays silent, so the joiner falls back to legacy mode solely
    // in that genuine case. The client treats `Waiting` as "keep reading".
    send_relay_message(&mut stream, &RelayResponse::Waiting).await?;

    pending_entry
        .rendezvous_tx
        .send((stream, joiner_caps))
        .map_err(|_| anyhow!("Relay host is no longer available"))?;
    Ok(())
}

/// Hand each peer the other's endpoints, then wait for both outcome reports.
/// `Ok(true)` means both sides confirmed a direct connection and the control
/// streams can be dropped; anything else means "bridge them".
async fn coordinate_punch(
    host: &mut TcpStream,
    joiner: &mut TcpStream,
    host_caps: PunchCaps,
    joiner_caps: PunchCaps,
) -> Result<bool> {
    let host_public = host.peer_addr()?.to_string();
    let joiner_public = joiner.peer_addr()?.to_string();

    send_relay_message(
        host,
        &RelayResponse::PunchStart {
            peer_public: joiner_public,
            peer_locals: joiner_caps.local_addrs,
        },
    )
    .await?;
    send_relay_message(
        joiner,
        &RelayResponse::PunchStart {
            peer_public: host_public,
            peer_locals: host_caps.local_addrs,
        },
    )
    .await?;

    let outcomes = tokio::time::timeout(PUNCH_REPORT_TIMEOUT, async {
        tokio::try_join!(
            recv_relay_message::<PunchOutcome>(host),
            recv_relay_message::<PunchOutcome>(joiner),
        )
    })
    .await;
    match outcomes {
        Ok(Ok((host_outcome, joiner_outcome))) => {
            Ok(host_outcome.success && joiner_outcome.success)
        }
        Ok(Err(e)) => Err(e.context("a peer vanished during the punch phase")),
        Err(_) => Err(anyhow!("timed out waiting for punch outcome reports")),
    }
}

fn validate_token(token: &str) -> Result<()> {
    if token.len() != RELAY_TOKEN_BYTES * 2 || !token.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!(
            "Relay token must be {} hex characters",
            RELAY_TOKEN_BYTES * 2
        );
    }
    Ok(())
}

async fn send_relay_message<T: Serialize>(stream: &mut TcpStream, value: &T) -> Result<()> {
    let payload = bincode::serialize(value)?;
    send_packet(stream, &payload).await?;
    Ok(())
}

async fn recv_relay_message<T: for<'de> Deserialize<'de>>(stream: &mut TcpStream) -> Result<T> {
    let payload = recv_packet(stream).await?;
    Ok(bincode::deserialize(&payload)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Reserve an ephemeral loopback port, then free it for the server to bind.
    fn free_port() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
        listener.local_addr().expect("local addr").port()
    }

    async fn start_relay() -> String {
        let port = free_port();
        tokio::spawn(async move {
            let _ = run_relay_server(port).await;
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        format!("127.0.0.1:{port}")
    }

    async fn exchange_ping_pong(host: RelayStream, join: RelayStream) {
        let (mut host_stream, _) = label_stream(host, "test");
        let (mut join_stream, _) = label_stream(join, "test");
        host_stream.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        join_stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
        join_stream.write_all(b"pong").await.unwrap();
        host_stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"pong");
    }

    #[tokio::test]
    async fn legacy_peers_pair_bridged_and_forward_bytes() {
        let relay_addr = start_relay().await;
        let token = generate_relay_token();

        let host_task = tokio::spawn({
            let relay_addr = relay_addr.clone();
            let token = token.clone();
            async move { connect_to_relay_with(&relay_addr, &token, true, false).await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let join_task = tokio::spawn({
            let relay_addr = relay_addr.clone();
            let token = token.clone();
            async move { connect_to_relay_with(&relay_addr, &token, false, false).await }
        });

        let host = host_task.await.unwrap().unwrap();
        let join = join_task.await.unwrap().unwrap();
        assert!(matches!(host, RelayStream::Relayed(_)));
        assert!(matches!(join, RelayStream::Relayed(_)));
        exchange_ping_pong(host, join).await;
    }

    #[tokio::test]
    async fn punch_capable_peers_connect_directly() {
        let relay_addr = start_relay().await;
        let token = generate_relay_token();

        let host_task = tokio::spawn({
            let relay_addr = relay_addr.clone();
            let token = token.clone();
            async move { connect_to_relay_with(&relay_addr, &token, true, true).await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let join_task = tokio::spawn({
            let relay_addr = relay_addr.clone();
            let token = token.clone();
            async move { connect_to_relay_with(&relay_addr, &token, false, true).await }
        });

        let host = host_task.await.unwrap().unwrap();
        let join = join_task.await.unwrap().unwrap();
        assert!(matches!(host, RelayStream::Direct(_)));
        assert!(matches!(join, RelayStream::Direct(_)));
        exchange_ping_pong(host, join).await;
    }

    #[tokio::test]
    async fn mixed_capability_peers_fall_back_to_bridged() {
        let relay_addr = start_relay().await;
        let token = generate_relay_token();

        // Punch-capable host, legacy joiner: the server must not start a punch
        // phase the joiner cannot understand.
        let host_task = tokio::spawn({
            let relay_addr = relay_addr.clone();
            let token = token.clone();
            async move { connect_to_relay_with(&relay_addr, &token, true, true).await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let join_task = tokio::spawn({
            let relay_addr = relay_addr.clone();
            let token = token.clone();
            async move { connect_to_relay_with(&relay_addr, &token, false, false).await }
        });

        let host = host_task.await.unwrap().unwrap();
        let join = join_task.await.unwrap().unwrap();
        assert!(matches!(host, RelayStream::Relayed(_)));
        assert!(matches!(join, RelayStream::Relayed(_)));
        exchange_ping_pong(host, join).await;
    }

    /// Start a relay whose rendezvous slots expire almost immediately, so the
    /// expiry paths can be exercised without a five-minute test.
    async fn start_relay_with_wait(wait: Duration) -> String {
        let port = free_port();
        tokio::spawn(async move {
            let _ = run_relay_server_with_wait_timeout(port, wait).await;
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        format!("127.0.0.1:{port}")
    }

    /// A joiner presenting a token nobody is hosting must be told so, promptly.
    /// Answering nothing would leave the client hanging on a typo.
    #[tokio::test]
    async fn an_unknown_token_is_refused() {
        let relay_addr = start_relay().await;
        let err = connect_to_relay_with(&relay_addr, &generate_relay_token(), false, true)
            .await
            .expect_err("joining a token nobody hosted must fail");
        assert!(
            err.to_string().contains("Unknown relay token"),
            "the error should say what was wrong: {err}"
        );
    }

    /// Two hosts cannot register the same token: the second would otherwise
    /// replace the first, silently stealing whoever joins next.
    #[tokio::test]
    async fn a_token_cannot_be_hosted_twice_at_once() {
        let relay_addr = start_relay().await;
        let token = generate_relay_token();

        let first = tokio::spawn({
            let relay_addr = relay_addr.clone();
            let token = token.clone();
            async move { connect_to_relay_with(&relay_addr, &token, true, true).await }
        });
        tokio::time::sleep(Duration::from_millis(150)).await;

        let err = connect_to_relay_with(&relay_addr, &token, true, true)
            .await
            .expect_err("the second host must be refused");
        assert!(
            err.to_string().contains("already in use"),
            "the error should say why: {err}"
        );

        first.abort();
    }

    /// A stale token must not pair. The slot is swept and the joiner is refused,
    /// rather than being handed a host that gave up long ago.
    #[tokio::test]
    async fn an_expired_token_no_longer_pairs() {
        let relay_addr = start_relay_with_wait(Duration::from_millis(300)).await;
        let token = generate_relay_token();

        let host = tokio::spawn({
            let relay_addr = relay_addr.clone();
            let token = token.clone();
            async move { connect_to_relay_with(&relay_addr, &token, true, true).await }
        });
        // Let the host register, then outlive its slot.
        tokio::time::sleep(Duration::from_millis(900)).await;

        let err = connect_to_relay_with(&relay_addr, &token, false, true)
            .await
            .expect_err("an expired token must not pair");
        let msg = err.to_string();
        assert!(
            msg.contains("expired") || msg.contains("Unknown relay token"),
            "the joiner should be refused, got: {msg}"
        );

        // And the host's own wait ends rather than parking forever.
        let host_result = tokio::time::timeout(Duration::from_secs(5), host)
            .await
            .expect("the host's wait must time out on its own");
        assert!(host_result.expect("host task").is_err());
    }

    /// The relay keeps serving after a pairing ends — including the same token
    /// again, since it is released once the two peers are joined. A rendezvous
    /// that leaked its slot would refuse the reconnect that follows every
    /// dropped connection.
    #[tokio::test]
    async fn a_token_can_be_reused_once_its_pairing_is_over() {
        let relay_addr = start_relay().await;
        let token = generate_relay_token();

        for round in 0..2 {
            let host_task = tokio::spawn({
                let relay_addr = relay_addr.clone();
                let token = token.clone();
                async move { connect_to_relay_with(&relay_addr, &token, true, false).await }
            });
            tokio::time::sleep(Duration::from_millis(100)).await;
            let join_task = tokio::spawn({
                let relay_addr = relay_addr.clone();
                let token = token.clone();
                async move { connect_to_relay_with(&relay_addr, &token, false, false).await }
            });

            let host = host_task
                .await
                .unwrap()
                .unwrap_or_else(|e| panic!("round {round} host: {e}"));
            let join = join_task
                .await
                .unwrap()
                .unwrap_or_else(|e| panic!("round {round} joiner: {e}"));
            exchange_ping_pong(host, join).await;
            // Dropping both ends releases the bridge before the next round.
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Several pairings share one relay. Each must reach its own partner: a
    /// rendezvous that mixed them up would connect two strangers, and the v3
    /// handshake would be the only thing standing between them.
    #[tokio::test]
    async fn concurrent_pairings_do_not_cross() {
        let relay_addr = start_relay().await;
        const PAIRS: usize = 4;

        let mut tasks = Vec::new();
        for i in 0..PAIRS {
            let relay_addr = relay_addr.clone();
            tasks.push(tokio::spawn(async move {
                let token = generate_relay_token();
                let host_task = tokio::spawn({
                    let relay_addr = relay_addr.clone();
                    let token = token.clone();
                    async move { connect_to_relay_with(&relay_addr, &token, true, false).await }
                });
                tokio::time::sleep(Duration::from_millis(100)).await;
                let join = connect_to_relay_with(&relay_addr, &token, false, false)
                    .await
                    .expect("joiner pairs");
                let host = host_task.await.unwrap().expect("host pairs");

                // A per-pair payload: if the relay crossed two rendezvous, the
                // bytes would arrive on the wrong socket.
                let (mut host_stream, _) = label_stream(host, "test");
                let (mut join_stream, _) = label_stream(join, "test");
                let payload = format!("pair-{i}");
                host_stream.write_all(payload.as_bytes()).await.unwrap();
                let mut buf = vec![0u8; payload.len()];
                join_stream.read_exact(&mut buf).await.unwrap();
                assert_eq!(
                    String::from_utf8(buf).unwrap(),
                    payload,
                    "pair {i} received another pair's bytes"
                );
            }));
        }
        for t in tasks {
            t.await.expect("pair task");
        }
    }

    /// Emulates the pre-punch relay server: only the two legacy request
    /// variants exist, an unknown variant is a deserialize error, and the
    /// connection is dropped without a response — exactly what a new client
    /// sees when talking to an old deployment.
    mod legacy_server {
        use super::*;

        #[derive(Serialize, Deserialize)]
        enum LegacyRequest {
            Host { token: String },
            Join { token: String },
        }

        #[derive(Serialize, Deserialize)]
        enum LegacyResponse {
            Waiting,
            Paired,
            Error(String),
        }

        pub async fn run(port: u16) -> Result<()> {
            let listener = TcpListener::bind(("127.0.0.1", port)).await?;
            let pending: Arc<Mutex<HashMap<String, oneshot::Sender<TcpStream>>>> =
                Arc::new(Mutex::new(HashMap::new()));
            loop {
                let (mut stream, _) = listener.accept().await?;
                let pending = pending.clone();
                tokio::spawn(async move {
                    let payload = recv_packet(&mut stream).await?;
                    // Old server: bincode error on a V2 variant → connection drop.
                    let request: LegacyRequest = bincode::deserialize(&payload)?;
                    match request {
                        LegacyRequest::Host { token } => {
                            let (tx, rx) = oneshot::channel();
                            pending.lock().await.insert(token, tx);
                            let waiting = bincode::serialize(&LegacyResponse::Waiting)?;
                            send_packet(&mut stream, &waiting).await?;
                            let mut peer: TcpStream = rx.await?;
                            let paired = bincode::serialize(&LegacyResponse::Paired)?;
                            send_packet(&mut stream, &paired).await?;
                            send_packet(&mut peer, &paired).await?;
                            copy_bidirectional(&mut stream, &mut peer).await?;
                        }
                        LegacyRequest::Join { token } => {
                            if let Some(tx) = pending.lock().await.remove(&token) {
                                let _ = tx.send(stream);
                            }
                        }
                    }
                    anyhow::Ok(())
                });
            }
        }
    }

    #[tokio::test]
    async fn new_clients_fall_back_against_a_legacy_server() {
        let port = free_port();
        tokio::spawn(async move {
            let _ = legacy_server::run(port).await;
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        let relay_addr = format!("127.0.0.1:{port}");
        let token = generate_relay_token();

        // Punch-capable registration gets dropped by the old server; the
        // client must silently retry in legacy mode and still pair.
        let host_task = tokio::spawn({
            let relay_addr = relay_addr.clone();
            let token = token.clone();
            async move { connect_to_relay_with(&relay_addr, &token, true, true).await }
        });
        tokio::time::sleep(Duration::from_millis(150)).await;
        let join_task = tokio::spawn({
            let relay_addr = relay_addr.clone();
            let token = token.clone();
            async move { connect_to_relay_with(&relay_addr, &token, false, true).await }
        });

        let host = host_task.await.unwrap().expect("host legacy fallback");
        let join = join_task.await.unwrap().expect("joiner legacy fallback");
        assert!(matches!(host, RelayStream::Relayed(_)));
        assert!(matches!(join, RelayStream::Relayed(_)));
        exchange_ping_pong(host, join).await;
    }
}
