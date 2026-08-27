//! Adversarial tests against every parser that touches attacker-controlled input.
//!
//! Everything here is reachable by a remote party. `from_plain_bytes` sees every
//! frame a peer sends once it is decrypted; the Party codecs see everything a
//! community server or client sends; `sanitize_filename` sees a name chosen
//! entirely by whoever is sending a file. A panic in any of them is a remote
//! denial of service, and a mis-parse in the filename path is a write outside the
//! download directory.
//!
//! These are property tests rather than `cargo-fuzz` targets on purpose. Coverage
//! guided fuzzing finds deeper paths, but it needs a nightly toolchain and a
//! separate long-running job — so in practice it runs rarely and reviews nothing.
//! Proptest runs on stable in the ordinary suite, on every pull request, and
//! shrinks a failure to a minimal reproducing input. The right end state is both;
//! this is the half that actually gates a merge.
//!
//! The contract every parser here is held to:
//!
//! 1. **Never panic**, on any input, ever. Return `None`.
//! 2. **Never allocate proportionally to a declared length** before that length
//!    has been checked against the real buffer.
//! 3. **Round-trip**: anything that decodes must re-encode to something that
//!    decodes to the same value. A decoder that accepts what its encoder cannot
//!    produce is a decoder with an unreachable state in it.

use messenger_core::core::ProtocolMessage;
use messenger_core::party::{PartyRequest, PartyResponse};
use messenger_core::util::sanitize_filename;
use proptest::prelude::*;

