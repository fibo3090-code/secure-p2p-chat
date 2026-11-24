//! Coverage-guided fuzzing of the length-prefixed packet reader. `recv_packet`
//! runs before the handshake and any decryption, so it is reachable by a port
//! scanner: it must refuse a length past `MAX_PACKET_SIZE` without allocating for
//! it, and never read more than the prefix promised.
#![no_main]

use libfuzzer_sys::fuzz_target;
use messenger_core::core::framing::recv_packet;
use messenger_core::MAX_PACKET_SIZE;

fuzz_target!(|data: &[u8]| {
    // A current-thread runtime per input (`recv_packet` is async).
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");

    rt.block_on(async {
        let mut cursor = std::io::Cursor::new(data);
        // A short or oversized frame must be an `Err`, not a panic or a huge
        // allocation; only success has assertions.
        if let Ok(packet) = recv_packet(&mut cursor).await {
            // Exactly the declared length, within the cap, and from the input.
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
            assert!(
                data.len() >= 4 + packet.len(),
                "returned more bytes than the input held"
            );
            assert_eq!(&packet[..], &data[4..4 + packet.len()]);
        }
    });
});
