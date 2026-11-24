//! Session key rotation (rekey): HKDF rotation is deterministic, both peers
//! reach the same key, and messages keep flowing across a rotation.
use p2pem_classic::core::{generate_rekey_nonce, rekey_session_key, AesCipher, ProtocolMessage};
use p2pem_classic::AES_KEY_SIZE;
use rand::Rng;
use std::time::Instant;

fn random_key() -> [u8; AES_KEY_SIZE] {
    rand::thread_rng().r#gen()
}

fn random_nonce() -> [u8; 16] {
    rand::thread_rng().r#gen()
}

/// Encrypt with `enc`, decrypt with `dec`, and check the plaintext survives.
fn assert_round_trip(enc: &AesCipher, dec: &AesCipher, plaintext: &[u8]) {
    let encrypted = enc.encrypt(plaintext, None);
    assert_eq!(dec.decrypt(&encrypted, None).unwrap(), plaintext);
}

#[test]
fn test_rekey_basic_key_derivation() {
    let original_key = random_key();
    let nonce = random_nonce();
    let rotated_key_1 = rekey_session_key(&original_key, &nonce);
    let rotated_key_2 = rekey_session_key(&original_key, &nonce);
    assert_eq!(rotated_key_1, rotated_key_2, "deterministic");
    assert_ne!(rotated_key_1, original_key);
}

#[test]
fn test_rekey_different_nonces_different_keys() {
    let original_key = random_key();
    let nonce_1 = random_nonce();
    let mut nonce_2 = random_nonce();
    if nonce_1 == nonce_2 {
        nonce_2[0] = nonce_2[0].wrapping_add(1);
    }
    assert_ne!(
        rekey_session_key(&original_key, &nonce_1),
        rekey_session_key(&original_key, &nonce_2)
    );
}

#[test]
fn test_rekey_message_encoding_decoding() {
    let nonce = generate_rekey_nonce();
    let seq = 42u64;
    let bytes = ProtocolMessage::Rekey {
        nonce: nonce.to_vec(),
        seq,
    }
    .to_plain_bytes();
    match ProtocolMessage::from_plain_bytes(&bytes) {
        Some(ProtocolMessage::Rekey {
            nonce: parsed_nonce,
            seq: parsed_seq,
        }) => {
            assert_eq!(parsed_nonce.as_slice(), &nonce);
            assert_eq!(parsed_seq, seq);
        }
        other => panic!("Expected Rekey message, got {other:?}"),
    }
}

#[test]
fn test_rekey_nonce_generation() {
    let nonce_1 = generate_rekey_nonce();
    let nonce_2 = generate_rekey_nonce();
    assert_eq!(nonce_1.len(), 16);
    assert_eq!(nonce_2.len(), 16);
    assert_ne!(nonce_1, nonce_2, "random (with overwhelming probability)");
}

#[test]
fn test_rekey_cipher_reset() {
    let key_1 = random_key();
    let cipher_1 = AesCipher::new(&key_1).unwrap();
    let plaintext = b"Test message before rekey";
    let encrypted_1 = cipher_1.encrypt(plaintext, None);
    assert_eq!(cipher_1.decrypt(&encrypted_1, None).unwrap(), plaintext);

    // A cipher on the rotated key works independently...
    let key_2 = rekey_session_key(&key_1, &generate_rekey_nonce());
    let cipher_2 = AesCipher::new(&key_2).unwrap();
    assert_round_trip(&cipher_2, &cipher_2, b"Test message after rekey");

    // ...and the old one can still decrypt its own messages.
    assert_eq!(cipher_1.decrypt(&encrypted_1, None).unwrap(), plaintext);
}

#[test]
fn test_rekey_sequence_produces_correct_chain() {
    let initial_key = random_key();
    let nonces: Vec<[u8; 16]> = (0..5).map(|_| random_nonce()).collect();
    let chain = |check: bool| {
        let mut key = initial_key;
        let mut chain = vec![key];
        for nonce in &nonces {
            let prev = key;
            key = rekey_session_key(&key, nonce);
            if check {
                assert_ne!(key, prev, "each key differs from the previous one");
                AesCipher::new(&key).unwrap();
            }
            chain.push(key);
        }
        chain
    };
    let key_chain = chain(true);
    assert_eq!(key_chain, chain(false), "Key chain is not deterministic");
    assert_eq!(key_chain.len(), 6); // initial + 5 rotations
}

#[test]
fn test_rekey_bidirectional_peers() {
    // Both peers start from the same ECDH key and rotate with the same nonce.
    let shared = random_key();
    let nonce = generate_rekey_nonce();
    let alice_key = rekey_session_key(&shared, &nonce);
    let bob_key = rekey_session_key(&shared, &nonce);
    assert_eq!(alice_key, bob_key);

    let alice = AesCipher::new(&alice_key).unwrap();
    let bob = AesCipher::new(&bob_key).unwrap();
    assert_round_trip(&alice, &bob, b"Hello from Alice after rekey");
    assert_round_trip(&bob, &alice, b"Hello from Bob after rekey");
}

/// Guards against rekeying becoming expensive (a password-hashing KDF would
/// stall the message loop every REKEY_MESSAGE_COUNT messages). The bound is
/// enormous on purpose: 1000 HKDFs take milliseconds, and a tight bound
/// flaked on loaded CI runners.
#[test]
fn test_rekey_timing_performance() {
    let key = random_key();
    let nonce = random_nonce();
    let start = Instant::now();
    for _ in 0..1000 {
        let _ = rekey_session_key(&key, &nonce);
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_secs() < 5,
        "1000 rekeys took {}ms — that is not a slow machine, that is a different KDF",
        elapsed.as_millis()
    );
}

#[test]
fn test_rekey_key_independence() {
    let key_a = random_key();
    let mut key_b = random_key();
    if key_a == key_b {
        key_b[0] = key_b[0].wrapping_add(1);
    }
    let nonce = random_nonce();
    assert_ne!(
        rekey_session_key(&key_a, &nonce),
        rekey_session_key(&key_b, &nonce)
    );
}

#[test]
fn test_rekey_message_preservation() {
    let key = random_key();
    let messages = [
        b"First message".to_vec(),
        b"Second message with more content".to_vec(),
        b"Third message with special chars: @#$%^&*()".to_vec(),
    ];
    let cipher = AesCipher::new(&key).unwrap();
    let encrypted: Vec<_> = messages.iter().map(|m| cipher.encrypt(m, None)).collect();

    let new_cipher = AesCipher::new(&rekey_session_key(&key, &generate_rekey_nonce())).unwrap();
    let new_message = b"Message after rekey";
    let encrypted_new = new_cipher.encrypt(new_message, None);

    // Old messages still decrypt with the old cipher, new with the new one.
    for (msg, enc) in messages.iter().zip(&encrypted) {
        assert_eq!(&cipher.decrypt(enc, None).unwrap(), msg);
    }
    assert_eq!(
        new_cipher.decrypt(&encrypted_new, None).unwrap(),
        new_message
    );
}
