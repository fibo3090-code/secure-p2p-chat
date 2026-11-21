//! Coverage-guided fuzzing of the peer-to-peer frame decoder.
//!
//! `from_plain_bytes` sees every decrypted frame a peer sends: the largest
//! attacker-controlled parser, reachable after a handshake. The property suite
//! (`core/tests/fuzz_parsers.rs`) gates every PR; this coverage-guided half is
//! run deliberately and longer, reaching branches random inputs rarely hit.
#![no_main]

use libfuzzer_sys::fuzz_target;
use messenger_core::core::ProtocolMessage;

fuzz_target!(|data: &[u8]| {
    // Contract 1: never panic, on any input.
    if let Some(decoded) = ProtocolMessage::from_plain_bytes(data) {
        // Contract 2: anything accepted survives a round trip.
        let re_encoded = decoded.to_plain_bytes();
        let again = ProtocolMessage::from_plain_bytes(&re_encoded)
            .expect("a frame this decoder produced must parse back");
        assert_eq!(
            std::mem::discriminant(&decoded),
            std::mem::discriminant(&again),
            "re-encoding changed the frame type"
        );
        // On the *bytes*, not just the type, or a decoder that silently alters
        // content passes. From the second encode on: the legacy `TEXT:` arm
        // stamps the current clock.
        assert_eq!(
            re_encoded,
            again.to_plain_bytes(),
            "encoding is not a fixpoint — the decoder altered the frame"
        );
    }
});
