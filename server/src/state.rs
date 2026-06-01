//! In-memory Party server state and the pure logic that drives it (Phase 1,
//! slice 1).
//!
//! This is the authoritative runtime model: members, channels, and durable
//! per-channel message history (which is what makes offline buffering work in the
//! Administered tier — a reconnecting member simply fetches history after the last
//! sequence it saw). It is deliberately network-free so it can be unit-tested
//! deterministically; the TCP/handshake runtime (slice 3) drives these methods.

// Several accessors and fields here (directory/channel listing, presence toggling,
// the member fingerprint binding) are exercised by the tests now and consumed by
// the network runtime in the next slice; allow them ahead of that wiring.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use messenger_core::party::{
    ChannelInfo, ChannelKind, Envelope, MemberInfo, MessagePayload, TrustTier,
};
use messenger_core::util::current_timestamp_millis;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Filename of the durable state snapshot under the operator's data dir.
const SNAPSHOT_FILE: &str = "party_state.json";

/// Why a join was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinError {
    WrongPassword,
    UsernameTaken,
    EmptyUsername,
}

impl JoinError {
    pub fn reason(&self) -> &'static str {
        match self {
            JoinError::WrongPassword => "incorrect server password",
            JoinError::UsernameTaken => "username already taken",
            JoinError::EmptyUsername => "username must not be empty",
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Member {
    id: Uuid,
    username: String,
    fingerprint: Option<String>,
    /// Presence is runtime-only and not persisted; members start offline on load.
    #[serde(skip)]
    online: bool,
}

#[derive(Serialize, Deserialize)]
struct Channel {
    id: Uuid,
    name: String,
    kind: ChannelKind,
    /// Durable history; index order is delivery order. `seq` is `index + 1`.
    messages: Vec<Envelope>,
}

/// Owned on-disk snapshot used when loading. Server name/password/tier come from
/// configuration, not the snapshot.
#[derive(Deserialize, Default)]
struct Snapshot {
    members: Vec<Member>,
    channels: Vec<Channel>,
}

/// Borrowing view used when saving, to avoid cloning channel history.
#[derive(Serialize)]
struct SnapshotRef<'a> {
    members: Vec<&'a Member>,
    channels: &'a [Channel],
}

/// The full server state.
pub struct PartyState {
    name: String,
    password: Option<String>,
    tier: TrustTier,
    members: HashMap<Uuid, Member>,
    channels: Vec<Channel>,
    /// When set, mutations are persisted to this snapshot file.
    persist_path: Option<PathBuf>,
}

impl PartyState {
    /// Create a new Administered server with a default `general` channel.
    pub fn new(name: impl Into<String>, password: Option<String>) -> Self {
        let general = Channel {
            id: Uuid::new_v4(),
            name: "general".to_string(),
            kind: ChannelKind::Public,
            messages: Vec::new(),
        };
        Self {
            name: name.into(),
            password,
            tier: TrustTier::Administered,
            members: HashMap::new(),
            channels: vec![general],
            persist_path: None,
        }
    }

    /// Load durable state from `<data_dir>/party_state.json`, or start fresh (with
    /// a default `general` channel) if absent. Subsequent mutations auto-persist to
    /// that file.
    pub fn load(
        name: impl Into<String>,
        password: Option<String>,
        data_dir: &Path,
    ) -> anyhow::Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let path = data_dir.join(SNAPSHOT_FILE);
        let mut state = Self::new(name, password);

        if path.exists() {
            let json = std::fs::read_to_string(&path)?;
            let snapshot: Snapshot = serde_json::from_str(&json)?;
            state.members = snapshot.members.into_iter().map(|m| (m.id, m)).collect();
            // Keep the default `general` channel if the snapshot has none.
            if !snapshot.channels.is_empty() {
                state.channels = snapshot.channels;
            }
        }

        state.persist_path = Some(path);
        Ok(state)
    }