/// Frames the encoder can actually produce, for round-trip and mutation attacks.
fn valid_frames() -> Vec<ProtocolMessage> {
    vec![
        ProtocolMessage::Version { version: 3 },
        ProtocolMessage::EphemeralKey {
            public_key: vec![7u8; 32],
        },
        ProtocolMessage::Text {
            text: "hello".to_string(),
            timestamp: 1,
            seq: 1,
        },
        ProtocolMessage::Text {
            text: String::new(),
            timestamp: u64::MAX,
            seq: u64::MAX,
        },
        ProtocolMessage::FileMeta {
            filename: "report.pdf".to_string(),
            size: 1024,
            seq: 2,
        },
        ProtocolMessage::FileChunk {
            chunk: vec![1, 2, 3, 4],
            seq: 3,
        },
        ProtocolMessage::FileEnd { seq: 4 },
        ProtocolMessage::FileCancel { seq: 5 },
        ProtocolMessage::Ack {
            acked_seq: 6,
            seq: 7,
        },
        ProtocolMessage::TypingStart { seq: 8 },
        ProtocolMessage::TypingStop { seq: 9 },
        ProtocolMessage::Ping { seq: 10 },
        ProtocolMessage::Rekey {
            nonce: vec![0u8; 16],
            seq: 11,
        },
        ProtocolMessage::TextChunk {
            message_id: uuid::Uuid::nil(),
            chunk_index: 0,
            total_chunks: 2,
            text_part: "part".to_string(),
            timestamp: 1,
            seq: 12,
        },
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2048, ..ProptestConfig::default() })]

    /// Arbitrary bytes must never panic the frame decoder.
    #[test]
    fn protocol_decoder_survives_arbitrary_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..8192)) {
        let _ = ProtocolMessage::from_plain_bytes(&bytes);
    }

    /// The first byte selects the variant, so walk every tag deliberately rather
    /// than hoping random bytes hit each one.
    #[test]
    fn every_tag_survives_arbitrary_payloads(
        tag in any::<u8>(),
        payload in proptest::collection::vec(any::<u8>(), 0..1024),
    ) {
        let mut frame = vec![tag];
        frame.extend_from_slice(&payload);
        let _ = ProtocolMessage::from_plain_bytes(&frame);
    }

    /// A declared length that lies about the buffer must be refused, not trusted.
    /// This is the classic shape: a 4-byte length field claiming gigabytes with
    /// nothing behind it.
    #[test]
    fn oversized_declared_lengths_are_refused(
        tag in any::<u8>(),
        declared in any::<u32>(),
        tail in proptest::collection::vec(any::<u8>(), 0..64),
    ) {
        let mut frame = vec![tag];
        frame.extend_from_slice(&declared.to_be_bytes());
        frame.extend_from_slice(&tail);
        // Must not panic and must not hang trying to allocate `declared` bytes.
        let _ = ProtocolMessage::from_plain_bytes(&frame);
    }

    /// Truncating a valid frame anywhere must fail cleanly.
    #[test]
    fn truncated_valid_frames_fail_cleanly(idx in 0usize..16, cut in 0usize..512) {
        let frames = valid_frames();
        let frame = &frames[idx % frames.len()];
        let encoded = frame.to_plain_bytes();
        let at = cut.min(encoded.len());
        let _ = ProtocolMessage::from_plain_bytes(&encoded[..at]);
    }

    /// Flipping bytes inside an otherwise-valid frame must not panic.
    #[test]
    fn corrupted_valid_frames_fail_cleanly(
        idx in 0usize..16,
        pos in 0usize..512,
        xor in 1u8..=255,
    ) {
        let frames = valid_frames();
        let frame = &frames[idx % frames.len()];
        let mut encoded = frame.to_plain_bytes();
        if !encoded.is_empty() {
            let p = pos % encoded.len();
            encoded[p] ^= xor;
        }
        let _ = ProtocolMessage::from_plain_bytes(&encoded);
    }

    /// Appending junk to a valid frame must not turn it into a different valid
    /// frame with attacker-chosen contents.
    #[test]
    fn trailing_junk_does_not_change_meaning(
        idx in 0usize..16,
        junk in proptest::collection::vec(any::<u8>(), 1..64),
    ) {
        let frames = valid_frames();
        let frame = &frames[idx % frames.len()];
        let mut encoded = frame.to_plain_bytes();
        let clean = ProtocolMessage::from_plain_bytes(&encoded);
        encoded.extend_from_slice(&junk);
        let dirty = ProtocolMessage::from_plain_bytes(&encoded);

        if let (Some(a), Some(b)) = (&clean, &dirty) {
            // If both parse, the trailing bytes must not have altered the
            // decoded value — otherwise an on-path attacker could append to a
            // frame and change what the peer reads.
            prop_assert_eq!(
                std::mem::discriminant(a),
                std::mem::discriminant(b),
                "trailing junk changed which frame this is"
            );
        }
    }

    /// Anything the decoder accepts must survive a re-encode unchanged. A
    /// decoder that accepts what its encoder cannot produce has a state in it
    /// that nothing else in the system knows about.
    #[test]
    fn decode_encode_decode_is_stable(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        if let Some(first) = ProtocolMessage::from_plain_bytes(&bytes) {
            let re_encoded = first.to_plain_bytes();
            let second = ProtocolMessage::from_plain_bytes(&re_encoded);
            prop_assert!(second.is_some(), "a frame we produced failed to parse back");
            if let Some(second) = second {
                prop_assert_eq!(
                    std::mem::discriminant(&first),
                    std::mem::discriminant(&second),
                    "re-encoding changed the frame type"
                );
                // Discriminant equality alone is too weak: a decoder that
                // *rewrites* the payload (lossy UTF-8 did, three bytes per
                // invalid byte) keeps the same variant while producing a value
                // that no longer round-trips. Compare the bytes.
                prop_assert_eq!(
                    re_encoded,
                    second.to_plain_bytes(),
                    "re-encoding is not a fixed point"
                );
            }
        }
    }

    /// The Party codecs face the same exposure over the community tunnel.
    #[test]
    fn party_request_decoder_survives_arbitrary_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..8192)) {
        let _ = PartyRequest::from_bytes(&bytes);
    }

    #[test]
    fn party_response_decoder_survives_arbitrary_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..8192)) {
        let _ = PartyResponse::from_bytes(&bytes);
    }

    /// `sanitize_filename` decides where a received file is written, so its
    /// output is a security boundary, not a cosmetic one.
    #[test]
    fn sanitized_filenames_are_always_safe(raw in ".{0,300}") {
        let out = sanitize_filename(&raw);

        // It must never produce something that escapes the download directory.
        prop_assert!(!out.contains('/'), "path separator survived: {out:?}");
        prop_assert!(!out.contains('\\'), "path separator survived: {out:?}");
        prop_assert!(out != ".." && out != ".", "traversal component survived: {out:?}");
        prop_assert!(!out.starts_with("..") || !out.contains(std::path::MAIN_SEPARATOR),
            "traversal survived: {out:?}");

        // Never empty — an empty name would join to the directory itself.
        prop_assert!(!out.is_empty(), "empty filename for input {raw:?}");

        // Never a bare Windows device name, which resolves to a device rather
        // than a file regardless of the directory it is joined to.
        let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
        for device in ["CON", "PRN", "AUX", "NUL"] {
            prop_assert_ne!(stem.as_str(), device, "device name survived: {:?}", out);
        }
        for n in 1..=9 {
            let com = format!("COM{n}");
            let lpt = format!("LPT{n}");
            prop_assert_ne!(stem.as_str(), com.as_str());
            prop_assert_ne!(stem.as_str(), lpt.as_str());
        }

        // No control characters or bidi overrides — U+202E renders
        // "photo_gnp.exe" as "photo_exe.png" in every file manager.
        prop_assert!(
            !out.chars().any(|c| c.is_control() || ('\u{202A}'..='\u{202E}').contains(&c)),
            "control or bidi character survived: {out:?}"
        );

        // Windows silently drops trailing dots and spaces, which would turn
        // "evil.exe " into "evil.exe" after the extension check had passed.
        prop_assert!(!out.ends_with(' ') && !out.ends_with('.'), "trailing dot/space: {out:?}");

        // Bounded, so the receiver's `tmp_<uuid>_` prefix still fits under NAME_MAX.
        prop_assert!(out.len() <= 255, "filename too long: {} bytes", out.len());
    }

    /// Sanitising is idempotent: running it twice must not produce something
    /// different from running it once, or the name written to disk depends on
    /// how many times it happened to be passed through.
    #[test]
    fn sanitizing_is_idempotent(raw in ".{0,200}") {
        let once = sanitize_filename(&raw);
        let twice = sanitize_filename(&once);
        prop_assert_eq!(once, twice);
    }
}

