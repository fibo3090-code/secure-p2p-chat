//! Party application protocol (Phase 1).
//!
//! Shared wire contract between the client and the Party server. It rides *on top
//! of* the established Protocol v3 encrypted tunnel (see `network::session`): the
//! handshake authenticates and encrypts the channel to the server, and these
//! messages carry the Party-level application semantics (join, directory, channel
//! messaging, offline catch-up).
//!
//! Two trust tiers share one data model (see `docs/05_platform_spec.md`):
//! - **Administered** (default): the server stores plaintext payloads.
//! - **E2EE** (Phase 4): payloads are ciphertext; the server never sees plaintext.
//!
//! The MVP implements the Administered tier.

use rsa::RsaPrivateKey;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite, ReadHalf, WriteHalf};
use uuid::Uuid;

use crate::core::{recv_packet, send_packet, AesCipher};
use crate::network::client_handshake;

/// Trust tier of a server / stored message. A property of the server, surfaced
/// prominently in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TrustTier {
    /// Server stores plaintext; enables offline buffering, search, admin-read.
    #[default]
    Administered,
    /// Server stores ciphertext only; admin cannot read (Phase 4).
    E2EE,
}

/// The application payload carried by an [`Envelope`]. For the Administered tier
/// this is plaintext; the E2EE tier (Phase 4) will add a ciphertext variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessagePayload {
    Text(String),
}

/// A single stored/transported message. One data model across both tiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub tier: TrustTier,
    /// Member id of the sender.
    pub sender: Uuid,
    /// Channel (or, later, DM thread) the message belongs to.
    pub channel: Uuid,
    /// Per-channel monotonic sequence number assigned by the server.
    pub seq: u64,
    /// Unix milliseconds.
    pub timestamp: u64,
    pub payload: MessagePayload,
}

/// Public information about a member, as shown in the directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberInfo {
    pub id: Uuid,
    pub username: String,
    pub online: bool,
}

/// Channel kind. The MVP ships `Public`; the rest are reserved for Phase 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ChannelKind {
    #[default]
    Public,
    Private,
    Locked,
    Announce,
}

/// Public information about a channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelInfo {
    pub id: Uuid,
    pub name: String,
    pub kind: ChannelKind,
}

/// Client → server messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartyRequest {
    /// Join the server with a chosen username and the optional server password.
    Join {
        username: String,
        password: Option<String>,
    },
    /// Request the member directory.
    ListMembers,
    /// Request the channel list.
    ListChannels,
    /// Post a text message to a channel.
    PostMessage { channel: Uuid, text: String },
    /// Fetch channel history strictly after `since_seq` (offline catch-up).
    /// `since_seq = 0` returns the whole channel.
    FetchHistory { channel: Uuid, since_seq: u64 },
    /// Send a direct (1:1) message to another member.
    SendDm { to: Uuid, text: String },
    /// Fetch direct-message history with another member (offline catch-up).
    FetchDmHistory { with: Uuid, since_seq: u64 },
    /// Create a new public channel.
    CreateChannel { name: String },
}

/// Deterministic, order-independent id for the 1:1 DM thread between two members,
/// so both participants and the server agree on the same `Envelope.channel` for
/// their direct messages. A DM `Envelope` is just a [`Message`]/[`History`] item
/// whose `channel` is this thread id rather than a real channel id.
pub fn dm_thread_id(a: Uuid, b: Uuid) -> Uuid {
    use sha2::{Digest, Sha256};
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    let mut hasher = Sha256::new();
    hasher.update(b"party-dm-thread");
    hasher.update(lo.as_bytes());
    hasher.update(hi.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

/// Server → client messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartyResponse {
    /// Join accepted; carries the assigned member id and server context.
    Joined {
        member_id: Uuid,
        server_name: String,
        tier: TrustTier,
    },
    /// Join rejected (bad password, duplicate username, ...).
    JoinRejected {
        reason: String,
    },
    Members(Vec<MemberInfo>),
    Channels(Vec<ChannelInfo>),
    /// Acknowledges a posted message with its assigned per-channel sequence.
    MessagePosted {
        channel: Uuid,
        seq: u64,
    },
    /// A message delivered to the client (history item or live broadcast).
    Message(Envelope),
    /// A batch of history items (response to `FetchHistory`).
    History(Vec<Envelope>),
    /// A non-fatal application error.
    Error(String),
}

impl PartyRequest {
    /// Serialize to bytes for transport inside the encrypted tunnel.
    pub fn to_bytes(&self) -> Vec<u8> {
        bincode::serialize(self).expect("PartyRequest serialization is infallible")
    }

    /// Parse from transport bytes; returns `None` on malformed input.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        bincode::deserialize(bytes).ok()
    }
}