    /// Best-effort write of the durable state to the snapshot file (no-op when no
    /// persist path is configured, e.g. in pure unit tests). Failures are logged
    /// rather than propagated so a transient disk error doesn't drop a live request.
    fn persist(&self) {
        let Some(path) = &self.persist_path else {
            return;
        };
        let snapshot = SnapshotRef {
            members: self.members.values().collect(),
            channels: &self.channels,
        };
        match serde_json::to_string(&snapshot) {
            Ok(json) => {
                if let Err(e) = std::fs::write(path, json) {
                    tracing::error!(error = %e, path = %path.display(), "failed to persist party state");
                }
            }
            Err(e) => tracing::error!(error = %e, "failed to serialize party state"),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn tier(&self) -> TrustTier {
        self.tier
    }

    /// The id of the default channel (`general`).
    pub fn default_channel(&self) -> Uuid {
        self.channels[0].id
    }

    /// Join the server: validates the password and a unique, non-empty username,
    /// then registers the member as online and returns its id.
    pub fn join(
        &mut self,
        username: &str,
        password: Option<&str>,
        fingerprint: Option<String>,
    ) -> Result<Uuid, JoinError> {
        let username = username.trim();
        if username.is_empty() {
            return Err(JoinError::EmptyUsername);
        }
        if self.password.as_deref() != password {
            return Err(JoinError::WrongPassword);
        }
        if self
            .members
            .values()
            .any(|m| m.username.eq_ignore_ascii_case(username))
        {
            return Err(JoinError::UsernameTaken);
        }

        let id = Uuid::new_v4();
        self.members.insert(
            id,
            Member {
                id,
                username: username.to_string(),
                fingerprint,
                online: true,
            },
        );
        self.persist();
        Ok(id)
    }

    /// Mark a member's presence. Used when a connection drops or reconnects.
    pub fn set_online(&mut self, member: Uuid, online: bool) {
        if let Some(m) = self.members.get_mut(&member) {
            m.online = online;
        }
    }

    pub fn is_member(&self, member: Uuid) -> bool {
        self.members.contains_key(&member)
    }

    /// The member directory, sorted by username for stable output.
    pub fn members(&self) -> Vec<MemberInfo> {
        let mut list: Vec<MemberInfo> = self
            .members
            .values()
            .map(|m| MemberInfo {
                id: m.id,
                username: m.username.clone(),
                online: m.online,
            })
            .collect();
        list.sort_by(|a, b| a.username.cmp(&b.username));
        list
    }

    pub fn channels(&self) -> Vec<ChannelInfo> {
        self.channels
            .iter()
            .map(|c| ChannelInfo {
                id: c.id,
                name: c.name.clone(),
                kind: c.kind,
            })
            .collect()
    }

    fn channel_mut(&mut self, id: Uuid) -> Option<&mut Channel> {
        self.channels.iter_mut().find(|c| c.id == id)
    }

    fn channel(&self, id: Uuid) -> Option<&Channel> {
        self.channels.iter().find(|c| c.id == id)
    }

    /// Append a text message from `sender` to `channel`, assigning the next
    /// per-channel sequence number. The message is stored durably (offline
    /// buffering) and the assigned envelope is returned for broadcast.
    ///
    /// Errors if the sender is not a member or the channel does not exist.
    pub fn post_message(
        &mut self,
        sender: Uuid,
        channel: Uuid,
        text: String,
    ) -> Result<Envelope, String> {
        if !self.is_member(sender) {
            return Err("sender is not a member of this server".to_string());
        }
        let tier = self.tier;
        let chan = self
            .channel_mut(channel)
            .ok_or_else(|| "unknown channel".to_string())?;
        let seq = chan.messages.len() as u64 + 1;
        let envelope = Envelope {
            tier,
            sender,
            channel,
            seq,
            timestamp: current_timestamp_millis(),
            payload: MessagePayload::Text(text),
        };
        chan.messages.push(envelope.clone());
        self.persist();
        Ok(envelope)
    }

    /// Channel history strictly after `since_seq` (offline catch-up).
    /// `since_seq = 0` returns the entire channel. Unknown channels yield `[]`.
    pub fn history_since(&self, channel: Uuid, since_seq: u64) -> Vec<Envelope> {
        match self.channel(channel) {
            Some(c) => c
                .messages
                .iter()
                .filter(|m| m.seq > since_seq)
                .cloned()
                .collect(),
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_server_join_succeeds_and_lists_member() {
        let mut state = PartyState::new("Open", None);
        let id = state.join("alice", None, None).expect("join");
        assert!(state.is_member(id));
        let members = state.members();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].username, "alice");
        assert!(members[0].online);
    }

    #[test]
    fn password_protected_join_enforced() {
        let mut state = PartyState::new("Locked", Some("s3cret".to_string()));
        assert_eq!(
            state.join("alice", None, None),
            Err(JoinError::WrongPassword)
        );
        assert_eq!(
            state.join("alice", Some("nope"), None),
            Err(JoinError::WrongPassword)
        );
        assert!(state.join("alice", Some("s3cret"), None).is_ok());
    }

    #[test]
    fn duplicate_and_empty_usernames_rejected() {
        let mut state = PartyState::new("Open", None);
        state.join("alice", None, None).unwrap();
        assert_eq!(
            state.join("Alice", None, None),
            Err(JoinError::UsernameTaken),
            "username uniqueness is case-insensitive"
        );
        assert_eq!(state.join("   ", None, None), Err(JoinError::EmptyUsername));
    }

    #[test]
    fn posting_assigns_monotonic_per_channel_seq() {
        let mut state = PartyState::new("Open", None);
        let alice = state.join("alice", None, None).unwrap();
        let chan = state.default_channel();

        let m1 = state
            .post_message(alice, chan, "first".to_string())
            .unwrap();
        let m2 = state
            .post_message(alice, chan, "second".to_string())
            .unwrap();
        assert_eq!(m1.seq, 1);
        assert_eq!(m2.seq, 2);
        assert_eq!(m2.sender, alice);
        assert_eq!(m2.payload, MessagePayload::Text("second".to_string()));
    }

    #[test]
    fn posting_rejects_non_members_and_unknown_channels() {
        let mut state = PartyState::new("Open", None);
        let stranger = Uuid::new_v4();
        let chan = state.default_channel();
        assert!(state.post_message(stranger, chan, "x".to_string()).is_err());

        let alice = state.join("alice", None, None).unwrap();
        assert!(state
            .post_message(alice, Uuid::new_v4(), "x".to_string())
            .is_err());
    }

    #[test]
    fn history_since_supports_offline_catch_up() {
        let mut state = PartyState::new("Open", None);
        let alice = state.join("alice", None, None).unwrap();
        let chan = state.default_channel();
        for i in 0..5 {
            state.post_message(alice, chan, format!("msg {i}")).unwrap();
        }

        // Whole channel.
        assert_eq!(state.history_since(chan, 0).len(), 5);
        // A member who last saw seq=3 gets only 4 and 5.
        let missed = state.history_since(chan, 3);
        assert_eq!(missed.len(), 2);
        assert_eq!(missed[0].seq, 4);
        assert_eq!(missed[1].seq, 5);
        // Caught up: nothing new.
        assert!(state.history_since(chan, 5).is_empty());
        // Unknown channel.
        assert!(state.history_since(Uuid::new_v4(), 0).is_empty());
    }

    #[test]
    fn presence_can_be_toggled() {
        let mut state = PartyState::new("Open", None);
        let alice = state.join("alice", None, None).unwrap();
        state.set_online(alice, false);
        assert!(!state.members()[0].online);
        state.set_online(alice, true);
        assert!(state.members()[0].online);
    }

    #[test]
    fn durable_state_persists_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let channel;
        {
            let mut state = PartyState::load("Srv", None, dir.path()).unwrap();
            channel = state.default_channel();
            let alice = state
                .join("alice", None, Some("FINGERPRINT".to_string()))
                .unwrap();
            state
                .post_message(alice, channel, "persisted msg".to_string())
                .unwrap();
        } // dropped; everything was written through to disk on each mutation

        let reloaded = PartyState::load("Srv", None, dir.path()).unwrap();
        // Member directory survived; presence resets to offline after reload.
        let members = reloaded.members();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].username, "alice");
        assert!(!members[0].online, "presence must reset to offline on load");
        // Channel identity + history survived.
        assert_eq!(reloaded.default_channel(), channel);
        let history = reloaded.history_since(channel, 0);
        assert_eq!(history.len(), 1);
        assert_eq!(
            history[0].payload,
            MessagePayload::Text("persisted msg".to_string())
        );
    }

    #[test]
    fn load_without_existing_snapshot_starts_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let state = PartyState::load("Srv", None, dir.path()).unwrap();
        assert!(state.members().is_empty());
        assert_eq!(state.channels().len(), 1); // default `general`
    }
}
