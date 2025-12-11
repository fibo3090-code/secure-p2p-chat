use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::types::{Chat, Config};
use uuid::Uuid;

/// History file format for JSON serialization
#[derive(Serialize, Deserialize)]
pub struct HistoryFile {
    pub version: String,
    pub chats: Vec<Chat>,
    pub contacts: Vec<crate::types::Contact>,
    #[serde(default)]
    pub config: Config,
    /// Contact -> chat association map so we can reconnect automatically
    #[serde(default)]
    pub contact_chat_map: Vec<(Uuid, Uuid)>,
}

impl HistoryFile {
    pub fn new(chats: Vec<Chat>) -> Self {
        Self {
            version: "1.0".to_string(),
            chats,
            contacts: Vec::new(),
            config: Config::default(),
            contact_chat_map: Vec::new(),
        }
    }

    /// Load history from JSON file
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let history: HistoryFile = serde_json::from_str(&content)?;

        if history.version != "1.0" {
            anyhow::bail!("Unsupported history version: {}", history.version);
        }

        tracing::info!("Loaded {} chats from history", history.chats.len());
        Ok(history)
    }

    /// Save history to JSON file
    pub fn save(&self, path: &Path) -> Result<()> {
        // Create parent directory if it doesn't exist
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = serde_json::to_string_pretty(&self)?;
        std::fs::write(path, content)?;

        tracing::info!("Saved {} chats to history", self.chats.len());
        Ok(())
    }
}

use crate::app::ChatManager;

impl ChatManager {
    /// Load chat history from file
    pub fn load_history(&mut self, path: &Path) -> Result<()> {
        let history = HistoryFile::load(path)?;

        for chat in history.chats {
            self.chats.insert(chat.id, chat);
        }

        for contact in history.contacts {
            self.contacts.insert(contact.id, contact);
        }

        // Restore persisted contact/chat associations for reconnect
        self.contact_to_chat
            .extend(history.contact_chat_map.into_iter());

        // Load persisted config (if present)
        self.config = history.config;

        Ok(())
    }

    /// Save chat history to file
    pub fn save_history(&self, path: &Path) -> Result<()> {
        let mut history = HistoryFile::new(self.chats.values().cloned().collect());
        history.contacts = self.contacts.values().cloned().collect();
        history.config = self.config.clone();
        history.contact_chat_map = self.contact_to_chat.iter().map(|(k, v)| (*k, *v)).collect();
        history.save(path)
    }

    /// Auto-save to default location
    pub fn auto_save(&self) -> Result<()> {
        let path = self.config.download_dir.join("history.json");
        self.save_history(&path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ChatManager;
    use tempfile::NamedTempFile;
    use uuid::Uuid;

    #[test]
    fn test_history_roundtrip() {
        let temp_file = NamedTempFile::new().unwrap();

        let chat = Chat {
            id: Uuid::new_v4(),
            title: "Test Chat".to_string(),
            peer_fingerprint: Some("abc123".to_string()),
            participants: Vec::new(),
            messages: Vec::new(),
            created_at: chrono::Utc::now(),
            peer_typing: false,
            typing_since: None,
        };

        let history = HistoryFile::new(vec![chat.clone()]);

        // Save
        history.save(temp_file.path()).unwrap();

        // Load
        let loaded = HistoryFile::load(temp_file.path()).unwrap();

        assert_eq!(loaded.version, "1.0");
        assert_eq!(loaded.chats.len(), 1);
        assert_eq!(loaded.chats[0].id, chat.id);
        assert_eq!(loaded.chats[0].title, chat.title);
    }

    #[test]
    fn history_persists_contact_mapping() {
        let temp_file = NamedTempFile::new().unwrap();

        let mut manager = ChatManager::new(Config::default());
        let chat_id = Uuid::new_v4();
        let contact_id = Uuid::new_v4();

        manager.chats.insert(
            chat_id,
            Chat {
                id: chat_id,
                title: "Chat".into(),
                peer_fingerprint: None,
                participants: Vec::new(),
                messages: Vec::new(),
                created_at: chrono::Utc::now(),
                peer_typing: false,
                typing_since: None,
            },
        );

        manager.contacts.insert(
            contact_id,
            crate::types::Contact {
                id: contact_id,
                name: "Alice".into(),
                address: Some("127.0.0.1:5000".into()),
                fingerprint: None,
                public_key: None,
                created_at: chrono::Utc::now(),
                trust_state: crate::types::TrustState::Unverified,
                notes: String::new(),
                tags: Vec::new(),
                last_seen: None,
            },
        );

        manager.associate_contact_with_chat(contact_id, chat_id);
        manager.save_history(temp_file.path()).unwrap();

        let mut reloaded = ChatManager::new(Config::default());
        reloaded.load_history(temp_file.path()).unwrap();

        assert_eq!(reloaded.contact_to_chat.get(&contact_id), Some(&chat_id));
    }
}
