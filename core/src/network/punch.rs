//! TCP hole punching (simultaneous open), coordinated by the relay rendezvous.
//!
//! The relay learns each peer's public endpoint from the source address of its
//! control connection and hands it to the other side together with the peer's
//! self-reported LAN candidates. Both peers then:
//!
//! 1. re-bind the *same local port* their relay control connection uses
//!    (`SO_REUSEADDR`, plus `SO_REUSEPORT` on Unix), so outbound SYNs reuse the
//!    NAT mapping the control connection already created;
//! 2. listen on that port *and* repeatedly `connect()` toward every peer
//!    candidate — crossing SYNs complete as a TCP simultaneous open on NATs
//!    with endpoint-independent mapping, and the listener catches the
//!    straggler SYN when one NAT is friendlier than the other;
//! 3. validate every socket that gets established with a fixed-size hello
//!    frame (magic + role + a tag derived from the rendezvous token), which
//!    rejects strangers, crossed sessions, and self-connections;
//! 4. deterministically select exactly one socket: the host picks the first
//!    validated socket and sends `SELECT`; the joiner answers `ACK`. Success is
//!    reported only after that mutual confirmation, so both ends run the chat
//!    handshake over the same connection.
//!
//! Everything is bounded by deadlines, and any failure makes the caller fall
//! back to the bridged relay path. The hello tag is pairing hygiene, not
//! authentication — the punched socket carries the exact same v3 handshake
//! (ECDH, identity proof, TOFU) as any direct connection.

use anyhow::{anyhow, bail, Result};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::{sleep, timeout, timeout_at, Instant};

/// Overall budget for establishing and validating candidate sockets.
pub(crate) const PUNCH_TIMEOUT: Duration = Duration::from_secs(10);
/// Extra slack for the SELECT/ACK confirmation after the candidate deadline.
/// The joiner waits twice this so the host always decides first.
const SELECT_GRACE: Duration = Duration::from_secs(3);
/// Per-attempt connect timeout; a NAT that will ever let the SYN through does
/// so quickly, and short attempts mean more crossing SYNs per budget.
const CONNECT_ATTEMPT_TIMEOUT: Duration = Duration::from_millis(1500);
/// Pause between connect retries toward the same candidate.
const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(300);
/// Budget for the hello exchange on one established socket.
const VALIDATE_TIMEOUT: Duration = Duration::from_secs(3);
/// Cap on how many peer candidates we will dial (abuse guard: candidates are
/// peer-supplied strings relayed by the server).
pub(crate) const MAX_PUNCH_CANDIDATES: usize = 8;

const HELLO_MAGIC: &[u8; 8] = b"P2PPNCH1";
const TAG_LEN: usize = 16;
const HELLO_LEN: usize = HELLO_MAGIC.len() + 1 + TAG_LEN;
const SELECT_BYTE: u8 = 0xA5;
const ACK_BYTE: u8 = 0x5A;

/// Which side of the rendezvous we are. The host leads socket selection; the
/// joiner follows. Both sides listen *and* dial regardless of role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PunchRole {
    Host,
    Joiner,
}

impl PunchRole {
    fn byte(self) -> u8 {
        match self {
            PunchRole::Host => 0,
            PunchRole::Joiner => 1,
        }
    }
}

/// Derive the pairing tag exchanged in hello frames from the rendezvous token.
fn token_tag(token: &str) -> [u8; TAG_LEN] {
    let digest = Sha256::digest(token.as_bytes());
    let mut tag = [0u8; TAG_LEN];
    tag.copy_from_slice(&digest[..TAG_LEN]);
    tag
}

/// Turn the relay-observed public endpoint plus the peer's self-reported LAN
/// candidates into a deduplicated, sanity-filtered dial list (public first).
pub(crate) fn parse_candidates(peer_public: &str, peer_locals: &[String]) -> Vec<SocketAddr> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for raw in std::iter::once(peer_public).chain(peer_locals.iter().map(String::as_str)) {
        let Ok(addr) = raw.parse::<SocketAddr>() else {
            continue;
        };
        if addr.port() == 0 || addr.ip().is_unspecified() || addr.ip().is_multicast() {
            continue;
        }
        if seen.insert(addr) {
            out.push(addr);
        }
        if out.len() >= MAX_PUNCH_CANDIDATES {
            break;
        }
    }
    out
}

