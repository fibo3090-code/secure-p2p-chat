//! Encrypted-history persistence: encrypted round-trip, wrong-key rejection,
//! plaintext/encrypted auto-detection, and graceful handling of corrupt files.

use p2pem_classic::app::chat_manager::ChatManager;
use p2pem_classic::app::persistence::HistoryFile;
use p2pem_classic::types::{Chat, ChatKind, Config, Message, MessageContent, Transport};
use std::io::Write;
use uuid::Uuid;

fn sample_chat(title: &str) -> Chat {
    Chat {
        id: Uuid::new_v4(),
        title: title.to_string(),
        kind: ChatKind::Dm,
        transport: Transport::Direct,
        peer_fingerprint: Some("FE".repeat(32)),
        participants: Vec::new(),
        messages: vec![Message {
            id: Uuid::new_v4(),
            from_me: true,
            content: MessageContent::Text {
                text: "remember this".to_string(),
            },
            timestamp: chrono::Utc::now(),
        }],
        created_at: chrono::Utc::now(),
        send_seq: 0,
        recv_seq: 0,
        peer_typing: false,
        typing_since: None,
        is_host_placeholder: false,
    }
}

#[test]
fn encrypted_history_roundtrip_preserves_chats() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.enc");
    let key = [7u8; 32];

    let history = HistoryFile::new(vec![sample_chat("Persisted")]);
    history.save_encrypted(&path, &key).unwrap();

    let loaded = HistoryFile::load_encrypted(&path, &key).unwrap();
    assert_eq!(loaded.chats.len(), 1);
    assert_eq!(loaded.chats[0].title, "Persisted");
    match &loaded.chats[0].messages[0].content {
        MessageContent::Text { text } => assert_eq!(text, "remember this"),
        other => panic!("expected text content, got {other:?}"),
    }
}

#[test]
fn wrong_key_cannot_decrypt_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.enc");
    let key = [1u8; 32];
    let wrong = [2u8; 32];

    HistoryFile::new(vec![sample_chat("Secret")])
        .save_encrypted(&path, &key)
        .unwrap();

    assert!(
        HistoryFile::load_encrypted(&path, &wrong).is_err(),
        "decryption under the wrong key must fail"
    );
}

#[test]
fn corrupt_history_file_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.enc");
    let key = [9u8; 32];

    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(b"this is not a valid encrypted history blob")
        .unwrap();
    drop(f);

    assert!(HistoryFile::load_encrypted(&path, &key).is_err());
}

#[test]
fn load_history_auto_detects_encrypted_payload() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.enc");
    let key = [3u8; 32];

    let mut mgr = ChatManager::new(Config::default());
    mgr.set_history_key(key);
    let chat = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat, "AutoDetect".to_string());
    mgr.save_history(&path).unwrap();

    let mut reloaded = ChatManager::new(Config::default());
    reloaded.set_history_key(key);
    // Returns Ok(true/false) depending on which format was detected; either way it
    // must succeed and restore the chat.
    reloaded.load_history_auto(&path, &key).unwrap();
    assert!(reloaded.get_chat(chat).is_some());
    assert_eq!(reloaded.get_chat(chat).unwrap().title, "AutoDetect");
}