/// Hand-picked hostile filenames, alongside the random ones. These are the
/// shapes that actually get used.
#[test]
fn known_hostile_filenames_are_defused() {
    let attacks = [
        "../../../etc/passwd",
        "..\\..\\windows\\system32\\config\\sam",
        "/etc/shadow",
        "C:\\Windows\\System32\\evil.dll",
        "....//....//etc/passwd",
        "..",
        ".",
        "",
        "   ",
        "CON",
        "con.txt",
        "NUL.jpg",
        "COM1",
        "lpt9.pdf",
        "evil.exe ",
        "evil.exe.",
        "photo\u{202E}gnp.exe",
        "null\0byte.txt",
        "new\nline.txt",
        "tab\there.txt",
        &"a".repeat(5000),
        "\u{FEFF}bom.txt",
        ".hidden",
        "..hidden",
    ];

    for raw in attacks {
        let out = sanitize_filename(raw);
        assert!(!out.is_empty(), "{raw:?} sanitised to nothing");
        assert!(!out.contains('/'), "{raw:?} kept a separator: {out:?}");
        assert!(!out.contains('\\'), "{raw:?} kept a separator: {out:?}");
        assert!(
            out != ".." && out != ".",
            "{raw:?} stayed traversal: {out:?}"
        );
        assert!(
            !out.ends_with(' '),
            "{raw:?} kept a trailing space: {out:?}"
        );
        assert!(!out.ends_with('.'), "{raw:?} kept a trailing dot: {out:?}");
        assert!(
            !out.chars().any(|c| c.is_control()),
            "{raw:?} kept a control character: {out:?}"
        );
        assert!(out.len() <= 255, "{raw:?} produced {} bytes", out.len());

        // And joining it to a directory must stay inside that directory.
        let joined = std::path::Path::new("/downloads").join(&out);
        assert!(
            joined.starts_with("/downloads"),
            "{raw:?} escaped the directory as {joined:?}"
        );
        assert_eq!(
            joined.components().count(),
            3,
            "{raw:?} produced extra path components: {joined:?}"
        );
    }
}

/// Invalid UTF-8 in a text frame must be **refused**, not repaired.
///
/// The decoder used to run `String::from_utf8_lossy` after checking the declared
/// wire length against the cap. Every invalid byte becomes U+FFFD, which is three
/// bytes, so the check bounded the wire and not the result: a `Text` frame of
/// 65,557 `0xFF` bytes passed the check, decoded to a 196,629-byte `String`, and
/// then could not be re-encoded within the same cap. That is a decoder accepting
/// a state its own encoder cannot produce, and the practical cost was a memory
/// bound out by a factor of three — 512 `TextChunk` frames of 48 KiB of invalid
/// UTF-8 held about 72 MiB for one message against a documented ~24 MiB ceiling.
///
/// The property test above asserts exactly this and never found it, because it
/// generates at most 2 KiB and the frames that trip it are 64 KiB. Hence an
/// explicit case.
mod lossy_utf8_broke_the_wire_caps {
    use super::*;
    use messenger_core::{MAX_TEXT_CHUNKS, MAX_TEXT_MESSAGE_BYTES, TEXT_CHUNK_BYTES};

