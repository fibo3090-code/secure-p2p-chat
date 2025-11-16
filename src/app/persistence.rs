use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::types::{Chat, Config};

/// History file format for JSON serialization
#[derive(Serialize, Deserialize)]
pub struct HistoryFile {
    pub version: String,
    pub chats: Vec<Chat>,
    pub contacts: Vec<crate::types::Contact>,
    #[serde(default)]
    pub config: Config,
}

impl HistoryFile {
    pub fn new(chats: Vec<Chat>) -> Self {
        Self {
            version: "1.0".to_string(),
            chats,
            contacts: Vec::new(),
            config: Config::default(),
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

        // Load persisted config (if present)
        self.config = history.config;

        Ok(())
    }

    /// Save chat history to file
    pub fn save_history(&self, path: &Path) -> Result<()> {
        let mut history = HistoryFile::new(self.chats.values().cloned().collect());
        history.contacts = self.contacts.values().cloned().collect();
        history.config = self.config.clone();
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
}
