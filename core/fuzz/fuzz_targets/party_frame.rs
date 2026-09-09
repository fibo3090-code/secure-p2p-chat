//! Coverage-guided fuzzing of the community-server codecs.
//!
//! `PartyRequest` is what a server parses from every client, and `PartyResponse`
//! is what a client parses from every server — including a server it has just
//! met and not yet decided to trust. Both are bincode, which is a format with no
//! self-description, so a malformed frame is entirely a question of how the
//! decoder handles nonsense.
#![no_main]

use libfuzzer_sys::fuzz_target;
use messenger_core::party::{PartyRequest, PartyResponse};

fuzz_target!(|data: &[u8]| {
    // Contract 1: never panic, in either direction — the client trusts the
    // server's frames as much as the server trusts the client's.
    let request = PartyRequest::from_bytes(data);
    let response = PartyResponse::from_bytes(data);

    // Contract 2: anything accepted must survive a round trip *byte for byte*.
    //
    // Without this the target asserted nothing at all: both calls were
    // `...ok()` and discarded, and a decoder that cannot panic structurally
    // cannot fail such a target no matter how long it runs. Encoding is the
    // property worth checking here, because bincode's accept set is easy to
    // make wider than its output set.
    if let Some(request) = request {
        let re_encoded = request.to_bytes();
        let again = PartyRequest::from_bytes(&re_encoded)
            .expect("a frame this encoder produced must parse back");
        assert_eq!(
            re_encoded,
            again.to_bytes(),
            "PartyRequest encoding is not a fixpoint"
        );

        // Contract 3: trailing bytes are not free. Bincode stops at the end of
        // a value and ignores the rest, which gives two distinct byte strings
        // one meaning — an on-path attacker appends to a frame and changes its
        // bytes without changing what the peer reads.
        let mut padded = re_encoded.clone();
        padded.push(0);
        assert!(
            PartyRequest::from_bytes(&padded).is_none(),
            "a PartyRequest with a trailing byte decoded to the frame without it"
        );
    }

    if let Some(response) = response {
        let re_encoded = response.to_bytes();
        let again = PartyResponse::from_bytes(&re_encoded)
            .expect("a frame this encoder produced must parse back");
        assert_eq!(
            re_encoded,
            again.to_bytes(),
            "PartyResponse encoding is not a fixpoint"
        );

        let mut padded = re_encoded.clone();
        padded.push(0);
        assert!(
            PartyResponse::from_bytes(&padded).is_none(),
            "a PartyResponse with a trailing byte decoded to the frame without it"
        );
    }
});
