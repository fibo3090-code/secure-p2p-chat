//! Coverage-guided fuzzing of the community-server codecs: `PartyRequest` (parsed
//! by the server from every client) and `PartyResponse` (parsed by a client, even
//! from a server it has not yet decided to trust). Both are bincode.
#![no_main]

use libfuzzer_sys::fuzz_target;
use messenger_core::party::{PartyRequest, PartyResponse};

fuzz_target!(|data: &[u8]| {
    // Contract 1: never panic, in either direction — the client trusts the
    // server's frames as much as the server trusts the client's.
    let request = PartyRequest::from_bytes(data);
    let response = PartyResponse::from_bytes(data);

    // Contract 2: anything accepted round-trips *byte for byte* (bincode's accept
    // set is easily wider than its output set).
    if let Some(request) = request {
        let re_encoded = request.to_bytes();
        let again = PartyRequest::from_bytes(&re_encoded)
            .expect("a frame this encoder produced must parse back");
        assert_eq!(
            re_encoded,
            again.to_bytes(),
            "PartyRequest encoding is not a fixpoint"
        );

        // Contract 3: trailing bytes are refused, or two byte strings share one
        // meaning.
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
