use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// A chat session with a peer
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Chat {
    pub id: Uuid,
    pub title: String,
    pub peer_fingerprint: Option<String>,
    /// Participants (references to Contact IDs). Empty for one-to-one until contact added.
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
}

/// A single message in a chat
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Message {
    pub id: Uuid,
    pub from_me: bool,
    pub content: MessageContent,
    pub timestamp: DateTime<Utc>,
}

/// A contact (a known peer)
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Contact {
    pub id: Uuid,
    pub name: String,
    pub address: Option<String>,
    pub fingerprint: Option<String>,
    pub public_key: Option<String>,
    pub created_at: DateTime<Utc>,

    // Contacts 2.0 Fields
    #[serde(default)]
    pub trust_state: TrustState,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub last_seen: Option<DateTime<Utc>>,
}

/// Trust level for a contact
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrustState {
    #[default]
    Unverified,
    Verified,
    Trusted,
    Blocked,
}

/// Message content types
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum MessageContent {
    #[serde(rename = "text")]
    Text {
        text: String,
    },

    #[serde(rename = "file")]
    File {
        filename: String,
        size: u64,
        path: Option<PathBuf>,
    },
    Edited {
        new_text: String,
    },
}

/// Toast notification for UI
#[derive(Debug, Clone)]
pub struct Toast {
    pub id: Uuid,
    pub level: ToastLevel,
    pub message: String,
    pub created_at: std::time::Instant,
    pub duration: std::time::Duration,
}

/// Toast severity level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastLevel {
    Info,
    Success,
    Warning,
    Error,
}

/// File transfer state
#[derive(Debug, Clone)]
pub struct FileTransferState {
    pub id: Uuid,
    pub chat_id: Uuid,
    pub filename: String,
    pub size: u64,
    pub received: u64,
    pub status: TransferStatus,
    pub seq: u64,
}

/// File transfer status
#[derive(Debug, Clone, PartialEq)]
pub enum TransferStatus {
    Pending,
    InProgress,
    Completed,
    Failed(String),
    Cancelled,
}

/// Session role
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRole {
    Host,
    Client,
}

/// Session status
#[derive(Debug, Clone, PartialEq)]
pub enum SessionStatus {
    Connecting,
    Handshaking,
    FingerprintPending,
    Active,
    Disconnected,
    Error(String),
}

/// Events sent from network session to app
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
        chat_id: Uuid,
    },
    ShowFingerprintVerification {
        fingerprint: String,
        peer_name: String,
        chat_id: Uuid,
    },
    Ready,
    MessageReceived(crate::core::ProtocolMessage),
    Disconnected,
    Error(String),
    Warning(String),
}

/// Application configuration
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
    pub notification_sound: NotificationSound,
    // Auto-host settings
    #[serde(default)]
    pub auto_host_on_startup: bool,
    #[serde(default = "default_listen_port")]
    pub listen_port: u16,
    #[serde(default)]
    pub auto_trust_on_first_use: bool,
    /// Enable broadcasting and discovery via mDNS (Bonjour).
    /// Disabled by default for privacy; enabling will advertise a hostname
    /// and a fingerprint on the local network.
    #[serde(default)]
    pub enable_mdns: bool,
}

/// Theme options
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    Light,
    Dark,
    Midnight,
    Forest,
}

/// Notification sound options
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotificationSound {
    None,
    Default,
    // Vibrate (for mobile, if applicable)
}

impl Default for Config {
    fn default() -> Self {
        Self {
            download_dir: PathBuf::from("Downloads"),
            temp_dir: PathBuf::from("temp"),
            auto_accept_files: false,
            enable_notifications: true,
            enable_typing_indicators: true,
            show_log_terminal: false,
            theme: Theme::Dark,
            font_size: 14,
            auto_connect: false,
            notification_sound: NotificationSound::Default,
            auto_host_on_startup: false,
            listen_port: 5000,
            auto_trust_on_first_use: false,
            enable_mdns: false,
        }
    }
}

fn default_listen_port() -> u16 {
    5000
}
