//! The history file is the user's whole message archive. It can be truncated
//! by a crash, damaged on disk, or opened with the wrong key; every one of
//! those must fail cleanly and leave the running state exactly as it was, and
//! a save must never write what it cannot encrypt.

use messenger_core::types::{Chat, ChatKind, Config, Message, MessageContent, Transport};
use p2pem_classic::app::persistence::HistoryFile;
use p2pem_classic::app::ChatManager;
use uuid::Uuid;

const KEY: [u8; 32] = [42; 32];
const TITLE: &str = "Keep me 🧷 — Ünïcödé";

/// A manager holding one conversation with one message, saved to `dir`.
fn saved_history(dir: &std::path::Path) -> (std::path::PathBuf, Uuid) {
    let mut mgr = ChatManager::new(Config::default());
    mgr.set_history_key(KEY);
    let chat = Chat {
        peer_fingerprint: Some("FE".repeat(32)),
        messages: vec![Message {
            id: Uuid::new_v4(),
            from_me: true,
            content: MessageContent::Text {
                text: "remember this".into(),
            },
            timestamp: chrono::Utc::now(),
            delivered: true,
        }],
        ..Chat::new(Uuid::new_v4(), TITLE, ChatKind::Dm, Transport::Direct)
    };
    let id = chat.id;
    mgr.chats.insert(id, chat);
    let path = dir.join("history.json.enc");
    mgr.save_history(&path).expect("save");
    (path, id)
}

/// Saved through the manager, loaded both through the manager and through the
/// file type directly: title, message and fingerprint all come back.
#[test]
fn a_saved_history_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let (path, id) = saved_history(dir.path());

    let mut fresh = ChatManager::new(Config::default());
    fresh.set_history_key(KEY);
    assert!(
        fresh.load_history_auto(&path, &KEY).unwrap(),
        "detected as encrypted"
    );
    let chat = fresh.get_chat(id).expect("chat restored");
    assert_eq!(chat.title, TITLE);
    assert_eq!(
        chat.peer_fingerprint.as_deref(),
        Some("FE".repeat(32).as_str())
    );
    assert!(matches!(
        &chat.messages[..],
        [Message { content: MessageContent::Text { text }, delivered: true, .. }] if text == "remember this"
    ));

    let file = HistoryFile::load_encrypted(&path, &KEY).expect("load");
    assert_eq!(file.chats.len(), 1);
    assert_eq!(file.chats[0].title, TITLE);
}

/// A damaged file — or the right file under the wrong key — fails to load and
/// leaves the running state untouched.
#[test]
fn an_unreadable_history_fails_cleanly_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (path, _) = saved_history(dir.path());
    let good = std::fs::read(&path).unwrap();

    let mut flipped = good.clone();
    let mid = flipped.len() / 2;
    flipped[mid] ^= 0x01;
    let cases: Vec<(&str, Vec<u8>, [u8; 32])> = vec![
        ("empty", Vec::new(), KEY),
        ("truncated", good[..good.len() / 2].to_vec(), KEY),
        ("one byte", good[..1].to_vec(), KEY),
        ("garbage", vec![0xFF; 4096], KEY),
        ("bit flip", flipped, KEY),
        ("plain json", br#"{"version":1,"chats":[]}"#.to_vec(), KEY),
        ("wrong key", good.clone(), [7u8; 32]),
    ];
    for (what, bytes, key) in cases {
        std::fs::write(&path, &bytes).unwrap();
        let mut mgr = ChatManager::new(Config::default());
        mgr.set_history_key(key);
        let existing = Uuid::new_v4();
        mgr.create_local_chat_for_test(existing, "in memory".into());
        assert!(
            mgr.load_history_auto(&path, &key).is_err(),
            "{what}: an unreadable history loaded"
        );
        assert_eq!(
            mgr.chat_ids(),
            vec![existing],
            "{what}: a failed load changed the running state"
        );
        assert!(
            HistoryFile::load_encrypted(&path, &key).is_err(),
            "{what}: the file type accepted it"
        );
    }
}

/// A save without a key refuses rather than writing plaintext, and leaves the
/// file that is there byte-for-byte intact.
#[test]
fn a_save_without_a_key_refuses_and_leaves_the_file_intact() {
    let dir = tempfile::tempdir().unwrap();
    let (path, _) = saved_history(dir.path());
    let before = std::fs::read(&path).unwrap();
    let mut mgr = ChatManager::new(Config::default());
    mgr.create_local_chat_for_test(Uuid::new_v4(), "unsaved".into());
    assert!(mgr.save_history(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