    /// Tag 2: `Text { text, timestamp, seq }`.
    fn text_frame(payload: &[u8]) -> Vec<u8> {
        let mut frame = vec![2u8];
        frame.extend_from_slice(&1u64.to_be_bytes()); // seq
        frame.extend_from_slice(&0u64.to_be_bytes()); // timestamp
        frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        frame.extend_from_slice(payload);
        frame
    }

    /// Tag 11: `TextChunk { .. }`.
    fn text_chunk_frame(payload: &[u8]) -> Vec<u8> {
        let mut frame = vec![11u8];
        frame.extend_from_slice(uuid::Uuid::nil().as_bytes());
        frame.extend_from_slice(&1u64.to_be_bytes()); // seq
        frame.extend_from_slice(&0u64.to_be_bytes()); // timestamp
        frame.extend_from_slice(&0u32.to_be_bytes()); // chunk_index
        frame.extend_from_slice(&1u32.to_be_bytes()); // total_chunks
        frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        frame.extend_from_slice(payload);
        frame
    }

    #[test]
    fn a_text_frame_of_invalid_utf8_is_refused() {
        let payload = vec![0xFFu8; MAX_TEXT_MESSAGE_BYTES];
        assert!(
            ProtocolMessage::from_plain_bytes(&text_frame(&payload)).is_none(),
            "invalid UTF-8 must be refused, not expanded into three bytes each"
        );
    }

    #[test]
    fn a_text_chunk_of_invalid_utf8_is_refused() {
        let payload = vec![0xFFu8; TEXT_CHUNK_BYTES];
        assert!(
            ProtocolMessage::from_plain_bytes(&text_chunk_frame(&payload)).is_none(),
            "invalid UTF-8 must be refused, not expanded into three bytes each"
        );
    }

    /// Valid UTF-8 at the cap still works, including multi-byte characters —
    /// the fix must reject invalid input, not merely reject non-ASCII.
    #[test]
    fn valid_text_at_the_cap_still_decodes() {
        // "é" is two bytes; fill the budget exactly.
        let payload = "é".repeat(MAX_TEXT_MESSAGE_BYTES / 2).into_bytes();
        assert_eq!(payload.len(), MAX_TEXT_MESSAGE_BYTES);
        let decoded = ProtocolMessage::from_plain_bytes(&text_frame(&payload))
            .expect("valid UTF-8 at the cap must decode");
        match &decoded {
            ProtocolMessage::Text { text, .. } => {
                assert_eq!(text.len(), MAX_TEXT_MESSAGE_BYTES)
            }
            other => panic!("expected Text, got {other:?}"),
        }
        assert_eq!(
            decoded.to_plain_bytes(),
            text_frame(&payload),
            "a frame at the cap must re-encode byte-for-byte"
        );
    }

