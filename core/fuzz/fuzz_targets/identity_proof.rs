//! Coverage-guided fuzzing of the handshake's identity frame. `IdentityProof` is
//! decoded **before any trust decision** (`core/src/network/session.rs`), from a
//! peer authenticated to nothing, and the signature check, fingerprint and TOFU
//! all read what this decoder produced. bincode is not self-describing, so a
//! believed length prefix decides how much it allocates.
#![no_main]

use libfuzzer_sys::fuzz_target;
use messenger_core::core::protocol::IdentityProof;
use messenger_core::util::decode_exact;

fuzz_target!(|data: &[u8]| {
    // `decode_exact`, as the handshake calls it (`proof || junk` is refused).
    // Contract 1: never panic, never allocate on a declared length alone.
    let Ok(proof) = decode_exact::<IdentityProof>(data) else {
        return;
    };

    // Contract 2: round trip — accept only what the encoder can reproduce.
    let re_encoded = bincode::serialize(&proof).expect("serialization is infallible");
    let again: IdentityProof =
        decode_exact(&re_encoded).expect("a frame this decoder produced must parse back");
    assert_eq!(
        re_encoded,
        bincode::serialize(&again).expect("serialization is infallible"),
        "re-encoding an IdentityProof is not a fixed point"
    );
    assert_eq!(proof.signature_scheme, again.signature_scheme);
    assert_eq!(proof.chat_id, again.chat_id);
    assert_eq!(proof.version, again.version);

    // Contract 3: trailing bytes are refused (no two encodings of one proof).
    let mut padded = re_encoded.clone();
    padded.push(0);
    assert!(
        decode_exact::<IdentityProof>(&padded).is_err(),
        "an IdentityProof with a trailing byte decoded to the frame without it"
    );
});
