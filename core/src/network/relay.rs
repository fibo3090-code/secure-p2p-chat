//! Relay rendezvous with TCP hole punching and a bridged fallback.
//!
//! A host registers a random token; a joiner presents the same token. When both sides
//! are punch-capable, the server sends each the other's public endpoint (observed from
//! the control connection) plus self-reported LAN candidates, and the peers attempt a
//! TCP hole punch (`punch.rs`). Only if that fails does the server pair the two control
//! connections and forward the (end-to-end encrypted) session bytes.
//!
//! Wire compatibility: the control protocol is bincode enums with variants only ever
//! appended. A legacy peer gets the bridged path; a new client facing a legacy server
//! sees the dropped connection and re-registers in legacy mode.
//!
//! The token is rendezvous pairing, not authentication: either way the socket carries
//! the full handshake and TOFU, and the relay never holds key material.

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
/// How long a freshly accepted connection has to send its `Host`/`Join` frame, so a
/// peer that connects and never speaks cannot hold a task and socket forever.
const RELAY_HELLO_TIMEOUT: Duration = Duration::from_secs(15);
/// Most rendezvous slots held at once; each parks a task and a socket for up to
/// [`RELAY_WAIT_TIMEOUT`].
const MAX_PENDING_RENDEZVOUS: usize = 1024;
/// Connections one address may open within [`RELAY_RATE_WINDOW`]; generous because a
/// client reconnects per rendezvous attempt.
const RELAY_MAX_CONNECTIONS_PER_IP: usize = 20;
const RELAY_RATE_WINDOW: Duration = Duration::from_secs(30);
/// How long the server waits for both punch outcomes before bridging. Must exceed the
/// client-side punch budget plus the selection grace.
const PUNCH_REPORT_TIMEOUT: Duration = Duration::from_secs(20);
/// Longest candidate string the server will forward (abuse guard).
const MAX_CANDIDATE_CHARS: usize = 64;

/// Set (non-empty) to disable hole punching and force the bridged relay path.
pub const NO_HOLEPUNCH_ENV: &str = "P2PEM_NO_HOLEPUNCH";

#[derive(Debug, Serialize, Deserialize)]
enum RelayRequest {
    /// Legacy (pre-punch) registrations: always bridged. Kept first, since bincode
    /// encodes the variant index.
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

/// What a punch-capable client reports about itself. The public endpoint is not
/// self-reported: the server observes it from the control connection, which is the
/// NAT mapping the punch reuses.
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
    /// Both sides are punch-capable: try these candidates, then report a
    /// [`PunchOutcome`]. Appended variant.
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
    /// Hole punch succeeded: a direct socket; the relay is out of the path.
    Direct(TcpStream),
    /// Bridged through the relay (legacy peers, punch failure, or punching disabled).
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

/// [`run_relay_server`] with a caller-chosen rendezvous lifetime, so expiry is testable
/// without a five-minute wait. Production always uses [`RELAY_WAIT_TIMEOUT`].
pub async fn run_relay_server_with_wait_timeout(port: u16, wait_timeout: Duration) -> Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
    // Same per-IP limit as the community server: every accepted socket parks a slot.
    let rate_limiter = Arc::new(Mutex::new(RateLimiter::with_limits(
        RELAY_MAX_CONNECTIONS_PER_IP,
        RELAY_RATE_WINDOW,
    )));
    tracing::info!("Relay server listening on port {}", port);