impl PartyResponse {
    /// Serialize to bytes for transport inside the encrypted tunnel.
    pub fn to_bytes(&self) -> Vec<u8> {
        bincode::serialize(self).expect("PartyResponse serialization is infallible")
    }

    /// Parse from transport bytes; returns `None` on malformed input.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        bincode::deserialize(bytes).ok()
    }
}

/// Send a serialized Party message over an established v3 tunnel: encrypt it with
/// the session `cipher` (bound to `transport_aad`) and length-prefix the result.
/// Used by both the client and the server for every Party request/response.
pub async fn send_framed<S>(
    stream: &mut S,
    cipher: &AesCipher,
    transport_aad: &[u8],
    payload: &[u8],
) -> anyhow::Result<()>
where
    S: AsyncWrite + Unpin,
{
    let ciphertext = cipher.encrypt(payload, Some(transport_aad));
    send_packet(stream, &ciphertext).await?;
    Ok(())
}

/// Receive and decrypt the next Party message from an established v3 tunnel.
/// Returns an error if the frame fails to authenticate/decrypt.
pub async fn recv_framed<S>(
    stream: &mut S,
    cipher: &AesCipher,
    transport_aad: &[u8],
) -> anyhow::Result<Vec<u8>>
where
    S: AsyncRead + Unpin,
{
    let ciphertext = recv_packet(stream).await?;
    cipher
        .decrypt(&ciphertext, Some(transport_aad))
        .ok_or_else(|| anyhow::anyhow!("failed to decrypt Party frame"))
}

/// Client-side handle to a Party server over an established Protocol v3 tunnel.
///
/// It completes the client handshake, then exposes `send`/`recv` over the encrypted
/// channel. Responses are a single stream of [`PartyResponse`] — direct replies and
/// pushed broadcasts interleaved — that the application correlates by variant. The
/// future client UI (slice 4) drives this; the server's integration tests exercise
/// it against the real `serve_connection`.
pub struct PartyClient<S> {
    stream: S,
    server_fingerprint: String,
    cipher: AesCipher,
    transport_aad: Vec<u8>,
}

/// Read half of a split [`PartyClient`].
pub struct PartyReader<R> {
    rd: R,
    cipher: AesCipher,
    transport_aad: Vec<u8>,
}

/// Write half of a split [`PartyClient`].
pub struct PartyWriter<W> {
    wr: W,
    cipher: AesCipher,
    transport_aad: Vec<u8>,
}

impl<R> PartyReader<R>
where
    R: AsyncRead + Unpin + Send,
{
    /// Receive the next message from the server (reply or pushed broadcast).
    pub async fn recv(&mut self) -> anyhow::Result<PartyResponse> {
        let bytes = recv_framed(&mut self.rd, &self.cipher, &self.transport_aad).await?;
        PartyResponse::from_bytes(&bytes)
            .ok_or_else(|| anyhow::anyhow!("malformed server response"))
    }
}

impl<W> PartyWriter<W>
where
    W: AsyncWrite + Unpin + Send,
{
    /// Send a request to the server.
    pub async fn send(&mut self, req: &PartyRequest) -> anyhow::Result<()> {
        send_framed(
            &mut self.wr,
            &self.cipher,
            &self.transport_aad,
            &req.to_bytes(),
        )
        .await
    }
}

