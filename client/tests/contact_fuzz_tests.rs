//! Property tests over the contact store.
//!
//! The original test drove `add` and `remove` with random names and asserted the
//! map grew and shrank. That is a test of `HashMap`, not of this app: names are
//! the one field with no rules attached to it.
//!
//! What actually carries weight here is the **trust lifecycle**, because the
//! contact store is where trust decisions are recorded and every one of them has
//! an invariant that a bug would break silently:
//!
//! - an imported invite is *never* trusted on arrival, whatever the link claims
//! - identity is the fingerprint, so importing twice must refresh, not duplicate
//! - blocking must survive anything short of deletion
//! - unblocking must not invent verification that never happened
//! - deleting a contact must revoke its trust, because the confirmation dialog
//!   promises exactly that
//!
//! These are checked against randomised sequences of operations rather than one
//! hand-picked path, since the ordering is where the interesting failures live.

use p2pem_classic::app::chat_manager::ChatManager;
use p2pem_classic::types::{Config, Contact, TrustState};
use proptest::prelude::*;
use uuid::Uuid;

/// A hex fingerprint derived from a seed, so a property can generate distinct
/// identities without generating invalid ones.
fn fingerprint_for(seed: u8) -> String {
    format!("{seed:02x}").repeat(32)
}

fn contact_with(name: &str, fingerprint: Option<String>) -> Contact {
    Contact {
        id: Uuid::new_v4(),
        name: name.to_string(),
        address: Some("192.168.1.10:12345".to_string()),
        addresses: Vec::new(),
        relay_server: None,
        relay_token: None,
        fingerprint,
        public_key: None,
        created_at: chrono::Utc::now(),
        // The point of the test: an invite may *claim* anything. The store must
        // not take its word for it.
        trust_state: TrustState::Trusted,
        notes: String::new(),
        tags: Vec::new(),
        last_seen: None,
    }
}

