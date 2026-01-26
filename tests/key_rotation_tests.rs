/// Key Rotation (Rekeying) Integration Tests
///
/// Tests for periodic session key rotation to provide perfect forward secrecy.
/// Ensures that:
/// - Keys can be rotated correctly with HKDF
/// - Rotation happens at correct message/time intervals
/// - Both peers rotate to the same key
/// - Messages continue to flow correctly after rotation
use encodeur_rsa_rust::core::{
    generate_rekey_nonce, rekey_session_key, AesCipher, ProtocolMessage,
};
use encodeur_rsa_rust::AES_KEY_SIZE;
use rand::RngCore;
use std::time::Instant;

#[test]
fn test_rekey_basic_key_derivation() {
    // Test that rekeying produces deterministic keys
    // Use cryptographically secure random values instead of hardcoded constants
    let mut original_key = [0u8; AES_KEY_SIZE];
    let mut nonce = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut original_key);
    rand::rngs::OsRng.fill_bytes(&mut nonce);

    let rotated_key_1 = rekey_session_key(&original_key, &nonce);
    let rotated_key_2 = rekey_session_key(&original_key, &nonce);

    // Same input should produce same output
    assert_eq!(rotated_key_1, rotated_key_2);

    // Output should be different from input
    assert_ne!(rotated_key_1, original_key);
}

#[test]
fn test_rekey_different_nonces_different_keys() {
    // Different nonces should produce different keys
    // Use cryptographically secure random values
    let mut original_key = [0u8; AES_KEY_SIZE];
    let mut nonce_1 = [0u8; 16];
    let mut nonce_2 = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut original_key);
    rand::rngs::OsRng.fill_bytes(&mut nonce_1);
    rand::rngs::OsRng.fill_bytes(&mut nonce_2);

    // Ensure nonces are different
    if nonce_1 == nonce_2 {
        nonce_2[0] = nonce_2[0].wrapping_add(1);
    }

    let key_1 = rekey_session_key(&original_key, &nonce_1);
    let key_2 = rekey_session_key(&original_key, &nonce_2);

    assert_ne!(key_1, key_2);
}

#[test]
fn test_rekey_message_encoding_decoding() {
    // Test that REKEY messages can be serialized and deserialized
    let nonce = generate_rekey_nonce();
    let seq = 42u64;

    let rekey_msg = ProtocolMessage::Rekey {
        nonce: nonce.to_vec(),
        seq,
    };

    let bytes = rekey_msg.to_plain_bytes();
    let parsed = ProtocolMessage::from_plain_bytes(&bytes);

    assert!(parsed.is_some());
    match parsed {
        Some(ProtocolMessage::Rekey {
            nonce: parsed_nonce,
            seq: parsed_seq,
        }) => {
            assert_eq!(parsed_nonce.as_slice(), &nonce);
            assert_eq!(parsed_seq, seq);
        }
        _ => panic!("Expected Rekey message"),
    }
}

#[test]
fn test_rekey_nonce_generation() {
    // Test that generated nonces are properly sized and random
    let nonce_1 = generate_rekey_nonce();
    let nonce_2 = generate_rekey_nonce();

    assert_eq!(nonce_1.len(), 16);
    assert_eq!(nonce_2.len(), 16);

    // Nonces should be different (with overwhelming probability)
    assert_ne!(nonce_1, nonce_2);
}

#[test]
fn test_rekey_cipher_reset() {
    // Test that new ciphers created with rotated keys work correctly
    // Use cryptographically secure random values
    let mut key_1 = [0u8; AES_KEY_SIZE];
    rand::rngs::OsRng.fill_bytes(&mut key_1);
    let cipher_1 = AesCipher::new(&key_1).unwrap();

    let plaintext = b"Test message before rekey";
    let encrypted_1 = cipher_1.encrypt(plaintext, None);

    // Verify decryption works
    let decrypted_1 = cipher_1.decrypt(&encrypted_1, None).unwrap();
    assert_eq!(plaintext, &decrypted_1[..]);

    // Rekey with a nonce
    let nonce = generate_rekey_nonce();
    let key_2 = rekey_session_key(&key_1, &nonce);
    let cipher_2 = AesCipher::new(&key_2).unwrap();

    // New cipher should work independently
    let plaintext_2 = b"Test message after rekey";
    let encrypted_2 = cipher_2.encrypt(plaintext_2, None);
    let decrypted_2 = cipher_2.decrypt(&encrypted_2, None).unwrap();
    assert_eq!(plaintext_2, &decrypted_2[..]);

    // Old cipher can still decrypt its own messages
    let old_decrypt = cipher_1.decrypt(&encrypted_1, None).unwrap();
    assert_eq!(plaintext, &old_decrypt[..]);
}

