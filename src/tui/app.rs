use crate::app::chat_manager::ChatManager;
use crate::identity::Identity;
use crate::types::Config;
use anyhow::Result;
use ratatui::widgets::ListState;
use std::path::PathBuf;
use uuid::Uuid;

pub struct TuiApp {
    pub chat_manager: ChatManager,
    pub chat_list_state: ListState,
    pub chat_ids: Vec<Uuid>,
    pub input_text: String,
    pub message_scroll: u16,
}

impl TuiApp {
    pub fn new() -> Result<Self> {
        let mut chat_manager = ChatManager::new(Config::default());

        let proj_dirs = directories::ProjectDirs::from("com", "chat-p2p", "EncryptedMessenger");

        let (history_path, identity, _is_new_identity) = if let Some(ref dirs) = proj_dirs {
            let data_dir = dirs.data_dir();
            std::fs::create_dir_all(data_dir).ok();

            let (identity, is_new) = Identity::get_or_create(data_dir, "TUI_User")?;
            (data_dir.join("history.json.enc"), identity, is_new)
        } else {
            let identity = Identity::new_with_plaintext("TUI_User".to_string())?;
            (PathBuf::from("history.json.enc"), identity, true)
        };

        if !identity.is_locked() {
            if let Ok(key) = identity.history_key() {
                chat_manager.set_history_key(key);
                if let Err(e) = chat_manager.load_history_auto(&history_path, &key) {
                    tracing::warn!("Failed to load TUI history: {}", e);
                }
            }
        }

        let chat_ids: Vec<Uuid> = chat_manager.chats.keys().copied().collect();
        let mut chat_list_state = ListState::default();
        if !chat_ids.is_empty() {
            chat_list_state.select(Some(0));
        }

        Ok(Self {
            chat_manager,
            chat_list_state,
            chat_ids,
            input_text: String::new(),
            message_scroll: 0,
        })
    }

    pub fn next_chat(&mut self) {
        if self.chat_ids.is_empty() {
            return;
        }
        let i = match self.chat_list_state.selected() {
            Some(i) => {
                if i >= self.chat_ids.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.chat_list_state.select(Some(i));
        self.message_scroll = 0;
    }

    pub fn previous_chat(&mut self) {
        if self.chat_ids.is_empty() {
            return;
        }
        let i = match self.chat_list_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.chat_ids.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.chat_list_state.select(Some(i));
        self.message_scroll = 0;
    }

    pub fn scroll_up(&mut self) {
        self.message_scroll = self.message_scroll.saturating_sub(1);
    }

    pub fn scroll_down(&mut self) {
        self.message_scroll = self.message_scroll.saturating_add(1);
    }

    pub fn send_message(&mut self) {
        if let Some(selected_index) = self.chat_list_state.selected() {
            if let Some(chat_id) = self.chat_ids.get(selected_index) {
                let text_to_send = self.input_text.trim().to_string();
                if !text_to_send.is_empty() {
                    if let Err(e) = self.chat_manager.send_message(*chat_id, text_to_send) {
                        tracing::error!("Failed to send message: {}", e);
                    }
                    self.input_text.clear();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TuiApp;
    use uuid::Uuid;

    #[test]
    fn test_new_app() {
        let app = TuiApp::new().unwrap();
        assert_eq!(app.input_text, "");
        assert_eq!(app.message_scroll, 0);
    }

    #[test]
    fn test_chat_selection() {
        let mut app = TuiApp::new().unwrap();
        app.chat_ids = vec![Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
        app.chat_list_state.select(Some(0));

        app.next_chat();
        assert_eq!(app.chat_list_state.selected(), Some(1));
        app.next_chat();
        assert_eq!(app.chat_list_state.selected(), Some(2));
        app.next_chat();
        assert_eq!(app.chat_list_state.selected(), Some(0)); // Wraps around

        app.previous_chat();
        assert_eq!(app.chat_list_state.selected(), Some(2)); // Wraps around
        app.previous_chat();
        assert_eq!(app.chat_list_state.selected(), Some(1));
        app.previous_chat();
        assert_eq!(app.chat_list_state.selected(), Some(0));
    }

    #[test]
    fn test_chat_selection_empty() {
        let mut app = TuiApp::new().unwrap();
        app.chat_ids = vec![];
        app.chat_list_state.select(None);

        app.next_chat();
        assert_eq!(app.chat_list_state.selected(), None);

        app.previous_chat();
        assert_eq!(app.chat_list_state.selected(), None);
    }

    #[test]
    fn test_scrolling() {
        let mut app = TuiApp::new().unwrap();
        app.message_scroll = 5;

        app.scroll_up();
        assert_eq!(app.message_scroll, 4);
        app.scroll_up();
        assert_eq!(app.message_scroll, 3);

        app.scroll_down();
        assert_eq!(app.message_scroll, 4);

        // Test saturating sub
        app.message_scroll = 0;
        app.scroll_up();
        assert_eq!(app.message_scroll, 0);
    }

    #[test]
    fn test_send_message_clears_input() {
        let mut app = TuiApp::new().unwrap();
        let chat_id = Uuid::new_v4();
        app.chat_manager
            .create_local_chat_for_test(chat_id, "Test Chat".to_string());
        app.chat_ids = vec![chat_id];
        app.chat_list_state.select(Some(0));
        app.input_text = "Hello, world!".to_string();

        app.send_message();

        assert_eq!(app.input_text, "");
    }
}