/// One step in a randomised lifecycle.
#[derive(Debug, Clone)]
enum Op {
    Block,
    Unblock,
    Remove,
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![Just(Op::Block), Just(Op::Unblock), Just(Op::Remove)]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// Adding and removing stays consistent for any names, including empty ones
    /// and ones that differ only by case or whitespace.
    #[test]
    fn add_and_remove_is_consistent(names in proptest::collection::vec(".{0,32}", 0..50)) {
        let mut manager = ChatManager::new(Config::default());
        let mut ids = Vec::new();

        for name in names {
            ids.push(manager.add_contact(name, None, None, None));
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

    /// A contact added by hand starts Unverified, whatever it is called.
    #[test]
    fn a_new_contact_is_never_trusted_on_arrival(name in ".{0,32}") {
        let mut manager = ChatManager::new(Config::default());
        let id = manager.add_contact(name, None, Some(fingerprint_for(1)), None);
        prop_assert_eq!(
            manager.get_contact(id).map(|c| c.trust_state),
            Some(TrustState::Unverified)
        );
    }

    /// An invite link is not verification. Whatever `trust_state` the link
    /// carries, the imported contact starts Unverified — otherwise pasting a
    /// link would pre-trust whatever fingerprint it named, and that peer would
    /// connect with no safety-code prompt at all.
    #[test]
    fn importing_an_invite_never_confers_trust(seed in 0u8..64) {
        let mut manager = ChatManager::new(Config::default());
        let id = manager
            .import_contact(contact_with("Imported", Some(fingerprint_for(seed))))
            .expect("a well-formed invite imports");
        prop_assert_eq!(
            manager.get_contact(id).map(|c| c.trust_state),
            Some(TrustState::Unverified)
        );
    }

    /// Identity is the fingerprint. Re-importing the same one refreshes the
    /// existing contact instead of adding a second card for the same person.
    #[test]
    fn importing_the_same_identity_twice_does_not_duplicate(seed in 0u8..64, times in 2usize..6) {
        let mut manager = ChatManager::new(Config::default());
        let fp = fingerprint_for(seed);
        let mut ids = Vec::new();
        for i in 0..times {
            ids.push(
                manager
                    .import_contact(contact_with(&format!("Name {i}"), Some(fp.clone())))
                    .expect("import succeeds"),
            );
        }
        prop_assert_eq!(manager.contacts.len(), 1);
        // And every import resolved to the same contact.
        prop_assert!(ids.windows(2).all(|w| w[0] == w[1]));
    }

    /// Your own invite is refused however many times it is pasted.
    #[test]
    fn your_own_invite_is_always_refused(seed in 0u8..64) {
        let mut manager = ChatManager::new(Config::default());
        let mine = fingerprint_for(seed);
        manager.set_my_fingerprint(mine.clone());

        // Case must not be a way around it.
        for variant in [mine.clone(), mine.to_uppercase()] {
            prop_assert!(
                manager.import_contact(contact_with("Me", Some(variant))).is_err(),
                "importing your own identity must be refused"
            );
        }
        prop_assert!(manager.contacts.is_empty());
    }

    /// Blocking sticks. Any sequence of block/unblock ends in exactly the state
    /// the last operation asked for, and a blocked contact is never left in some
    /// intermediate trust state.
    #[test]
    fn block_and_unblock_converge(seed in 0u8..64, ops in proptest::collection::vec(op_strategy(), 1..12)) {
        let mut manager = ChatManager::new(Config::default());
        let id = manager
            .import_contact(contact_with("Peer", Some(fingerprint_for(seed))))
            .expect("import succeeds");

        let mut removed = false;
        let mut blocked = false;
        for op in ops {
            match op {
                Op::Block => {
                    let ok = manager.block_contact(id).is_ok();
                    prop_assert_eq!(ok, !removed, "block should only work on a live contact");
                    if ok { blocked = true; }
                }
                Op::Unblock => {
                    let ok = manager.unblock_contact(id).is_ok();
                    prop_assert_eq!(ok, !removed, "unblock should only work on a live contact");
                    if ok { blocked = false; }
                }
                Op::Remove => {
                    manager.remove_contact(id);
                    removed = true;
                }
            }

            match manager.get_contact(id).map(|c| c.trust_state) {
                None => prop_assert!(removed, "the contact vanished without being removed"),
                Some(TrustState::Blocked) => prop_assert!(blocked, "blocked without a block"),
                Some(other) => {
                    prop_assert!(!blocked, "a blocked contact must read as Blocked, got {:?}", other);
                    // Unblocking must not invent verification. Nothing in this
                    // test ever confirms a fingerprint in a chat, so the only
                    // honest resting state is Unverified.
                    prop_assert_eq!(
                        other,
                        TrustState::Unverified,
                        "unblocking must not promote a contact that was never verified"
                    );
                }
            }
        }
    }

    /// Deleting a contact revokes its trust: the fingerprint is cleared from
    /// every chat that held it, so the peer has to be verified again. The
    /// confirmation dialog promises this, which is what makes it worth a test.
    #[test]
    fn deleting_a_contact_revokes_the_fingerprint_it_vouched_for(seed in 0u8..64) {
        let mut manager = ChatManager::new(Config::default());
        let fp = fingerprint_for(seed);
        let id = manager
            .import_contact(contact_with("Peer", Some(fp.clone())))
            .expect("import succeeds");

        // A conversation that trusts this fingerprint.
        let chat_id = Uuid::new_v4();
        manager.create_local_chat_for_test(chat_id, "Peer".to_string());
        if let Some(chat) = manager.chats.get_mut(&chat_id) {
            chat.peer_fingerprint = Some(fp.clone());
        }
        manager.associate_contact_with_chat(id, chat_id);

        manager.remove_contact(id);

        prop_assert!(manager.get_contact(id).is_none());
        prop_assert_eq!(
            manager.chats.get(&chat_id).and_then(|c| c.peer_fingerprint.clone()),
            None,
            "deleting the contact must clear the fingerprint it vouched for"
        );
    }
}