/// Try to hole punch a direct TCP connection to the peer. `local_port` must be
/// the local port of the relay control connection (its NAT mapping is what the
/// punch reuses). Returns a mutually confirmed, validated stream, or an error
/// after the deadline — the caller then falls back to the bridged relay.
pub(crate) async fn attempt_punch(
    role: PunchRole,
    local_port: u16,
    peer_candidates: &[SocketAddr],
    token: &str,
) -> Result<TcpStream> {
    attempt_punch_with_timeout(role, local_port, peer_candidates, token, PUNCH_TIMEOUT).await
}

pub(crate) async fn attempt_punch_with_timeout(
    role: PunchRole,
    local_port: u16,
    peer_candidates: &[SocketAddr],
    token: &str,
    budget: Duration,
) -> Result<TcpStream> {
    if peer_candidates.is_empty() {
        bail!("no usable hole-punch candidates");
    }
    if local_port == 0 {
        bail!("cannot punch without a bound local port");
    }
    let tag = token_tag(token);
    let deadline = Instant::now() + budget;

    // Validated sockets flow from listener/dialer workers into the selection
    // phase. Workers stop at the deadline; dropping the JoinSet aborts any
    // stragglers and closes their unclaimed sockets.
    let (validated_tx, validated_rx) = mpsc::channel::<TcpStream>(MAX_PUNCH_CANDIDATES);
    let mut workers: JoinSet<()> = JoinSet::new();

    // Listeners: v4 always (the relay connection is normally v4), v6 only if
    // some candidate needs it. A bind failure is not fatal — the dialers can
    // still win via simultaneous open.
    let want_v6 = peer_candidates.iter().any(SocketAddr::is_ipv6);
    let want_v4 = peer_candidates.iter().any(SocketAddr::is_ipv4);
    for ipv6 in [false, true] {
        if (ipv6 && !want_v6) || (!ipv6 && !want_v4) {
            continue;
        }
        match punch_listener(ipv6, local_port) {
            Ok(listener) => {
                let tx = validated_tx.clone();
                workers.spawn(async move {
                    loop {
                        match timeout_at(deadline, listener.accept()).await {
                            Ok(Ok((stream, _))) => {
                                let tx = tx.clone();
                                tokio::spawn(async move {
                                    match validate(stream, role, tag).await {
                                        Ok(valid) => {
                                            let _ = tx.send(valid).await;
                                        }
                                        Err(e) => {
                                            tracing::trace!(error = %e, "punch accept rejected")
                                        }
                                    }
                                });
                            }
                            Ok(Err(e)) => {
                                tracing::trace!(error = %e, "punch accept failed");
                                sleep(CONNECT_RETRY_DELAY).await;
                            }
                            Err(_) => return,
                        }
                    }
                });
            }
            Err(e) => tracing::debug!(error = %e, ipv6, "punch listener bind failed"),
        }
    }

    // Dialers: one retry loop per candidate, all from the same local port.
    for &dest in peer_candidates.iter().take(MAX_PUNCH_CANDIDATES) {
        let tx = validated_tx.clone();
        workers.spawn(async move {
            while Instant::now() < deadline {
                match connect_once(local_port, dest).await {
                    Ok(stream) => match validate(stream, role, tag).await {
                        Ok(valid) => {
                            let _ = tx.send(valid).await;
                            return;
                        }
                        Err(e) => {
                            tracing::trace!(error = %e, %dest, "punch dial rejected");
                            sleep(CONNECT_RETRY_DELAY).await;
                        }
                    },
                    Err(_) => sleep(CONNECT_RETRY_DELAY).await,
                }
            }
        });
    }
    drop(validated_tx);

    let result = match role {
        PunchRole::Host => select_as_host(validated_rx, deadline).await,
        PunchRole::Joiner => select_as_joiner(validated_rx, deadline).await,
    };
    workers.abort_all();
    result
}

/// Host side of selection: take validated sockets as they arrive, claim the
/// first one that answers the SELECT/ACK exchange.
async fn select_as_host(
    mut validated_rx: mpsc::Receiver<TcpStream>,
    deadline: Instant,
) -> Result<TcpStream> {
    let select_deadline = deadline + SELECT_GRACE;
    loop {
        let stream = match timeout_at(select_deadline, validated_rx.recv()).await {
            Ok(Some(stream)) => stream,
            Ok(None) => bail!("hole punch produced no usable connection"),
            Err(_) => bail!("hole punch timed out"),
        };
        match confirm_selection(stream).await {
            Ok(confirmed) => return Ok(confirmed),
            Err(e) => tracing::trace!(error = %e, "punch selection candidate failed"),
        }
    }
}