    loop {
        // A failed accept is almost always transient (EMFILE, peer gone); `?` would
        // take the rendezvous down for the life of the process.
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

/// Errors from one registration attempt, split by whether legacy mode could help.
enum AttemptError {
    /// Dropped before any answer: a pre-punch relay that could not parse the request.
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
    // Reuse-enabled dial: the punch phase re-binds this connection's local port.
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
                        // Best-effort: the direct socket is already confirmed.
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
    let token = token.to_string();
    if !try_punch {
        return if as_host {
            RelayRequest::Host { token }
        } else {
            RelayRequest::Join { token }
        };
    }
    // LAN candidate: lets two peers behind the same NAT punch even without hairpinning.
    let mut local_addrs = Vec::new();
    if let Some(ip) = crate::util::primary_local_ipv4() {
        local_addrs.push(crate::util::format_host_port(&ip, local_port));
    }
    let punch = PunchCaps { local_addrs };
    if as_host {
        RelayRequest::HostV2 { token, punch }
    } else {
        RelayRequest::JoinV2 { token, punch }
    }
}

/// Turn on TCP keepalive for a bridged socket. A relayed chat may idle for hours, so
/// no inactivity timeout; but a half-open peer (slept, lost network, killed without
/// FIN) would pin `copy_bidirectional`, both sockets and the task forever. Failure to
/// set it is logged and ignored rather than refusing to bridge.
fn enable_keepalive(stream: &TcpStream, side: &str) {
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(Duration::from_secs(60))
        .with_interval(Duration::from_secs(20));
    if let Err(e) = socket2::SockRef::from(stream).set_tcp_keepalive(&keepalive) {
        tracing::debug!(error = %e, side, "could not enable TCP keepalive on a bridged socket");
    }
}

async fn handle_relay_connection(
    mut stream: TcpStream,
    pending: PendingMap,
    wait_timeout: Duration,
) -> Result<()> {
    // Bounded, so a client that never speaks cannot hold this task open.
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
    let (token, caps, is_host) = match hello {
        RelayRequest::Host { token } => (token, None, true),
        RelayRequest::Join { token } => (token, None, false),
        RelayRequest::HostV2 { token, punch } => (token, Some(sanitize_caps(punch)), true),
        RelayRequest::JoinV2 { token, punch } => (token, Some(sanitize_caps(punch)), false),
    };
    if is_host {
        host_flow(stream, pending, token, caps, wait_timeout).await
    } else {
        join_flow(stream, pending, token, caps, wait_timeout).await
    }
}

/// Tell the client why it is refused, then fail with the same message.
async fn refuse(stream: &mut TcpStream, reason: &str) -> Result<()> {
    send_relay_message(stream, &RelayResponse::Error(reason.to_string())).await?;
    bail!("{reason}")
}

/// Bound peer-supplied candidate lists before forwarding them (the server never parses
/// them, so cap count and length).
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
        // Sweep expired slots before deciding the map is full, or a burst of abandoned
        // hosts would lock out legitimate ones until their own timeouts fire.
        guard.retain(|_, entry| entry.created_at.elapsed() <= wait_timeout);
        if guard.len() >= MAX_PENDING_RENDEZVOUS {
            send_relay_message(
                &mut stream,
                &RelayResponse::Error("Relay is at capacity, try again shortly".to_string()),
            )
            .await?;
            bail!("relay rendezvous table is full ({MAX_PENDING_RENDEZVOUS} slots)");
        }
        // This answer reveals whether a token is registered, but so does `Waiting`:
        // registering is the observable difference. Token entropy and
        // `validate_token` are what bound guessing.
        if guard.contains_key(&token) {
            return refuse(&mut stream, "Relay token already in use").await;
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

    let waiting_since = Instant::now();
    let (mut peer_stream, joiner_caps) =
        match tokio::time::timeout(wait_timeout, rendezvous_rx).await {
            Ok(Ok(paired)) => paired,
            // The sender went away unpaired: the joiner's task dropped, or a sweep
            // removed our aged-out slot (the timeout by another route). Free the
            // slot and say which, so an expiry is not mistaken for a broken relay.
            Ok(Err(_)) => {
                release_dead_slot(&pending, &token).await;
                let reason = if waiting_since.elapsed() >= wait_timeout {
                    "Relay wait timed out"
                } else {
                    "Relay joiner dropped before pairing"
                };
                return refuse(&mut stream, reason).await;
            }
            Err(_) => {
                release_dead_slot(&pending, &token).await;
                return refuse(&mut stream, "Relay wait timed out").await;
            }
        };

    // Punch only when both sides are punch-capable; any hiccup degrades to the bridge.
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
    // A peer that vanishes without closing would otherwise pin this task forever.
    enable_keepalive(&stream, "host");
    enable_keepalive(&peer_stream, "joiner");
    copy_bidirectional(&mut stream, &mut peer_stream).await?;
    Ok(())
}

/// Release a host's rendezvous slot once its wait is over, but only if it is still a
/// dead slot (receiver gone). A sweep may already have removed it and the same token
/// been registered again by a retrying host, whose live slot must survive.
async fn release_dead_slot(pending: &PendingMap, token: &str) {
    let mut guard = pending.lock().await;
    if guard
        .get(token)
        .is_some_and(|slot| slot.rendezvous_tx.is_closed())
    {
        guard.remove(token);
    }
}

async fn join_flow(
    mut stream: TcpStream,
    pending: PendingMap,
    token: String,
    joiner_caps: Option<PunchCaps>,
    wait_timeout: Duration,
) -> Result<()> {
    validate_token(&token)?;
    // Separate statement so the lock is released before any reply is sent.
    let entry = pending.lock().await.remove(&token);
    let Some(pending_entry) = entry else {
        return refuse(&mut stream, "Unknown relay token").await;
    };
    if pending_entry.created_at.elapsed() > wait_timeout {
        return refuse(&mut stream, "Relay token expired").await;
    }

    // Acknowledge before handing the socket over: only a legacy relay (which cannot
    // parse `JoinV2`) stays silent, so the joiner falls back to legacy mode solely in
    // that case and not when the host vanished at the handoff.
    send_relay_message(&mut stream, &RelayResponse::Waiting).await?;

    pending_entry
        .rendezvous_tx
        .send((stream, joiner_caps))
        .map_err(|_| anyhow!("Relay host is no longer available"))?;
    Ok(())
}

/// Hand each peer the other's endpoints, then wait for both outcome reports.
/// `Ok(true)` means both confirmed a direct connection; anything else means bridge.
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
    use tokio::task::JoinHandle;

    /// Reserve an ephemeral loopback port, then free it for the server to bind.
    fn free_port() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
        listener.local_addr().expect("local addr").port()
    }

