//! Coverage-guided fuzzing of the community-server codecs.
//!
//! `PartyRequest` is what a server parses from every client, and `PartyResponse`
//! is what a client parses from every server — including a server it has just
//! met and not yet decided to trust. Both are bincode, which is a format with no
//! self-description, so a malformed frame is entirely a question of how the
//! decoder handles nonsense.
//!
//! ## What this asserts, and why it used to assert nothing
//!
//! This target was two `let _ = …from_bytes(data);` lines. `from_bytes` returns
//! an `Option`, so those calls could not fail: no panic, no over-allocation, no
//! property at all. It spent a third of every fuzzing budget confirming that a
//! function returns. The two properties below are the ones that can actually
//! break, and both are one line.
#![no_main]

use libfuzzer_sys::fuzz_target;
use messenger_core::party::{PartyRequest, PartyResponse};

fuzz_target!(|data: &[u8]| {
    // Both directions, since the client trusts the server's frames as much as
    // the server trusts the client's.
    if let Some(req) = PartyRequest::from_bytes(data) {
        // Round trip: anything the decoder accepts must survive a re-encode.
        // A decoder that accepts what its own encoder cannot reproduce holds a
        // state nothing downstream knows how to handle.
        let re_encoded = req.to_bytes();
        let again = PartyRequest::from_bytes(&re_encoded)
            .expect("a frame this decoder produced must parse back");
        assert_eq!(
            re_encoded,
            again.to_bytes(),
            "re-encoding a PartyRequest is not a fixed point"
        );

        // Trailing junk must not be accepted. bincode 1.x stops at the end of
        // the value and silently ignores whatever follows, so without this two
        // *distinct* frames decode identically — which turns a frame's bytes
        // into something an on-path party can pad without changing its meaning.
        let mut padded = re_encoded.clone();
        padded.push(0);
        assert!(
            PartyRequest::from_bytes(&padded).is_none(),
            "a PartyRequest with trailing bytes must be refused, not truncated to fit"
        );
    }

    if let Some(resp) = PartyResponse::from_bytes(data) {
        let re_encoded = resp.to_bytes();
        let again = PartyResponse::from_bytes(&re_encoded)
            .expect("a frame this decoder produced must parse back");
        assert_eq!(
            re_encoded,
            again.to_bytes(),
            "re-encoding a PartyResponse is not a fixed point"
        );

        let mut padded = re_encoded.clone();
        padded.push(0);
        assert!(
            PartyResponse::from_bytes(&padded).is_none(),
            "a PartyResponse with trailing bytes must be refused, not truncated to fit"
        );
    }
});
