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

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use uuid::Uuid;

use crate::core::{recv_packet, send_packet, AesCipher};

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
        ];
        for req in requests {
            let bytes = req.to_bytes();
            assert_eq!(PartyRequest::from_bytes(&bytes), Some(req));
        }
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
}