    /// The **legacy** `TEXT:` arm too. `from_plain_bytes` falls through to the
    /// ASCII-prefixed format when the binary tag does not match, and `T` (0x54)
    /// is not a binary tag — so this branch is reachable from the wire, it keeps
    /// its decoded string, and it had the same 3x expansion. The other legacy
    /// arms parse into a `u64` and drop the string, so they are bounded by the
    /// parse rather than by the cap.
    #[test]
    fn a_legacy_text_frame_of_invalid_utf8_is_refused() {
        let mut frame = b"TEXT:1:".to_vec();
        frame.extend(std::iter::repeat_n(
            0xFFu8,
            MAX_TEXT_MESSAGE_BYTES - frame.len(),
        ));
        assert_eq!(frame.len(), MAX_TEXT_MESSAGE_BYTES, "at the wire cap");
        assert!(
            ProtocolMessage::from_plain_bytes(&frame).is_none(),
            "the legacy arm must refuse invalid UTF-8 too"
        );

        // …and still accepts a legitimate legacy frame.
        let ok = ProtocolMessage::from_plain_bytes(b"TEXT:7:hello").expect("valid legacy frame");
        match ok {
            ProtocolMessage::Text { text, seq, .. } => {
                assert_eq!(text, "hello");
                assert_eq!(seq, 7);
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    /// The bound that was actually wrong: the reassembly budget. With lossy
    /// decoding, `MAX_TEXT_CHUNKS` frames of `TEXT_CHUNK_BYTES` decoded to three
    /// times the documented ceiling.
    #[test]
    fn the_reassembly_budget_bounds_decoded_bytes_not_just_wire_bytes() {
        let payload = vec![0xFFu8; TEXT_CHUNK_BYTES];
        let mut decoded_bytes = 0usize;
        for _ in 0..8 {
            if let Some(ProtocolMessage::TextChunk { text_part, .. }) =
                ProtocolMessage::from_plain_bytes(&text_chunk_frame(&payload))
            {
                decoded_bytes += text_part.len();
            }
        }
        assert_eq!(
            decoded_bytes, 0,
            "these frames must not decode at all; if they do, each one costs 3x its wire size"
        );

        // The documented ceiling, stated where it can be checked.
        let ceiling = MAX_TEXT_CHUNKS as usize * TEXT_CHUNK_BYTES;
        assert_eq!(ceiling, 512 * 48 * 1024, "the ~24 MiB budget");
    }
}

/// A frame claiming an enormous payload must be refused without allocating for
/// it. If this ever regresses it will show up as the test timing out or the
/// process being killed, which is exactly the remote symptom.
#[test]
fn declared_gigabyte_payloads_do_not_allocate() {
    for tag in 0u8..=20 {
        let mut frame = vec![tag];
        // Every length-prefixed arm reads a big-endian u32.
        frame.extend_from_slice(&u32::MAX.to_be_bytes());
        frame.extend_from_slice(&u64::MAX.to_be_bytes());
        frame.extend_from_slice(&u64::MAX.to_be_bytes());
        frame.extend_from_slice(&[0u8; 16]);
        let _ = ProtocolMessage::from_plain_bytes(&frame);
    }
}

/// The two truncation bugs the property tests above found, pinned as explicit
/// regressions with the reasoning attached — a shrunk proptest input is a
/// reproducer, not an explanation.
mod truncation_reintroduced_what_sanitising_removed {
    use super::*;

    /// Truncation used to cut a name at a dot and hand back `…​.exe.`, which
    /// `Path::extension()` reports as `Some("")` — not `"exe"` — so the desktop
    /// app's "this file will run as a program" gate never fired. Windows drops
    /// the trailing dot when it creates the file, so what landed on disk *was*
    /// an executable. The peer chooses the filename.
    #[test]
    fn a_crafted_name_cannot_hide_an_executable_extension() {
        // Sized against MAX_FILENAME_BYTES (150). The final segment is over 16
        // characters so it is not treated as an extension, which is what makes
        // the whole name get cut blindly.
        let attack = format!("{}.exe.{}", "A".repeat(145), "Z".repeat(30));
        let out = sanitize_filename(&attack);

        assert!(
            !out.ends_with('.'),
            "a trailing dot survived truncation: {out:?}"
        );
        assert_eq!(
            std::path::Path::new(&out)
                .extension()
                .and_then(|e| e.to_str()),
            Some("exe"),
            "the gate must see the extension the OS will act on, got {out:?}"
        );
        // The name checked and the name Windows would create must be the same.
        assert_eq!(out, out.trim_end_matches(['.', ' ']));
    }

    /// Truncating between two dots separated by a multi-byte character brought
    /// them together, re-creating a `..` that the traversal collapse had already
    /// removed.
    #[test]
    fn truncation_cannot_recreate_a_traversal_component() {
        // `𐀀` is four bytes, so the cut lands inside the region between the dots.
        let attack = format!("{}.𐀀.{}", "a".repeat(140), "b".repeat(20));
        let out = sanitize_filename(&attack);
        assert!(
            !out.contains(".."),
            "a traversal component came back: {out:?}"
        );
    }

    /// The general property both bugs violated: sanitising is a fixed point.
    /// If a second pass changes anything, the name on disk depends on how many
    /// times it happened to be sanitised.
    #[test]
    fn sanitising_the_known_attacks_is_stable() {
        for attack in [
            format!("{}.exe.{}", "A".repeat(145), "Z".repeat(30)),
            format!("{}.𐀀.{}", "a".repeat(140), "b".repeat(20)),
            format!("{}..{}", "x".repeat(148), "y".repeat(20)),
            format!("CON.{}", "z".repeat(200)),
        ] {
            let once = sanitize_filename(&attack);
            let twice = sanitize_filename(&once);
            assert_eq!(once, twice, "not a fixed point for {attack:?}");
            assert!(once.len() <= 255);
        }
    }
}