async fn confirm_selection(mut stream: TcpStream) -> Result<TcpStream> {
    stream.write_all(&[SELECT_BYTE]).await?;
    let mut ack = [0u8; 1];
    timeout(SELECT_GRACE, stream.read_exact(&mut ack))
        .await
        .map_err(|_| anyhow!("selection ack timed out"))??;
    if ack[0] != ACK_BYTE {
        bail!("unexpected selection ack byte {:#x}", ack[0]);
    }
    Ok(stream)
}

/// Joiner side of selection: park every validated socket on a SELECT read and
/// acknowledge whichever one the host picks. The longer deadline guarantees
/// the host decides first.
async fn select_as_joiner(
    mut validated_rx: mpsc::Receiver<TcpStream>,
    deadline: Instant,
) -> Result<TcpStream> {
    let select_deadline = deadline + SELECT_GRACE * 2;
    let mut waiters: JoinSet<Result<TcpStream>> = JoinSet::new();
    let mut rx_open = true;
    loop {
        tokio::select! {
            maybe = validated_rx.recv(), if rx_open => match maybe {
                Some(mut stream) => {
                    waiters.spawn(async move {
                        let mut sel = [0u8; 1];
                        stream.read_exact(&mut sel).await?;
                        if sel[0] != SELECT_BYTE {
                            bail!("unexpected selection byte {:#x}", sel[0]);
                        }
                        stream.write_all(&[ACK_BYTE]).await?;
                        Ok(stream)
                    });
                }
                None => {
                    rx_open = false;
                    if waiters.is_empty() {
                        bail!("hole punch produced no usable connection");
                    }
                }
            },
            joined = waiters.join_next(), if !waiters.is_empty() => {
                match joined {
                    Some(Ok(Ok(stream))) => return Ok(stream),
                    // A waiter failed (bad SELECT byte / dropped socket). If the
                    // receiver is closed and no waiters remain, nothing else can
                    // arrive — give up so we fall back to the bridged relay.
                    Some(_) if !rx_open && waiters.is_empty() => {
                        bail!("hole punch produced no usable connection");
                    }
                    _ => {}
                }
            },
            _ = tokio::time::sleep_until(select_deadline) => bail!("hole punch timed out"),
        }
    }
}

/// Bind a reusable listening socket on the punch port.
fn punch_listener(ipv6: bool, local_port: u16) -> Result<TcpListener> {
    let socket = reusable_socket(ipv6, local_port)?;
    Ok(socket.listen(16)?)
}

/// Dial one candidate from the (reused) punch port, bounded by the per-attempt
/// timeout.
async fn connect_once(local_port: u16, dest: SocketAddr) -> Result<TcpStream> {
    let socket = reusable_socket(dest.is_ipv6(), local_port)?;
    let stream = timeout(CONNECT_ATTEMPT_TIMEOUT, socket.connect(dest))
        .await
        .map_err(|_| anyhow!("connect attempt timed out"))??;
    Ok(stream)
}

/// Connect to `host:port` from a reuse-enabled socket. The relay control
/// connection MUST be dialed this way: punch sockets later re-bind its local
/// port, and (on Linux) that only works when the first socket on the port was
/// itself bound with the reuse flags set.
pub(crate) async fn connect_reusable(host: &str, port: u16) -> Result<TcpStream> {
    let mut last_err: Option<anyhow::Error> = None;
    for addr in tokio::net::lookup_host((host, port)).await? {
        let socket = match reusable_socket(addr.is_ipv6(), 0) {
            Ok(socket) => socket,
            Err(e) => {
                last_err = Some(e);
                continue;
            }
        };
        match socket.connect(addr).await {
            Ok(stream) => return Ok(stream),
            Err(e) => last_err = Some(e.into()),
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("relay address did not resolve")))
}

/// A TCP socket bound to the punch port with address (and, on Unix, port)
/// reuse enabled, so several dialers, the listener, and the still-open relay
/// control connection can share one local port.
fn reusable_socket(ipv6: bool, local_port: u16) -> Result<TcpSocket> {
    let socket = if ipv6 {
        TcpSocket::new_v6()?
    } else {
        TcpSocket::new_v4()?
    };
    socket.set_reuseaddr(true)?;
    #[cfg(all(unix, not(target_os = "solaris"), not(target_os = "illumos")))]
    socket.set_reuseport(true)?;
    let bind_addr: SocketAddr = if ipv6 {
        (Ipv6Addr::UNSPECIFIED, local_port).into()
    } else {
        (Ipv4Addr::UNSPECIFIED, local_port).into()
    };
    socket.bind(bind_addr)?;
    Ok(socket)
}

