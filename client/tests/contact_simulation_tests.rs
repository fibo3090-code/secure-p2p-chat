use p2pem_classic::app::chat_manager::ChatManager;
use p2pem_classic::types::{Config, TrustState};

#[tokio::test]
async fn simulation_contacts_lifecycle() {
    let mut manager = ChatManager::new(Config::default());

    // Scenario: User imports 500 contacts, modifies 50, blocks 10, deletes 5

    // 1. Bulk Import
    let mut ids = Vec::new();
    for i in 0..500 {
        let id = manager.add_contact(
            format!("Imported User {}", i),
            Some(format!("192.168.1.{}", i % 255)),
            Some(format!("FINGERPRINT_{}", i)),
            None,
        );
        ids.push(id);
    }

    assert_eq!(manager.contacts.len(), 500);

    // 2. Modify Metadata (Simulate marking as Trusted)
    for id in ids.iter().take(50) {
        if let Some(contact) = manager.contacts.get_mut(id) {
            contact.trust_state = TrustState::Trusted;
            contact.notes = "Verified in person".to_string();
            contact.tags.push("Work".to_string());
        }
    }

    // 3. Block malicious users
    for id in ids.iter().take(60).skip(50) {
        if let Some(contact) = manager.contacts.get_mut(id) {
            contact.trust_state = TrustState::Blocked;
            contact.notes = "Spammer".to_string();
        }
    }

    // 4. Verification
    let trusted_count = manager
        .contacts
        .values()
        .filter(|c| c.trust_state == TrustState::Trusted)
        .count();
    assert_eq!(trusted_count, 50);

    let blocked_count = manager
        .contacts
        .values()
        .filter(|c| c.trust_state == TrustState::Blocked)
        .count();
    assert_eq!(blocked_count, 10);

    // 5. Deletion
    for id in ids.iter().take(405).skip(400) {
        manager.remove_contact(*id);
    }

    assert_eq!(manager.contacts.len(), 495);

    println!("Simulation lifecycle passed.");
}
