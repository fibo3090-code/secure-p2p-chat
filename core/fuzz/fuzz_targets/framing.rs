//! Coverage-guided fuzzing of the length-prefixed packet reader.
//!
//! `recv_packet` sits **earlier in the trust chain than anything else fuzzed
//! here**. It runs before the handshake and before any decryption, on the first
//! four bytes anyone who can open a TCP socket sends. Every other parser in this
//! directory is reachable only after a peer has completed a v3 handshake;
//! this one is reachable by a port scanner.
//!
//! What it must do with a hostile prefix: refuse a length past
//! `MAX_PACKET_SIZE` without allocating for it, and never read more than the
//! prefix promised.
#![no_main]

use libfuzzer_sys::fuzz_target;
use messenger_core::core::framing::recv_packet;
use messenger_core::MAX_PACKET_SIZE;

fuzz_target!(|data: &[u8]| {
    // A current-thread runtime per input: `recv_packet` is async, and the
    // property being tested is entirely about what it does with the bytes.
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");

    rt.block_on(async {
        let mut cursor = std::io::Cursor::new(data);
        // A short or oversized frame is an `Err`, which is the correct outcome;
        // what matters is that it is an error and not a panic or a
        // multi-gigabyte allocation. Only the success path has assertions.
        if let Ok(packet) = recv_packet(&mut cursor).await {
            // Whatever came back must be the length the prefix declared, and
            // that length must be within the cap.
            assert!(
                packet.len() <= MAX_PACKET_SIZE,
                "returned a packet past the cap: {} bytes",
                packet.len()
            );
            let declared =
                u32::from_be_bytes(data[..4].try_into().expect("read succeeded")) as usize;
            assert_eq!(
                packet.len(),
                declared,
                "returned a different number of bytes than the prefix declared"
            );
            // And it must not have invented bytes the input did not contain.
            assert!(
                data.len() >= 4 + packet.len(),
                "returned more bytes than the input held"
            );
            assert_eq!(&packet[..], &data[4..4 + packet.len()]);
        }
    });
});
