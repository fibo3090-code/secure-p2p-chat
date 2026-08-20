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
    // Both directions, since the client trusts the server's frames as much as
    // the server trusts the client's.
    let _ = PartyRequest::from_bytes(data);
    let _ = PartyResponse::from_bytes(data);
});