impl<S> PartyClient<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    /// Complete the client side of the v3 handshake against a Party server and
    /// return a ready client. `chat_id` is this client's advertised session id.
    pub async fn connect(
        mut stream: S,
        privkey: &RsaPrivateKey,
        chat_id: Uuid,
    ) -> anyhow::Result<Self> {
        let tunnel = client_handshake(&mut stream, privkey, chat_id).await?;
        Ok(Self {
            stream,
            server_fingerprint: tunnel.peer_fingerprint,
            cipher: tunnel.cipher,
            transport_aad: tunnel.transport_aad,
        })
    }

    /// The server's handshake-verified identity fingerprint, for TOFU pinning.
    pub fn server_fingerprint(&self) -> &str {
        &self.server_fingerprint
    }

    /// Split into independent read and write halves so an application can run a
    /// receive loop concurrently with sending requests.
    pub fn split(self) -> (PartyReader<ReadHalf<S>>, PartyWriter<WriteHalf<S>>) {
        let (rd, wr) = tokio::io::split(self.stream);
        (
            PartyReader {
                rd,
                cipher: self.cipher.clone(),
                transport_aad: self.transport_aad.clone(),
            },
            PartyWriter {
                wr,
                cipher: self.cipher,
                transport_aad: self.transport_aad,
            },
        )
    }

    /// Send a request to the server.
    pub async fn send(&mut self, req: &PartyRequest) -> anyhow::Result<()> {
        send_framed(
            &mut self.stream,
            &self.cipher,
            &self.transport_aad,
            &req.to_bytes(),
        )
        .await
    }

    /// Receive the next message from the server (a reply or a pushed broadcast).
    pub async fn recv(&mut self) -> anyhow::Result<PartyResponse> {
        let bytes = recv_framed(&mut self.stream, &self.cipher, &self.transport_aad).await?;
        PartyResponse::from_bytes(&bytes)
            .ok_or_else(|| anyhow::anyhow!("malformed server response"))
    }

    /// Join the server with a username and the optional server password. The
    /// `Joined`/`JoinRejected` reply arrives before any broadcast can reach this
    /// connection (the server registers a connection for broadcasts only once it
    /// has joined), so the next message is the join result.
    pub async fn join(
        &mut self,
        username: &str,
        password: Option<String>,
    ) -> anyhow::Result<PartyResponse> {
        self.send(&PartyRequest::Join {
            username: username.to_string(),
            password,
        })
        .await?;
        self.recv().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_envelope() -> Envelope {
        Envelope {
            tier: TrustTier::Administered,
            sender: Uuid::new_v4(),
            channel: Uuid::new_v4(),
            seq: 7,
            timestamp: 1_700_000_000_000,
            payload: MessagePayload::Text("hello channel".to_string()),
        }
    }

    #[test]
    fn request_roundtrips() {
        let requests = vec![
            PartyRequest::Join {
                username: "alice".to_string(),
                password: Some("hunter2".to_string()),
            },
            PartyRequest::Join {
                username: "bob".to_string(),
                password: None,
            },
            PartyRequest::ListMembers,
            PartyRequest::ListChannels,
            PartyRequest::PostMessage {
                channel: Uuid::new_v4(),
                text: "hi all 👋".to_string(),
            },
            PartyRequest::FetchHistory {
                channel: Uuid::new_v4(),
                since_seq: 42,
            },
            PartyRequest::SendDm {
                to: Uuid::new_v4(),
                text: "psst".to_string(),
            },
            PartyRequest::FetchDmHistory {
                with: Uuid::new_v4(),
                since_seq: 3,
            },
            PartyRequest::CreateChannel {
                name: "random".to_string(),
            },
        ];
        for req in requests {
            let bytes = req.to_bytes();
            assert_eq!(PartyRequest::from_bytes(&bytes), Some(req));
        }
    }

    #[test]
    fn dm_thread_id_is_order_independent_and_distinct() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        assert_eq!(
            dm_thread_id(a, b),
            dm_thread_id(b, a),
            "thread id must be symmetric"
        );
        assert_ne!(
            dm_thread_id(a, b),
            dm_thread_id(a, c),
            "different pairs differ"
        );
    }

    #[test]
    fn response_roundtrips() {
        let responses = vec![
            PartyResponse::Joined {
                member_id: Uuid::new_v4(),
                server_name: "Study Server".to_string(),
                tier: TrustTier::Administered,
            },
            PartyResponse::JoinRejected {
                reason: "wrong password".to_string(),
            },
            PartyResponse::Members(vec![MemberInfo {
                id: Uuid::new_v4(),
                username: "alice".to_string(),
                online: true,
            }]),
            PartyResponse::Channels(vec![ChannelInfo {
                id: Uuid::new_v4(),
                name: "general".to_string(),
                kind: ChannelKind::Public,
            }]),
            PartyResponse::MessagePosted {
                channel: Uuid::new_v4(),
                seq: 1,
            },
            PartyResponse::Message(sample_envelope()),
            PartyResponse::History(vec![sample_envelope(), sample_envelope()]),
            PartyResponse::Error("boom".to_string()),
        ];
        for resp in responses {
            let bytes = resp.to_bytes();
            assert_eq!(PartyResponse::from_bytes(&bytes), Some(resp));
        }
    }

    #[test]
    fn malformed_bytes_return_none() {
        assert!(PartyRequest::from_bytes(&[0xff, 0xff, 0xff, 0xff]).is_none());
        assert!(PartyResponse::from_bytes(&[]).is_none());
    }

    #[test]
    fn tier_and_channel_kind_default() {
        assert_eq!(TrustTier::default(), TrustTier::Administered);
        assert_eq!(ChannelKind::default(), ChannelKind::Public);
    }

    #[tokio::test]
    async fn framed_message_roundtrips_over_a_tunnel() {
        let cipher = AesCipher::new(&[3u8; 32]).unwrap();
        let aad = b"transport|party-test";
        let (mut a, mut b) = tokio::io::duplex(4096);

        let req = PartyRequest::PostMessage {
            channel: Uuid::new_v4(),
            text: "hi".to_string(),
        };
        send_framed(&mut a, &cipher, aad, &req.to_bytes())
            .await
            .unwrap();
        let bytes = recv_framed(&mut b, &cipher, aad).await.unwrap();
        assert_eq!(PartyRequest::from_bytes(&bytes), Some(req));

        // A frame decrypted under the wrong AAD must fail to authenticate.
        send_framed(&mut a, &cipher, aad, &PartyRequest::ListMembers.to_bytes())
            .await
            .unwrap();
        assert!(recv_framed(&mut b, &cipher, b"wrong-aad").await.is_err());
    }

    #[tokio::test]
    async fn party_client_connect_split_send_and_recv() {
        use crate::core::generate_rsa_keypair;
        use crate::network::host_handshake;
        use crate::RSA_KEY_BITS;

        let server_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let client_priv = generate_rsa_keypair(RSA_KEY_BITS).unwrap();
        let (mut server_stream, client_stream) = tokio::io::duplex(1 << 16);

        // Minimal server: handshake, then answer one ListMembers with an empty list.
        let server = tokio::spawn(async move {
            let tunnel = host_handshake(&mut server_stream, &server_priv, Uuid::new_v4())
                .await
                .unwrap();
            let bytes = recv_framed(&mut server_stream, &tunnel.cipher, &tunnel.transport_aad)
                .await
                .unwrap();
            assert_eq!(
                PartyRequest::from_bytes(&bytes),
                Some(PartyRequest::ListMembers)
            );
            send_framed(
                &mut server_stream,
                &tunnel.cipher,
                &tunnel.transport_aad,
                &PartyResponse::Members(Vec::new()).to_bytes(),
            )
            .await
            .unwrap();
        });

        let client = PartyClient::connect(client_stream, &client_priv, Uuid::new_v4())
            .await
            .unwrap();
        assert!(!client.server_fingerprint().is_empty());

        let (mut reader, mut writer) = client.split();
        writer.send(&PartyRequest::ListMembers).await.unwrap();
        assert!(matches!(
            reader.recv().await.unwrap(),
            PartyResponse::Members(ref m) if m.is_empty()
        ));
        server.await.unwrap();
    }
}
