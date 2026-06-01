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

const RELAY_WAIT_TIMEOUT: Duration = Duration::from_secs(300);
const RELAY_TOKEN_BYTES: usize = 16;

#[derive(Debug, Serialize, Deserialize)]
enum RelayRequest {
    Host { token: String },
    Join { token: String },
}

#[derive(Debug, Serialize, Deserialize)]
enum RelayResponse {
    Waiting,
    Paired,
    Error(String),
}

struct PendingRelay {
    created_at: Instant,
    rendezvous_tx: oneshot::Sender<TcpStream>,
}

type PendingMap = Arc<Mutex<HashMap<String, PendingRelay>>>;

pub fn generate_relay_token() -> String {
    let mut bytes = [0u8; RELAY_TOKEN_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub async fn run_relay_server(port: u16) -> Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
    tracing::info!("Relay server listening on port {}", port);

    loop {
        let (stream, addr) = listener.accept().await?;
        tracing::info!("Relay client connected from {}", addr);
        let pending = pending.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_relay_connection(stream, pending).await {
                tracing::warn!("Relay connection failed: {}", e);
            }
        });
    }
}

pub async fn run_host_session_via_relay(
    relay_server: &str,
    token: &str,
    privkey: rsa::RsaPrivateKey,
    to_app_tx: tokio::sync::mpsc::UnboundedSender<crate::types::SessionEvent>,
    from_app_rx: tokio::sync::mpsc::UnboundedReceiver<crate::core::ProtocolMessage>,
    confirm_rx: tokio::sync::mpsc::UnboundedReceiver<bool>,
    chat_id: uuid::Uuid,
) -> Result<()> {
    let mut stream = connect_to_relay(relay_server, token, true).await?;
    crate::network::session::run_host_session_over_stream(
        &mut stream,
        format!("relay:{}", relay_server),
        privkey,
        to_app_tx,
        from_app_rx,
        confirm_rx,
        chat_id,
        None,
    )
    .await
}

pub async fn run_client_session_via_relay(
    relay_server: &str,
    token: &str,
    privkey: rsa::RsaPrivateKey,
    to_app_tx: tokio::sync::mpsc::UnboundedSender<crate::types::SessionEvent>,
    from_app_rx: tokio::sync::mpsc::UnboundedReceiver<crate::core::ProtocolMessage>,
    confirm_rx: tokio::sync::mpsc::UnboundedReceiver<bool>,
    chat_id: uuid::Uuid,
) -> Result<()> {
    let mut stream = connect_to_relay(relay_server, token, false).await?;
    crate::network::session::run_client_session_over_stream(
        &mut stream,
        format!("relay:{}", relay_server),
        privkey,
        to_app_tx,
        from_app_rx,
        confirm_rx,
        chat_id,
        None,
    )
    .await
}

async fn connect_to_relay(relay_server: &str, token: &str, as_host: bool) -> Result<TcpStream> {
    validate_token(token)?;
    let (host, port) = crate::util::parse_host_port(relay_server, Some(crate::PORT_DEFAULT))?;
    let mut stream = TcpStream::connect((host.as_str(), port)).await?;
    let request = if as_host {
        RelayRequest::Host {
            token: token.to_string(),
        }
    } else {
        RelayRequest::Join {
            token: token.to_string(),
        }
    };
    send_relay_message(&mut stream, &request).await?;

    loop {
        match recv_relay_message::<RelayResponse>(&mut stream).await? {
            RelayResponse::Waiting => continue,
            RelayResponse::Paired => {
                tracing::info!("Relay session paired via {}", relay_server);
                return Ok(stream);
            }
            RelayResponse::Error(message) => bail!("Relay refused connection: {}", message),
        }
    }
}

async fn handle_relay_connection(mut stream: TcpStream, pending: PendingMap) -> Result<()> {
    match recv_relay_message::<RelayRequest>(&mut stream).await? {
        RelayRequest::Host { token } => {
            validate_token(&token)?;

            let (rendezvous_tx, rendezvous_rx) = oneshot::channel();
            {
                let mut guard = pending.lock().await;
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

            let mut peer_stream =
                match tokio::time::timeout(RELAY_WAIT_TIMEOUT, rendezvous_rx).await {
                    Ok(Ok(peer_stream)) => peer_stream,
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

            send_relay_message(&mut stream, &RelayResponse::Paired).await?;
            send_relay_message(&mut peer_stream, &RelayResponse::Paired).await?;
            copy_bidirectional(&mut stream, &mut peer_stream).await?;
            Ok(())
        }
        RelayRequest::Join { token } => {
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

            if pending_entry.created_at.elapsed() > RELAY_WAIT_TIMEOUT {
                send_relay_message(
                    &mut stream,
                    &RelayResponse::Error("Relay token expired".to_string()),
                )
                .await?;
                bail!("Relay token expired");
            }

            pending_entry
                .rendezvous_tx
                .send(stream)
                .map_err(|_| anyhow!("Relay host is no longer available"))?;
            Ok(())
        }
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

    #[tokio::test]
    async fn relay_pairs_peers_and_forwards_bytes() {
        let port = crate::PORT_DEFAULT + 77;
        tokio::spawn(async move {
            let _ = run_relay_server(port).await;
        });

        tokio::time::sleep(Duration::from_millis(200)).await;

        let token = generate_relay_token();
        let relay_addr = format!("127.0.0.1:{}", port);

        let host_task = tokio::spawn({
            let relay_addr = relay_addr.clone();
            let token = token.clone();
            async move {
                let mut stream = connect_to_relay(&relay_addr, &token, true).await.unwrap();
                stream.write_all(b"ping").await.unwrap();
                let mut buf = [0u8; 4];
                stream.read_exact(&mut buf).await.unwrap();
                assert_eq!(&buf, b"pong");
            }
        });

        let join_task = tokio::spawn(async move {
            let mut stream = connect_to_relay(&relay_addr, &token, false).await.unwrap();
            let mut buf = [0u8; 4];
            stream.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"ping");
            stream.write_all(b"pong").await.unwrap();
        });

        host_task.await.unwrap();
        join_task.await.unwrap();
    }
}
