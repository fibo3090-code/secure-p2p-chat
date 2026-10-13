//! A history file is the user's whole message archive; it can be truncated by a
//! crash, damaged on disk, or opened with the wrong key. Loading one must fail
//! cleanly and leave the running state exactly as it was.

use messenger_core::types::Config;
use p2pem_classic::app::ChatManager;
use uuid::Uuid;

const KEY: [u8; 32] = [42; 32];

fn manager_with_history(dir: &std::path::Path) -> (ChatManager, std::path::PathBuf, Uuid) {
    let mut mgr = ChatManager::new(Config::default());
    mgr.set_history_key(KEY);
    let chat = Uuid::new_v4();
    mgr.create_local_chat_for_test(chat, "Keep me 🧷 — Ünïcödé".into());
    let path = dir.join("history.json.enc");
    mgr.save_history(&path).expect("save");
    (mgr, path, chat)
}

#[test]
fn a_saved_history_round_trips_byte_for_byte_titles() {
    let dir = tempfile::tempdir().unwrap();
    let (_, path, chat) = manager_with_history(dir.path());
    let mut fresh = ChatManager::new(Config::default());
    fresh.set_history_key(KEY);
    assert!(fresh.load_history_auto(&path, &KEY).unwrap());
    assert_eq!(fresh.get_chat(chat).unwrap().title, "Keep me 🧷 — Ünïcödé");
}

#[test]
fn damaged_history_fails_cleanly_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (_, path, _) = manager_with_history(dir.path());
    let good = std::fs::read(&path).unwrap();

    let damaged: Vec<(&str, Vec<u8>)> = vec![
        ("empty", Vec::new()),
        ("truncated", good[..good.len() / 2].to_vec()),
        ("one byte", good[..1].to_vec()),
        ("garbage", vec![0xFF; 4096]),
        ("bit flip", {
            let mut b = good.clone();
            let mid = b.len() / 2;
            b[mid] ^= 0x01;
            b
        }),
        ("plain json", br#"{"version":1,"chats":[]}"#.to_vec()),
    ];
    for (what, bytes) in damaged {
        std::fs::write(&path, &bytes).unwrap();
        let mut mgr = ChatManager::new(Config::default());
        mgr.set_history_key(KEY);
        let existing = Uuid::new_v4();
        mgr.create_local_chat_for_test(existing, "in memory".into());
        let result = mgr.load_history_auto(&path, &KEY);
        assert!(result.is_err(), "{what}: a damaged history loaded");
        assert_eq!(
            mgr.chat_ids(),
            vec![existing],
            "{what}: a failed load changed the running state"
        );
    }
}

#[test]
fn the_wrong_key_fails_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let (_, path, _) = manager_with_history(dir.path());
    let mut mgr = ChatManager::new(Config::default());
    let wrong = [7u8; 32];
    mgr.set_history_key(wrong);
    assert!(mgr.load_history_auto(&path, &wrong).is_err());
    assert!(mgr.chat_ids().is_empty());
}

#[test]
fn a_save_without_a_key_refuses_and_leaves_the_file_intact() {
    let dir = tempfile::tempdir().unwrap();
    let (mut mgr, path, chat) = manager_with_history(dir.path());
    let before = std::fs::read(&path).unwrap();
    // A save without a key refuses rather than writing plaintext.
    mgr.history_key = None;
    assert!(mgr.save_history(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let _ = chat;
}
