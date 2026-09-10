//! Coverage-guided fuzzing of the handshake's identity frame.
//!
//! `IdentityProof` is bincode-decoded inside the v3 tunnel **before any trust
//! decision has been made** (`core/src/network/session.rs`). At that point the
//! peer is authenticated to nothing: the tunnel proves only that somebody
//! completed an ECDH, and this struct is what they send to claim who they are.
//! Everything downstream — the signature check, the fingerprint, TOFU — reads
//! fields this decoder produced.
//!
//! bincode is not self-describing, so a malformed frame is entirely a question
//! of how the decoder handles nonsense: a length prefix it believes decides how
//! much it allocates.
#![no_main]

use libfuzzer_sys::fuzz_target;
use messenger_core::core::protocol::IdentityProof;
use messenger_core::util::decode_exact;

fuzz_target!(|data: &[u8]| {
    // `decode_exact`, not `bincode::deserialize` — that is what the handshake
    // calls, and the difference is the point: it refuses bytes left over after
    // the proof, so `proof || junk` is not silently the same frame as `proof`.
    //
    // Contract 1: never panic, and never allocate on a declared length alone —
    // a `Vec<u8>` field whose prefix claims 4 GiB must fail, not reserve.
    let Ok(proof) = decode_exact::<IdentityProof>(data) else {
        return;
    };

    // Contract 2: round trip. A decoder that accepts what its own encoder
    // cannot reproduce is holding a state nothing downstream expects.
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

    // Contract 3: trailing bytes are refused. This frame is read before any
    // trust decision, so two byte strings with one authenticated meaning is a
    // property worth pinning at the earliest reachable point.
    let mut padded = re_encoded.clone();
    padded.push(0);
    assert!(
        decode_exact::<IdentityProof>(&padded).is_err(),
        "an IdentityProof with a trailing byte decoded to the frame without it"
    );
});