#[test]
fn test_rekey_sequence_produces_correct_chain() {
    // Test that multiple rekeying operations produce a proper key chain
    // and verify deterministic reproduction of the chain
    // Use cryptographically secure random values
    let mut initial_key = [0u8; AES_KEY_SIZE];
    rand::rngs::OsRng.fill_bytes(&mut initial_key);
    let mut current_key = initial_key;
    let mut key_chain = vec![current_key];

    // Perform multiple rekeying operations and build the key chain
    // Use cryptographically secure random nonces
    let mut nonces = Vec::new();
    for _ in 0..5 {
        let mut nonce = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        nonces.push(nonce);
    }

    for nonce in &nonces {
        let prev_key = current_key;
        current_key = rekey_session_key(&current_key, nonce);

        // Verify each key is different from the previous one
        assert_ne!(current_key, prev_key);

        // Can create a cipher with this key
        let _ = AesCipher::new(&current_key).unwrap();

        key_chain.push(current_key);
    }

    // Now verify deterministic reproduction: rebuild the chain from the same initial key and nonces
    let mut reproduced_key = initial_key;
    let mut reproduced_chain = vec![reproduced_key];

    for nonce in &nonces {
        reproduced_key = rekey_session_key(&reproduced_key, nonce);
        reproduced_chain.push(reproduced_key);
    }

    // Chains must match exactly, proving deterministic key derivation
    assert_eq!(
        key_chain, reproduced_chain,
        "Key chain is not deterministic"
    );
    assert_eq!(key_chain.len(), 6); // initial + 5 rotations
}

#[test]
fn test_rekey_bidirectional_peers() {
    // Simulate two peers rekeying to the same key
    // Use cryptographically secure random values

    // Alice's side
    let mut alice_key_1 = [0u8; AES_KEY_SIZE];
    rand::rngs::OsRng.fill_bytes(&mut alice_key_1);
    let nonce = generate_rekey_nonce();
    let alice_key_2 = rekey_session_key(&alice_key_1, &nonce);

    // Bob's side
    let bob_key_1 = alice_key_1; // Same starting key from ECDH
    let bob_key_2 = rekey_session_key(&bob_key_1, &nonce);

    // Both should derive the same next key
    assert_eq!(alice_key_2, bob_key_2);

    // Create ciphers with the rotated keys
    let alice_cipher = AesCipher::new(&alice_key_2).unwrap();
    let bob_cipher = AesCipher::new(&bob_key_2).unwrap();

    // Test cross-encryption/decryption
    let alice_msg = b"Hello from Alice after rekey";
    let alice_encrypted = alice_cipher.encrypt(alice_msg, None);
    let alice_decrypted = bob_cipher.decrypt(&alice_encrypted, None).unwrap();
    assert_eq!(alice_msg, &alice_decrypted[..]);

    // And the reverse
    let bob_msg = b"Hello from Bob after rekey";
    let bob_encrypted = bob_cipher.encrypt(bob_msg, None);
    let bob_decrypted = alice_cipher.decrypt(&bob_encrypted, None).unwrap();
    assert_eq!(bob_msg, &bob_decrypted[..]);
}

#[test]
fn test_rekey_timing_performance() {
    // Ensure rekeying operations are fast (< 100ms for 1000 operations in debug mode)
    // Use cryptographically secure random values
    let mut key = [0u8; AES_KEY_SIZE];
    let mut nonce = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut key);
    rand::rngs::OsRng.fill_bytes(&mut nonce);

    let start = Instant::now();
    for _ in 0..1000 {
        let _ = rekey_session_key(&key, &nonce);
    }
    let elapsed = start.elapsed();

    // Should be reasonably fast (< 100ms for 1000 operations, even in debug mode)
    // In release mode, this should be < 10ms
    assert!(
        elapsed.as_millis() < 100,
        "1000 rekeys took {}ms",
        elapsed.as_millis()
    );
}

#[test]
fn test_rekey_key_independence() {
    // Test that rotating from two different starting keys produces different results
    // Use cryptographically secure random values
    let mut key_a = [0u8; AES_KEY_SIZE];
    let mut key_b = [0u8; AES_KEY_SIZE];
    let mut nonce = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut key_a);
    rand::rngs::OsRng.fill_bytes(&mut key_b);
    rand::rngs::OsRng.fill_bytes(&mut nonce);

    // Ensure keys are different
    if key_a == key_b {
        key_b[0] = key_b[0].wrapping_add(1);
    }

    let rotated_a = rekey_session_key(&key_a, &nonce);
    let rotated_b = rekey_session_key(&key_b, &nonce);

    // Different starting keys should produce different rotated keys
    assert_ne!(rotated_a, rotated_b);
}

#[test]
fn test_rekey_message_preservation() {
    // Test that messages are correctly preserved through encryption/decryption
    // even with rekeying
    // Use cryptographically secure random values
    let mut key = [0u8; AES_KEY_SIZE];
    rand::rngs::OsRng.fill_bytes(&mut key);

    // Create multiple messages
    let messages = vec![
        b"First message".to_vec(),
        b"Second message with more content".to_vec(),
        b"Third message with special chars: @#$%^&*()".to_vec(),
    ];

    // Encrypt with original key
    let cipher = AesCipher::new(&key).unwrap();
    let mut encrypted_msgs = Vec::new();
    for msg in &messages {
        encrypted_msgs.push(cipher.encrypt(msg, None));
    }

    // Rekey
    let nonce = generate_rekey_nonce();
    let new_key = rekey_session_key(&key, &nonce);
    let new_cipher = AesCipher::new(&new_key).unwrap();

    // Encrypt new messages with new key
    let new_message = b"Message after rekey";
    let encrypted_new = new_cipher.encrypt(new_message, None);

    // Verify all old messages are still decryptable (with old cipher)
    for (i, encrypted) in encrypted_msgs.iter().enumerate() {
        let decrypted = cipher.decrypt(encrypted, None).unwrap();
        assert_eq!(messages[i], decrypted);
    }

    // Verify new message is decryptable with new cipher
    let decrypted_new = new_cipher.decrypt(&encrypted_new, None).unwrap();
    assert_eq!(new_message, &decrypted_new[..]);
}
