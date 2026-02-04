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
    use crate::app::chat_manager::SessionHandle;
    use tokio::sync::mpsc;
    use uuid::Uuid;

    use crate::core::ProtocolMessage;

    // Helper to setup a chat that can actually receive messages (has a session)
    // We return the receiver so it doesn't get dropped (which closes the channel)
    fn setup_chat_with_session(
        app: &mut TuiApp,
        name: &str,
    ) -> (Uuid, mpsc::UnboundedReceiver<ProtocolMessage>) {
        let chat_id = Uuid::new_v4();
        app.chat_manager
            .create_local_chat_for_test(chat_id, name.to_string());

        // Create dummy session
        let (tx, rx) = mpsc::unbounded_channel();
        let session = SessionHandle { from_app_tx: tx };
        app.chat_manager.add_session_for_test(chat_id, session);

        app.chat_ids = vec![chat_id];
        app.chat_list_state.select(Some(0));
        (chat_id, rx)
    }

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
        let (_chat_id, _rx) = setup_chat_with_session(&mut app, "Test Chat");
        app.input_text = "Hello, world!".to_string();

        app.send_message();

        assert_eq!(app.input_text, "");
    }

    #[test]
    fn test_input_text_append() {
        let mut app = TuiApp::new().unwrap();
        app.input_text = "Hello".to_string();

        app.input_text.push('!');
        assert_eq!(app.input_text, "Hello!");

        app.input_text.push_str(" World");
        assert_eq!(app.input_text, "Hello! World");
    }

    #[test]
    fn test_input_text_delete() {
        let mut app = TuiApp::new().unwrap();
        app.input_text = "Test".to_string();

        app.input_text.pop();
        assert_eq!(app.input_text, "Tes");

        app.input_text.pop();
        app.input_text.pop();
        app.input_text.pop();
        assert_eq!(app.input_text, "");

        // Pop empty string should be safe
        app.input_text.pop();
        assert_eq!(app.input_text, "");
    }

    #[test]
    fn test_send_empty_message_noop() {
        let mut app = TuiApp::new().unwrap();
        let (chat_id, _rx) = setup_chat_with_session(&mut app, "Test");

        // 1. Empty string
        app.input_text = "".to_string();
        app.send_message();
        let chat = app.chat_manager.chats.get(&chat_id).unwrap();
        assert!(chat.messages.is_empty());

        // 2. Whitespace only
        app.input_text = "   ".to_string();
        app.send_message();
        let chat = app.chat_manager.chats.get(&chat_id).unwrap();
        assert!(chat.messages.is_empty());

        assert_eq!(app.input_text, "   ");
    }

    #[test]
    fn test_send_message_adds_to_history() {
        let mut app = TuiApp::new().unwrap();
        let (chat_id, _rx) = setup_chat_with_session(&mut app, "Test");

        app.input_text = "Message 1".to_string();

        // Use a wrapper to check for result
        if let Some(selected_index) = app.chat_list_state.selected() {
            if let Some(cid) = app.chat_ids.get(selected_index) {
                let res = app.chat_manager.send_message(*cid, app.input_text.clone());
                assert!(res.is_ok(), "send_message failed: {:?}", res.err());
            } else {
                panic!("chat_id not found in app.chat_ids");
            }
        } else {
            panic!("no chat selected");
        }

        app.input_text.clear();

        let chat = app.chat_manager.chats.get(&chat_id).unwrap();
        assert_eq!(
            chat.messages.len(),
            1,
            "History length should be 1 after sending"
        );
        match &chat.messages[0].content {
            crate::types::MessageContent::Text { text } => assert_eq!(text, "Message 1"),
            _ => panic!("Wrong message content"),
        }

        // Send second message
        app.input_text = "Message 2".to_string();
        app.send_message();

        let chat = app.chat_manager.chats.get(&chat_id).unwrap();
        assert_eq!(chat.messages.len(), 2);
        match &chat.messages[1].content {
            crate::types::MessageContent::Text { text } => assert_eq!(text, "Message 2"),
            _ => panic!("Wrong message content"),
        }
    }

    #[test]
    fn test_send_message_increments_seq() {
        let mut app = TuiApp::new().unwrap();
        let (chat_id, _rx) = setup_chat_with_session(&mut app, "Test");

        let initial_seq = app.chat_manager.chats.get(&chat_id).unwrap().send_seq;

        app.input_text = "Seq Test".to_string();
        app.send_message();

        let new_seq = app.chat_manager.chats.get(&chat_id).unwrap().send_seq;
        assert_eq!(new_seq, initial_seq + 1);
    }

    #[test]
    fn test_send_to_nonexistent_chat() {
        let mut app = TuiApp::new().unwrap();
        // Force state: empty chat_ids but selected index 0
        app.chat_ids = vec![];
        app.chat_list_state.select(Some(0));
        app.input_text = "Crash?".to_string();

        // Should not panic
        app.send_message();

        // Input should NOT be cleared because send failed
        assert_eq!(app.input_text, "Crash?");
    }

    #[test]
    fn test_scroll_overflow() {
        let mut app = TuiApp::new().unwrap();
        app.message_scroll = u16::MAX;

        app.scroll_down();
        assert_eq!(app.message_scroll, u16::MAX);

        app.message_scroll = 0;
        app.scroll_up();
        assert_eq!(app.message_scroll, 0);
    }

    #[test]
    fn test_unicode_input() {
        let mut app = TuiApp::new().unwrap();
        let (chat_id, _rx) = setup_chat_with_session(&mut app, "Unicode");

        let emoji_msg = "Hello 👋 🌍";
        app.input_text = emoji_msg.to_string();

        app.send_message();

        let chat = app.chat_manager.chats.get(&chat_id).unwrap();
        match &chat.messages[0].content {
            crate::types::MessageContent::Text { text } => assert_eq!(text, emoji_msg),
            _ => panic!("Wrong content"),
        }
    }

    #[test]
    fn test_very_long_input() {
        let mut app = TuiApp::new().unwrap();
        let (chat_id, _rx) = setup_chat_with_session(&mut app, "Long");

        let long_msg = "a".repeat(1000);
        app.input_text = long_msg.clone();

        app.send_message();

        let chat = app.chat_manager.chats.get(&chat_id).unwrap();
        match &chat.messages[0].content {
            crate::types::MessageContent::Text { text } => assert_eq!(text, &long_msg),
            _ => panic!("Wrong content"),
        }
    }
}
