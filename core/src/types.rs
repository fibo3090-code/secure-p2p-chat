use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Chat {
    pub id: Uuid,
    pub title: String,
    pub peer_fingerprint: Option<String>,
    /// Contact IDs; empty for one-to-one until a contact is added.
    pub participants: Vec<Uuid>,
    pub messages: Vec<Message>,
    pub created_at: DateTime<Utc>,
    #[serde(skip)]
    pub send_seq: u64,
    #[serde(skip)]
    pub recv_seq: u64,
    #[serde(skip)]
    pub peer_typing: bool,
    #[serde(skip)]
    pub typing_since: Option<std::time::Instant>,
    /// True if this chat is a placeholder for a listening host, not a real conversation.
    #[serde(default)]
    pub is_host_placeholder: bool,
    /// What kind of conversation this is (DM / flat group / community channel).
    #[serde(default)]
    pub kind: ChatKind,
    /// How bytes reach the peer(s) for this conversation (orthogonal to `kind`).
    #[serde(default)]
    pub transport: Transport,
    /// How many `messages` the user has seen. Persisted so unread survives a
    /// restart; never seed it from the live count. Old history loads as `0`
    /// (everything unread), which is the conservative side.
    #[serde(default)]
    pub read_count: usize,
    /// Set once the user renames the chat, so `Connected` stops relabelling
    /// it with the peer address.
    #[serde(default)]
    pub title_is_custom: bool,
}

impl Chat {
    /// A new, empty conversation (no fingerprint, so TOFU runs). `kind` and
    /// `transport` are required because they drive the UI badges; guessing
    /// them once persisted a relay chat as direct.
    pub fn new(id: Uuid, title: impl Into<String>, kind: ChatKind, transport: Transport) -> Self {
        Self {
            id,
            title: title.into(),
            kind,
            transport,
            peer_fingerprint: None,
            participants: Vec::new(),
            messages: Vec::new(),
            created_at: Utc::now(),
            peer_typing: false,
            typing_since: None,
            send_seq: 0,
            recv_seq: 0,
            is_host_placeholder: false,
            read_count: 0,
            title_is_custom: false,
        }
    }

    /// Accept `seq` if it climbs strictly above the last accepted one on this
    /// chat (the app-layer half of replay protection), else refuse a replay.
    pub fn accept_seq(&mut self, seq: u64) -> bool {
        if seq <= self.recv_seq {
            return false;
        }
        self.recv_seq = seq;
        true
    }

    /// Reset both counters when a **new** session attaches to this chat: the
    /// wire seq restarts at 1, so a stale `recv_seq` would discard the peer's
    /// first messages as replays. Replay protection still holds per session
    /// in `run_message_loop`.
    pub fn reset_sequences(&mut self) {
        self.recv_seq = 0;
        self.send_seq = 0;
    }

    pub fn clear_typing(&mut self) {
        self.peer_typing = false;
        self.typing_since = None;
    }

    /// Messages received from the peer that the user has not seen yet. Own
    /// messages never count, so sending never badges your own conversation.
    pub fn unread_count(&self) -> usize {
        self.messages
            .get(self.read_count.min(self.messages.len())..)
            .map(|tail| tail.iter().filter(|m| !m.from_me).count())
            .unwrap_or(0)
    }

    /// Mark every message currently in the conversation as seen.
    pub fn mark_read(&mut self) {
        self.read_count = self.messages.len();
    }
}

/// The kind of conversation, independent of how it is transported.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChatKind {
    /// One-to-one direct message.
    #[default]
    Dm,
    /// A flat group of 3+ peers (no channels).
    Group,
    /// A channel inside a community / server.
    Channel,
}

