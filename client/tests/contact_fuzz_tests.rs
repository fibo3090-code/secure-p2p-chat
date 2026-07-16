use p2pem_classic::app::chat_manager::ChatManager;
use p2pem_classic::types::Config;
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn prop_add_remove_contacts(names in proptest::collection::vec(".{0,32}", 0..50)) {
        let mut manager = ChatManager::new(Config::default());
        let mut ids = Vec::new();

        for name in names {
            let id = manager.add_contact(name, None, None, None);
            ids.push(id);
        }

        prop_assert_eq!(manager.contacts.len(), ids.len());
        for id in &ids {
            prop_assert!(manager.contacts.contains_key(id));
        }

        for id in ids {
            manager.remove_contact(id);
        }

        prop_assert!(manager.contacts.is_empty());
    }
}