    async fn start_relay() -> String {
        start_relay_with_wait(RELAY_WAIT_TIMEOUT).await
    }

    /// A relay with a caller-chosen slot lifetime, so expiry is testable quickly.
    async fn start_relay_with_wait(wait: Duration) -> String {
        let port = free_port();
        tokio::spawn(async move {
            let _ = run_relay_server_with_wait_timeout(port, wait).await;
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        format!("127.0.0.1:{port}")
    }

    fn spawn_connect(
        relay: &str,
        token: &str,
        as_host: bool,
        punch: bool,
    ) -> JoinHandle<Result<RelayStream>> {
        let (relay, token) = (relay.to_string(), token.to_string());
        tokio::spawn(async move { connect_to_relay_with(&relay, &token, as_host, punch).await })
    }

    /// Register a host, give it `delay`, then a joiner; returns both outcomes.
    async fn pair(
        relay: &str,
        token: &str,
        host_punch: bool,
        join_punch: bool,
        delay: Duration,
    ) -> (Result<RelayStream>, Result<RelayStream>) {
        let host = spawn_connect(relay, token, true, host_punch);
        tokio::time::sleep(delay).await;
        let join = spawn_connect(relay, token, false, join_punch);
        (host.await.unwrap(), join.await.unwrap())
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

    /// A finished host releases only a dead slot: removing by token alone deleted a
    /// slot a new host had registered with the same token after the old one expired.
    #[tokio::test]
    async fn a_finished_wait_releases_only_a_dead_slot() {
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let token = "ab".repeat(RELAY_TOKEN_BYTES);

        let (tx, live_rx) = oneshot::channel();
        pending.lock().await.insert(
            token.clone(),
            PendingRelay {
                created_at: Instant::now(),
                rendezvous_tx: tx,
            },
        );
        release_dead_slot(&pending, &token).await;
        assert!(
            pending.lock().await.contains_key(&token),
            "a waiting host's slot was released by somebody else's timeout"
        );

        drop(live_rx);
        release_dead_slot(&pending, &token).await;
        assert!(!pending.lock().await.contains_key(&token));
    }

    /// (host punch-capable, joiner punch-capable, expect a direct socket). A mixed
    /// pair must not start a punch phase the legacy joiner cannot understand.
    #[tokio::test]
    async fn peers_pair_direct_or_bridged_by_capability() {
        let relay = start_relay().await;
        for (host_punch, join_punch, direct) in [
            (false, false, false),
            (true, true, true),
            (true, false, false),
        ] {
            let token = generate_relay_token();
            let delay = Duration::from_millis(100);
            let (host, join) = pair(&relay, &token, host_punch, join_punch, delay).await;
            let (host, join) = (host.unwrap(), join.unwrap());
            for stream in [&host, &join] {
                assert_eq!(
                    matches!(stream, RelayStream::Direct(_)),
                    direct,
                    "host punch {host_punch}, joiner punch {join_punch}: {stream:?}"
                );
            }
            exchange_ping_pong(host, join).await;
        }
    }

    /// Answering nothing would leave a client hanging on a typo'd token.
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

    /// A second host would otherwise replace the first and steal whoever joins next.
    #[tokio::test]
    async fn a_token_cannot_be_hosted_twice_at_once() {
        let relay_addr = start_relay().await;
        let token = generate_relay_token();
        let first = spawn_connect(&relay_addr, &token, true, true);
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

    /// A stale token is refused rather than handed a host that gave up, and the
    /// host's own wait ends instead of parking forever.
    #[tokio::test]
    async fn an_expired_token_no_longer_pairs() {
        let relay_addr = start_relay_with_wait(Duration::from_millis(300)).await;
        let token = generate_relay_token();
        let host = spawn_connect(&relay_addr, &token, true, true);
        tokio::time::sleep(Duration::from_millis(900)).await;

        let err = connect_to_relay_with(&relay_addr, &token, false, true)
            .await
            .expect_err("an expired token must not pair");
        let msg = err.to_string();
        assert!(
            msg.contains("expired") || msg.contains("Unknown relay token"),
            "the joiner should be refused, got: {msg}"
        );

        let host_result = tokio::time::timeout(Duration::from_secs(5), host)
            .await
            .expect("the host's wait must time out on its own");
        assert!(host_result.expect("host task").is_err());
    }

    /// The token is released once its peers are joined, so the reconnect that
    /// follows every dropped connection can reuse it.
    #[tokio::test]
    async fn a_token_can_be_reused_once_its_pairing_is_over() {
        let relay_addr = start_relay().await;
        let token = generate_relay_token();
        for round in 0..2 {
            let (host, join) = pair(
                &relay_addr,
                &token,
                false,
                false,
                Duration::from_millis(100),
            )
            .await;
            let host = host.unwrap_or_else(|e| panic!("round {round} host: {e}"));
            let join = join.unwrap_or_else(|e| panic!("round {round} joiner: {e}"));
            exchange_ping_pong(host, join).await;
            // Dropping both ends releases the bridge before the next round.
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Several pairings share one relay; each must reach its own partner.
    #[tokio::test]
    async fn concurrent_pairings_do_not_cross() {
        let relay_addr = start_relay().await;
        let tasks: Vec<_> = (0..4)
            .map(|i| {
                let relay_addr = relay_addr.clone();
                tokio::spawn(async move {
                    let token = generate_relay_token();
                    let delay = Duration::from_millis(100);
                    let (host, join) = pair(&relay_addr, &token, false, false, delay).await;
                    let (mut host_stream, _) = label_stream(host.expect("host pairs"), "test");
                    let (mut join_stream, _) = label_stream(join.expect("joiner pairs"), "test");
                    let payload = format!("pair-{i}");
                    host_stream.write_all(payload.as_bytes()).await.unwrap();
                    let mut buf = vec![0u8; payload.len()];
                    join_stream.read_exact(&mut buf).await.unwrap();
                    assert_eq!(
                        String::from_utf8(buf).unwrap(),
                        payload,
                        "pair {i} received another pair's bytes"
                    );
                })
            })
            .collect();
        for t in tasks {
            t.await.expect("pair task");
        }
    }

    /// The pre-punch relay server: only the legacy variants exist, so a V2 request
    /// fails to deserialize and the connection drops without a response.
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

    /// A punch-capable registration dropped by an old server is silently retried in
    /// legacy mode and still pairs.
    #[tokio::test]
    async fn new_clients_fall_back_against_a_legacy_server() {
        let port = free_port();
        tokio::spawn(async move {
            let _ = legacy_server::run(port).await;
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        let relay_addr = format!("127.0.0.1:{port}");
        let token = generate_relay_token();

        let (host, join) = pair(&relay_addr, &token, true, true, Duration::from_millis(150)).await;
        let host = host.expect("host legacy fallback");
        let join = join.expect("joiner legacy fallback");
        assert!(matches!(host, RelayStream::Relayed(_)));
        assert!(matches!(join, RelayStream::Relayed(_)));
        exchange_ping_pong(host, join).await;
    }
}