/// How a conversation's bytes reach the peer(s); orthogonal to [`ChatKind`].
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Transport {
    /// Direct peer-to-peer TCP (one peer exposes a port).
    #[default]
    Direct,
    /// Brokered through a relay (no port-forwarding, blind pipe).
    Relay,
    /// Terminated by a server that holds the conversation.
    Server,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Message {
    pub id: Uuid,
    pub from_me: bool,
    pub content: MessageContent,
    pub timestamp: DateTime<Utc>,
    /// Set once the peer acknowledged receipt (sent messages only; stays
    /// `false` for old peers and old history).
    #[serde(default)]
    pub delivered: bool,
}

impl Message {
    /// A message this device sent; `id` is what the delivery receipt matches.
    pub fn sent(id: Uuid, content: MessageContent) -> Self {
        Self {
            id,
            from_me: true,
            content,
            timestamp: Utc::now(),
            delivered: false,
        }
    }

    /// A message the peer sent, stamped with when it was written.
    pub fn received(content: MessageContent, timestamp: DateTime<Utc>) -> Self {
        Self {
            id: Uuid::new_v4(),
            from_me: false,
            content,
            timestamp,
            delivered: false,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Contact {
    pub id: Uuid,
    pub name: String,
    pub address: Option<String>,
    /// Extra direct-connect candidates in priority order; `address` stays the
    /// primary for back-compat.
    #[serde(default)]
    pub addresses: Vec<String>,
    #[serde(default)]
    pub relay_server: Option<String>,
    #[serde(default)]
    pub relay_token: Option<String>,
    pub fingerprint: Option<String>,
    pub public_key: Option<String>,
    pub created_at: DateTime<Utc>,

    #[serde(default)]
    pub trust_state: TrustState,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub last_seen: Option<DateTime<Utc>>,
}

impl Contact {
    /// Direct-connect candidates in priority order, falling back to the
    /// legacy single `address`.
    pub fn candidate_addresses(&self) -> Vec<String> {
        if self.addresses.is_empty() {
            self.address.iter().cloned().collect()
        } else {
            self.addresses.clone()
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrustState {
    #[default]
    Unverified,
    Verified,
    Trusted,
    Blocked,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum MessageContent {
    #[serde(rename = "text")]
    Text { text: String },

    #[serde(rename = "file")]
    File {
        filename: String,
        size: u64,
        path: Option<PathBuf>,
    },
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub id: Uuid,
    pub level: ToastLevel,
    pub message: String,
    pub created_at: std::time::Instant,
    pub duration: std::time::Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastLevel {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferDirection {
    Incoming,
    Outgoing,
}

#[derive(Debug, Clone)]
pub struct FileTransferState {
    pub id: Uuid,
    pub chat_id: Uuid,
    pub filename: String,
    pub size: u64,
    /// Bytes transferred so far (received for incoming, sent for outgoing).
    pub received: u64,
    pub status: TransferStatus,
    pub seq: u64,
    pub direction: TransferDirection,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TransferStatus {
    Pending,
    /// Incoming transfer held until the user accepts (`auto_accept_files`
    /// off); chunks spool meanwhile, declining deletes the spool.
    AwaitingAcceptance,
    InProgress,
    Completed,
    Failed(String),
    Cancelled,
}

impl TransferStatus {
    /// Still running, including an offer awaiting the user's decision.
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Pending | Self::AwaitingAcceptance | Self::InProgress
        )
    }
}

/// Events sent from a network session to the app.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    Listening {
        port: u16,
    },
    Connected {
        peer: String,
    },
    NewConnection {
        peer_addr: String,
        fingerprint: String,
        /// Short authentication string (same on both ends unless MITM'd).
        sas: String,
        chat_id: Uuid,
    },
    ShowFingerprintVerification {
        fingerprint: String,
        peer_name: String,
        /// Short authentication string for this session (see `NewConnection`).
        sas: String,
        chat_id: Uuid,
    },
    Ready,
    MessageReceived(crate::core::ProtocolMessage),
    /// The outgoing transfer's final `FileEnd` (this seq) hit the wire; only
    /// now is it complete.
    FileSendComplete {
        seq: u64,
    },
    /// An outgoing text frame (or a large one's last chunk) was written with
    /// this seq, used to match the peer's later `Ack { acked_seq }`.
    TextSendComplete {
        seq: u64,
    },
    Disconnected,
    Error(String),
    Warning(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub download_dir: PathBuf,
    pub temp_dir: PathBuf,
    pub auto_accept_files: bool,
    pub enable_notifications: bool,
    pub enable_typing_indicators: bool,
    pub show_log_terminal: bool,
    pub theme: Theme,
    pub font_size: u8,
    pub auto_connect: bool,
    #[serde(default)]
    pub auto_host_on_startup: bool,
    #[serde(default = "default_listen_port")]
    pub listen_port: u16,
    #[serde(default)]
    pub auto_trust_on_first_use: bool,
    /// Advertise and discover via mDNS. Off by default: it reveals a hostname
    /// and fingerprint on the LAN.
    #[serde(default)]
    pub enable_mdns: bool,
    /// Map the listening port via UPnP/IGD and embed the external address in
    /// invites. Off by default: it opens a router port and reveals the IP.
    #[serde(default)]
    pub enable_upnp: bool,
    #[serde(default)]
    pub relay_server: Option<String>,
    /// RFC 3339 time of the last identity backup, so the app can say "never
    /// backed up".
    #[serde(default)]
    pub identity_backed_up_at: Option<String>,
}

/// Theme options
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    Light,
    Dark,
    Midnight,
    Forest,
    Rose,
}

/// The user's real Downloads directory (absolute; the old relative default
/// resolved against the working directory).
pub fn default_download_dir() -> PathBuf {
    if let Some(dirs) = directories::UserDirs::new() {
        if let Some(dl) = dirs.download_dir() {
            return dl.to_path_buf();
        }
        return dirs.home_dir().join("Downloads");
    }
    PathBuf::from("Downloads")
}

/// A per-app scratch directory under the OS temp dir.
pub fn default_temp_dir() -> PathBuf {
    std::env::temp_dir().join("chat-p2p")
}

impl Default for Config {
    fn default() -> Self {
        Self {
            download_dir: default_download_dir(),
            temp_dir: default_temp_dir(),
            auto_accept_files: false,
            enable_notifications: true,
            enable_typing_indicators: true,
            show_log_terminal: false,
            theme: Theme::Dark,
            font_size: 14,
            auto_connect: false,
            auto_host_on_startup: false,
            listen_port: crate::PORT_DEFAULT,
            auto_trust_on_first_use: false,
            enable_mdns: false,
            enable_upnp: false,
            relay_server: None,
            identity_backed_up_at: None,
        }
    }
}

fn default_listen_port() -> u16 {
    crate::PORT_DEFAULT
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Chat as persisted before kind/transport/read_count/title_is_custom.
    fn old_chat() -> Chat {
        serde_json::from_str(
            r#"{
                "id":"00000000-0000-0000-0000-000000000000",
                "title":"old","peer_fingerprint":null,"participants":[],
                "messages":[],"created_at":"2020-01-01T00:00:00Z"
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn config_default_is_privacy_conservative() {
        let c = Config::default();
        assert!(
            !c.auto_accept_files,
            "files must not auto-accept by default"
        );
        assert!(
            !c.auto_trust_on_first_use,
            "TOFU auto-trust must be off by default"
        );
        assert!(!c.enable_mdns, "mDNS advertising must be off by default");
        assert!(!c.enable_upnp, "UPnP port mapping must be off by default");
        assert!(!c.auto_host_on_startup);
        assert!(!c.auto_connect);
        assert_eq!(c.listen_port, crate::PORT_DEFAULT);
        assert_eq!(c.theme, Theme::Dark);
        assert!(c.relay_server.is_none());
    }

    #[test]
    fn config_serde_roundtrip_preserves_fields() {
        let c = Config {
            theme: Theme::Forest,
            font_size: 18,
            enable_mdns: true,
            enable_upnp: true,
            relay_server: Some("relay.example:9000".to_string()),
            ..Config::default()
        };
        let json = serde_json::to_string(&c).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back.theme, Theme::Forest);
        assert_eq!(back.font_size, 18);
        assert!(back.enable_mdns);
        assert!(back.enable_upnp);
        assert_eq!(back.relay_server.as_deref(), Some("relay.example:9000"));
    }

    #[test]
    fn config_deserializes_with_optional_fields_missing() {
        // Predates the #[serde(default)] fields; the retired
        // `notification_sound` key must still be tolerated.
        let json = r#"{
            "download_dir": "Downloads",
            "temp_dir": "temp",
            "auto_accept_files": false,
            "enable_notifications": true,
            "enable_typing_indicators": true,
            "show_log_terminal": false,
            "theme": "Light",
            "font_size": 14,
            "auto_connect": false,
            "notification_sound": "Default"
        }"#;
        let c: Config = serde_json::from_str(json).unwrap();
        assert_eq!(c.theme, Theme::Light);
        assert!(!c.auto_host_on_startup);
        assert_eq!(c.listen_port, crate::PORT_DEFAULT);
        assert!(!c.auto_trust_on_first_use);
        assert!(!c.enable_mdns);
        assert!(!c.enable_upnp);
        assert!(c.relay_server.is_none());
    }

    #[test]
    fn trust_state_default_is_unverified() {
        assert_eq!(TrustState::default(), TrustState::Unverified);
    }

    #[test]
    fn chat_kind_transport_defaults_and_back_compat() {
        assert_eq!(ChatKind::default(), ChatKind::Dm);
        assert_eq!(Transport::default(), Transport::Direct);
        // A Chat persisted before these fields existed loads with the defaults.
        let c = old_chat();
        assert_eq!(c.kind, ChatKind::Dm);
        assert_eq!(c.transport, Transport::Direct);
        assert!(!c.is_host_placeholder);
        let mut c2 = c.clone();
        c2.kind = ChatKind::Group;
        c2.transport = Transport::Relay;
        let back: Chat = serde_json::from_str(&serde_json::to_string(&c2).unwrap()).unwrap();
        assert_eq!(back.kind, ChatKind::Group);
        assert_eq!(back.transport, Transport::Relay);
    }

    /// The read mark survives save/load, or messages that arrived while the
    /// app was closed are silently marked as seen.
    #[test]
    fn read_count_persists_and_defaults_for_old_history() {
        let old = old_chat();
        assert_eq!(old.read_count, 0, "old history loads as fully unread");

        let mut c = old.clone();
        c.read_count = 7;
        let back: Chat = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back.read_count, 7);
    }

    /// A rename survives save/load; old history loads without one.
    #[test]
    fn title_is_custom_persists_and_defaults_for_old_history() {
        let old = old_chat();
        assert!(!old.title_is_custom);
        let mut c = old.clone();
        c.title = "Mum".to_string();
        c.title_is_custom = true;
        let back: Chat = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert!(back.title_is_custom);
        assert_eq!(back.title, "Mum");
    }

    #[test]
    fn unread_counts_only_unseen_peer_messages() {
        fn msg(from_me: bool) -> Message {
            let text = MessageContent::Text {
                text: "hi".to_string(),
            };
            if from_me {
                Message::sent(Uuid::new_v4(), text)
            } else {
                Message::received(text, Utc::now())
            }
        }
        let mut chat = old_chat();
        chat.messages.push(msg(false));
        chat.messages.push(msg(false));
        assert_eq!(chat.unread_count(), 2);

        chat.mark_read();
        assert_eq!(chat.unread_count(), 0);

        // Own messages never badge the conversation you just typed in.
        chat.messages.push(msg(true));
        assert_eq!(chat.unread_count(), 0);

        chat.messages.push(msg(false));
        assert_eq!(chat.unread_count(), 1);

        // A read mark past the end must saturate, not panic.
        chat.read_count = 99;
        assert_eq!(chat.unread_count(), 0);
    }

    #[test]
    fn theme_serde_roundtrip() {
        for theme in [
            Theme::Light,
            Theme::Dark,
            Theme::Midnight,
            Theme::Forest,
            Theme::Rose,
        ] {
            let json = serde_json::to_string(&theme).unwrap();
            let back: Theme = serde_json::from_str(&json).unwrap();
            assert_eq!(theme, back);
        }
    }

    #[test]
    fn message_content_serde_is_tagged() {
        let file = MessageContent::File {
            filename: "a.bin".to_string(),
            size: 10,
            path: None,
        };
        let json = serde_json::to_string(&file).unwrap();
        assert!(json.contains("\"type\":\"file\""));
        let text = MessageContent::Text {
            text: "hi".to_string(),
        };
        let json = serde_json::to_string(&text).unwrap();
        assert!(json.contains("\"type\":\"text\""));
    }

    #[test]
    fn contact_deserializes_without_contacts_2_0_fields() {
        let json = format!(
            r#"{{"id":"{}","name":"Bob","address":null,"fingerprint":null,
                 "public_key":null,"created_at":"2020-01-01T00:00:00Z","last_seen":null}}"#,
            Uuid::nil()
        );
        let c: Contact = serde_json::from_str(&json).unwrap();
        assert_eq!(c.name, "Bob");
        assert_eq!(c.trust_state, TrustState::Unverified);
        assert!(c.notes.is_empty());
        assert!(c.tags.is_empty());
        assert!(c.relay_server.is_none());
    }
}
