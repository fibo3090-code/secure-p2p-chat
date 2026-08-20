//! Coverage-guided fuzzing of the peer-to-peer frame decoder.
//!
//! `from_plain_bytes` sees every frame a peer sends, once decrypted. It is the
//! single largest attacker-controlled parser in the project: reaching it needs
//! only a completed handshake, and everything after it trusts what it returns.
//!
//! The property suite in `core/tests/fuzz_parsers.rs` covers the same function
//! and runs on every pull request, which is what makes it useful. This target is
//! the other half: libFuzzer mutates toward new coverage, so it walks into
//! branch combinations a random generator reaches only by luck. Neither replaces
//! the other — the cheap one gates merges, this one is run deliberately and for
//! longer.
#![no_main]

use libfuzzer_sys::fuzz_target;
use messenger_core::core::ProtocolMessage;

fuzz_target!(|data: &[u8]| {
    // Contract 1: never panic, on any input.
    if let Some(decoded) = ProtocolMessage::from_plain_bytes(data) {
        // Contract 2: anything accepted must survive a round trip. A decoder
        // that accepts what its encoder cannot reproduce has a state in it that
        // no other part of the system knows how to handle.
        let re_encoded = decoded.to_plain_bytes();
        let again = ProtocolMessage::from_plain_bytes(&re_encoded)
            .expect("a frame this decoder produced must parse back");
        assert_eq!(
            std::mem::discriminant(&decoded),
            std::mem::discriminant(&again),
            "re-encoding changed the frame type"
        );
    }
});