/// Mutual hello exchange on a freshly established socket: proves the other end
/// is our rendezvous peer (same token, opposite role) before the socket may
/// enter selection.
async fn validate(mut stream: TcpStream, role: PunchRole, tag: [u8; TAG_LEN]) -> Result<TcpStream> {
    stream.set_nodelay(true).ok();
    let exchange = async {
        let mut hello = [0u8; HELLO_LEN];
        hello[..HELLO_MAGIC.len()].copy_from_slice(HELLO_MAGIC);
        hello[HELLO_MAGIC.len()] = role.byte();
        hello[HELLO_MAGIC.len() + 1..].copy_from_slice(&tag);
        stream.write_all(&hello).await?;

        let mut peer = [0u8; HELLO_LEN];
        stream.read_exact(&mut peer).await?;
        if &peer[..HELLO_MAGIC.len()] != HELLO_MAGIC {
            bail!("bad punch hello magic");
        }
        if peer[HELLO_MAGIC.len()] == role.byte() {
            bail!("punch socket loops back to our own role");
        }
        if peer[HELLO_MAGIC.len() + 1..] != tag {
            bail!("punch pairing tag mismatch");
        }
        anyhow::Ok(())
    };
    timeout(VALIDATE_TIMEOUT, exchange)
        .await
        .map_err(|_| anyhow!("punch validation timed out"))??;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Reserve an ephemeral loopback port, then free it for the punch to bind.
    fn free_port() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
        listener.local_addr().expect("local addr").port()
    }

    #[test]
    fn candidates_are_filtered_deduped_and_capped() {
        let locals: Vec<String> = vec![
            "not-an-addr".into(),
            "192.168.1.9:0".into(),      // port 0
            "0.0.0.0:9000".into(),       // unspecified
            "224.0.0.1:9000".into(),     // multicast
            "203.0.113.7:9000".into(),   // duplicate of public
            "192.168.1.9:9000".into(),   // good LAN candidate
            "[2001:db8::7]:9000".into(), // good v6 candidate
        ];
        let parsed = parse_candidates("203.0.113.7:9000", &locals);
        assert_eq!(
            parsed,
            vec![
                "203.0.113.7:9000".parse().unwrap(),
                "192.168.1.9:9000".parse().unwrap(),
                "[2001:db8::7]:9000".parse().unwrap(),
            ]
        );

        let many: Vec<String> = (0..20)
            .map(|i| format!("203.0.113.{}:9000", i + 10))
            .collect();
        assert_eq!(
            parse_candidates("203.0.113.7:9000", &many).len(),
            MAX_PUNCH_CANDIDATES
        );
    }

    #[tokio::test]
    async fn peers_punch_each_other_over_loopback() {
        let port_a = free_port();
        let port_b = free_port();
        let token = "cafebabe";

        let host = tokio::spawn(async move {
            attempt_punch(
                PunchRole::Host,
                port_a,
                &[format!("127.0.0.1:{port_b}").parse().unwrap()],
                token,
            )
            .await
        });
        let joiner = tokio::spawn(async move {
            attempt_punch(
                PunchRole::Joiner,
                port_b,
                &[format!("127.0.0.1:{port_a}").parse().unwrap()],
                token,
            )
            .await
        });

        let mut host_stream = host.await.unwrap().expect("host punch");
        let mut joiner_stream = joiner.await.unwrap().expect("joiner punch");

        host_stream.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        joiner_stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
        joiner_stream.write_all(b"pong").await.unwrap();
        host_stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"pong");
    }

    #[tokio::test]
    async fn mismatched_tokens_never_pair() {
        let port_a = free_port();
        let port_b = free_port();
        let budget = Duration::from_secs(2);

        let host = tokio::spawn(async move {
            attempt_punch_with_timeout(
                PunchRole::Host,
                port_a,
                &[format!("127.0.0.1:{port_b}").parse().unwrap()],
                "token-one",
                budget,
            )
            .await
        });
        let joiner = tokio::spawn(async move {
            attempt_punch_with_timeout(
                PunchRole::Joiner,
                port_b,
                &[format!("127.0.0.1:{port_a}").parse().unwrap()],
                "token-two",
                budget,
            )
            .await
        });

        assert!(host.await.unwrap().is_err());
        assert!(joiner.await.unwrap().is_err());
    }
}
