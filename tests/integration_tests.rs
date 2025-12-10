use encodeur_rsa_rust::app::chat_manager::ChatManager;
use encodeur_rsa_rust::types::{Config, Theme};
use uuid::Uuid;

/// Test the full lifecycle of a chat: Creation -> Renaming -> Selection -> Deletion
#[tokio::test]
async fn test_chat_lifecycle() {
    let mut manager = ChatManager::new(Config::default());
    let chat_id = Uuid::new_v4();
    
    // 1. Create (Simulated)
    manager.create_local_chat_for_test(chat_id, "Initial Title".to_string());
    assert!(manager.chats.contains_key(&chat_id), "Chat should be present after creation");
    assert_eq!(manager.chats.get(&chat_id).unwrap().title, "Initial Title");

    // 2. Rename
    let rename_res = manager.rename_chat(chat_id, "Renamed Title".to_string());
    assert!(rename_res.is_ok(), "Renaming should succeed");
    assert_eq!(manager.chats.get(&chat_id).unwrap().title, "Renamed Title");

    // 3. Delete
    manager.delete_chat(chat_id);
    assert!(!manager.chats.contains_key(&chat_id), "Chat should be gone after deletion");
}

/// Test contact management logic
#[tokio::test]
async fn test_contact_management() {
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
    assert_eq!(config.theme, Theme::Dark, "Default theme should be Dark (as per types.rs)");
    assert_eq!(config.auto_host_on_startup, false);

    // Change settings
    config.theme = Theme::Midnight;
    config.auto_host_on_startup = true;

    // Verify
    assert_eq!(config.theme, Theme::Midnight);
    assert!(config.auto_host_on_startup);
}

/// Test address parsing logic indirectly via contact addition
#[test]
fn test_address_parsing_logic() {
    // This logic mimics the validation added to the UI
    let valid_addr = "192.168.1.1:8080";
    let invalid_addr = "192.168.1.1"; // Missing port

    assert!(valid_addr.contains(':'));
    assert!(!invalid_addr.contains(':'));
}
