use p2pem_classic::app::chat_manager::ChatManager;
use p2pem_classic::types::{Config, Theme};
use rand::RngCore;
use tempfile::NamedTempFile;
use uuid::Uuid;

/// Test the full lifecycle of a chat: Creation -> Renaming -> Persistence -> Deletion
#[test]
fn test_chat_lifecycle() {
    let mut manager = ChatManager::new(Config::default());
    let chat_id = Uuid::new_v4();

    // Set encryption key for history (required for save/load)
    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    manager.set_history_key(key);

    // 1. Create (Simulated)
    manager.create_local_chat_for_test(chat_id, "Initial Title".to_string());
    assert!(
        manager.chats.contains_key(&chat_id),
        "Chat should be present after creation"
    );
    assert_eq!(manager.chats.get(&chat_id).unwrap().title, "Initial Title");

    // 2. Rename
    let rename_res = manager.rename_chat(chat_id, "Renamed Title".to_string());
    assert!(rename_res.is_ok(), "Renaming should succeed");
    assert_eq!(manager.chats.get(&chat_id).unwrap().title, "Renamed Title");

    // 3. Persist history and reload
    let history_file = NamedTempFile::new().unwrap();
    manager.save_history(history_file.path()).unwrap();
    let mut reloaded = ChatManager::new(Config::default());
    reloaded.set_history_key(key);
    reloaded
        .load_history_encrypted(history_file.path(), &key)
        .unwrap();
    assert_eq!(reloaded.chats.get(&chat_id).unwrap().title, "Renamed Title");

    // 4. Delete
    manager.delete_chat(chat_id);
    assert!(
        !manager.chats.contains_key(&chat_id),
        "Chat should be gone after deletion"
    );
}

/// Test contact management logic
#[test]
fn test_contact_management() {
    let mut manager = ChatManager::new(Config::default());

    // 1. Add Valid Contact
    let name = "Test User".to_string();
    let addr = Some("127.0.0.1:5000".to_string());
    let id = manager.add_contact(name.clone(), addr.clone(), None, None);

    assert!(manager.contacts.contains_key(&id));
    let contact = manager.contacts.get(&id).unwrap();
    assert_eq!(contact.name, name);
    assert_eq!(contact.address, addr);
}

/// Test configuration settings and theme persistence logic
#[test]
fn test_config_and_theme() {
    let mut config = Config::default();

    // Default check
    assert_eq!(
        config.theme,
        Theme::Dark,
        "Default theme should be Dark (as per types.rs)"
    );
    assert!(!config.auto_host_on_startup);

    // Change settings
    config.theme = Theme::Midnight;
    config.auto_host_on_startup = true;

    // Verify
    assert_eq!(config.theme, Theme::Midnight);
    assert!(config.auto_host_on_startup);
}

/// Test address parsing logic with real parser
#[test]
fn test_address_parsing_logic() {
    let valid_addr = "192.168.1.1:8080";
    let invalid_addr = "192.168.1.1"; // Missing port

    let parsed = ChatManager::parse_address(valid_addr).unwrap();
    assert_eq!(parsed.0, "192.168.1.1");
    assert_eq!(parsed.1, 8080);
    assert!(ChatManager::parse_address(invalid_addr).is_err());
}

#[test]
fn test_address_parsing_supports_bracketed_ipv6() {
    let parsed = ChatManager::parse_address("[2001:db8::1]:7000").unwrap();
    assert_eq!(parsed.0, "2001:db8::1");
    assert_eq!(parsed.1, 7000);
}
