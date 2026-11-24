//! Property tests (proptest, on stable, gating every PR) against every parser
//! that touches attacker-controlled input; `core/fuzz/` is the coverage-guided
//! half. A panic here is a remote DoS, a filename mis-parse a write outside the
//! download directory. Every parser must:
//!
//! 1. **Never panic** on any input; return `None`.
//! 2. **Never allocate proportionally to a declared length** before checking it
//!    against the real buffer.
//! 3. **Round-trip**: anything that decodes re-encodes to the same value.

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

/// `valid_frames()[idx % len]`, encoded.
fn encoded_frame(idx: usize) -> Vec<u8> {
    let frames = valid_frames();
    frames[idx % frames.len()].to_plain_bytes()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2048, ..ProptestConfig::default() })]

    /// Arbitrary bytes must never panic the frame decoder.
    #[test]
    fn protocol_decoder_survives_arbitrary_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..8192)) {
        let _ = ProtocolMessage::from_plain_bytes(&bytes);
    }

    /// Walk every first-byte tag deliberately.
    #[test]
    fn every_tag_survives_arbitrary_payloads(
        tag in any::<u8>(),
        payload in proptest::collection::vec(any::<u8>(), 0..1024),
    ) {
        let mut frame = vec![tag];
        frame.extend_from_slice(&payload);
        let _ = ProtocolMessage::from_plain_bytes(&frame);
    }

    /// A declared length that lies about the buffer (4 bytes claiming
    /// gigabytes) must be refused without allocating it.
    #[test]
    fn oversized_declared_lengths_are_refused(
        tag in any::<u8>(),
        declared in any::<u32>(),
        tail in proptest::collection::vec(any::<u8>(), 0..64),
    ) {
        let mut frame = vec![tag];
        frame.extend_from_slice(&declared.to_be_bytes());
        frame.extend_from_slice(&tail);
        let _ = ProtocolMessage::from_plain_bytes(&frame);
    }

    /// Truncating a valid frame anywhere must fail cleanly.
    #[test]
    fn truncated_valid_frames_fail_cleanly(idx in 0usize..16, cut in 0usize..512) {
        let encoded = encoded_frame(idx);
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
        let mut encoded = encoded_frame(idx);
        if !encoded.is_empty() {
            let p = pos % encoded.len();
            encoded[p] ^= xor;
        }
        let _ = ProtocolMessage::from_plain_bytes(&encoded);
    }

    /// Appending junk must not turn a frame into a different one.
    #[test]
    fn trailing_junk_does_not_change_meaning(
        idx in 0usize..16,
        junk in proptest::collection::vec(any::<u8>(), 1..64),
    ) {
        let mut encoded = encoded_frame(idx);
        let clean = ProtocolMessage::from_plain_bytes(&encoded);
        encoded.extend_from_slice(&junk);
        let dirty = ProtocolMessage::from_plain_bytes(&encoded);

        if let (Some(a), Some(b)) = (&clean, &dirty) {
            prop_assert_eq!(
                std::mem::discriminant(a),
                std::mem::discriminant(b),
                "trailing junk changed which frame this is"
            );
        }
    }

    /// Anything the decoder accepts re-encodes to a byte fixpoint. Bytes, not
    /// discriminants: a discriminant check passes a decoder that alters content.
    /// Compared from the second encode on, as legacy `TEXT:` stamps the clock.
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
                prop_assert_eq!(
                    &re_encoded,
                    &second.to_plain_bytes(),
                    "encoding is not a fixpoint — the decoder altered the frame"
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

    /// Party frames reject trailing bytes (bincode ignores them by default,
    /// giving two byte strings one meaning).
    #[test]
    fn party_frames_reject_trailing_bytes(
        junk in proptest::collection::vec(any::<u8>(), 1..64),
    ) {
        let req = PartyRequest::ListChannels;
        let mut bytes = req.to_bytes();
        prop_assert!(
            PartyRequest::from_bytes(&bytes).is_some(),
            "the encoder's own output must parse back"
        );
        bytes.extend_from_slice(&junk);
        prop_assert!(
            PartyRequest::from_bytes(&bytes).is_none(),
            "trailing bytes decoded to the frame without them"
        );

        let resp = PartyResponse::Error("nope".to_string());
        let mut bytes = resp.to_bytes();
        prop_assert!(PartyResponse::from_bytes(&bytes).is_some());
        bytes.extend_from_slice(&junk);
        prop_assert!(
            PartyResponse::from_bytes(&bytes).is_none(),
            "trailing bytes decoded to the frame without them"
        );
    }

    /// `sanitize_filename` decides where a received file is written.
    #[test]
    fn sanitized_filenames_are_always_safe(raw in ".{0,300}") {
        let out = sanitize_filename(&raw);

        prop_assert!(!out.contains('/'), "path separator survived: {out:?}");
        prop_assert!(!out.contains('\\'), "path separator survived: {out:?}");
        prop_assert!(out != ".." && out != ".", "traversal component survived: {out:?}");
        prop_assert!(!out.starts_with("..") || !out.contains(std::path::MAIN_SEPARATOR),
            "traversal survived: {out:?}");

        // Never empty (it would join to the directory itself).
        prop_assert!(!out.is_empty(), "empty filename for input {raw:?}");

        // Never a bare Windows device name.
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

        // No control or bidi characters (the full set the sanitiser strips).
        prop_assert!(
            !out.chars().any(|c| c.is_control()
                || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}')),
            "control or bidi character survived: {out:?}"
        );

        // Windows drops trailing dots/spaces ("evil.exe " -> "evil.exe").
        prop_assert!(!out.ends_with(' ') && !out.ends_with('.'), "trailing dot/space: {out:?}");

        // Bounded, so the receiver's `tmp_<uuid>_` prefix still fits under NAME_MAX.
        prop_assert!(out.len() <= 255, "filename too long: {} bytes", out.len());
    }

    /// Sanitising is idempotent, or the name on disk depends on how often it
    /// was sanitised.
    #[test]
    fn sanitizing_is_idempotent(raw in ".{0,200}") {
        let once = sanitize_filename(&raw);
        let twice = sanitize_filename(&once);
        prop_assert_eq!(once, twice);
    }
}

/// Hand-picked hostile filenames, alongside the random ones.
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

        // Joined to a directory it must stay inside it.
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

/// Invalid UTF-8 in a peer string field is refused, not repaired: lossy
/// decoding expands each bad byte to three while every cap counts wire bytes
/// (a 64 KiB `Text` became 192 KiB; 512 chunks ~72 MiB vs ~24 MiB), and it
/// accepted frames the encoder cannot reproduce.
#[test]
fn invalid_utf8_is_refused_rather_than_repaired() {
    let bad = [0xFFu8; 3];
    let cases: [(&str, Vec<u8>); 5] = [
        // Tag 2 — Text: seq, timestamp, len, bytes.
        (
            "Text",
            [
                &[2u8][..],
                &1u64.to_be_bytes(),
                &0u64.to_be_bytes(),
                &3u32.to_be_bytes(),
                &bad,
            ]
            .concat(),
        ),
        // Tag 11 — TextChunk: id, seq, timestamp, index, total, len, bytes.
        (
            "TextChunk",
            [
                &[11u8][..],
                &[0u8; 16],
                &1u64.to_be_bytes(),
                &0u64.to_be_bytes(),
                &0u32.to_be_bytes(),
                &2u32.to_be_bytes(),
                &3u32.to_be_bytes(),
                &bad,
            ]
            .concat(),
        ),
        // Tag 3 — FileMeta: two byte strings must not collapse onto one name.
        (
            "FileMeta",
            [
                &[3u8][..],
                &1u64.to_be_bytes(),
                &10u64.to_be_bytes(),
                &3u32.to_be_bytes(),
                &bad,
            ]
            .concat(),
        ),
        // Legacy arms; `FILE_META|` used to keep "\u{fffd}\u{fffd}\u{fffd}".
        (
            "legacy FILE_META",
            [&b"FILE_META|1|"[..], &bad, b"|10"].concat(),
        ),
        ("legacy TEXT", [&b"TEXT:1:"[..], &bad].concat()),
    ];
    for (name, frame) in cases {
        assert!(
            ProtocolMessage::from_plain_bytes(&frame).is_none(),
            "invalid UTF-8 accepted in {name}"
        );
    }
}

/// The legacy numeric arms bound the slice before decoding it: lossy decoding
/// of `PING:` + 8 MiB of `0xFF` cost ~24 MiB transiently per frame.
#[test]
fn legacy_numeric_arms_refuse_an_oversized_field() {
    for prefix in [
        &b"VERSION:"[..],
        &b"FILE_END:"[..],
        &b"PING:"[..],
        &b"TYPING_START:"[..],
        &b"TYPING_STOP:"[..],
    ] {
        let mut frame = prefix.to_vec();
        frame.extend(std::iter::repeat_n(b'9', 4096));
        assert!(
            ProtocolMessage::from_plain_bytes(&frame).is_none(),
            "{} accepted a 4 KiB number",
            String::from_utf8_lossy(prefix)
        );

        let mut ok = prefix.to_vec();
        ok.extend_from_slice(b"1");
        assert!(
            ProtocolMessage::from_plain_bytes(&ok).is_some(),
            "{} refused a valid frame",
            String::from_utf8_lossy(prefix)
        );
    }
}

/// A frame claiming an enormous payload must be refused without allocating
/// (a regression shows up as a timeout or OOM kill).
#[test]
fn declared_gigabyte_payloads_do_not_allocate() {
    for tag in 0u8..=20 {
        let mut frame = vec![tag];
        frame.extend_from_slice(&u32::MAX.to_be_bytes());
        frame.extend_from_slice(&u64::MAX.to_be_bytes());
        frame.extend_from_slice(&u64::MAX.to_be_bytes());
        frame.extend_from_slice(&[0u8; 16]);
        let _ = ProtocolMessage::from_plain_bytes(&frame);
    }
}

/// The two truncation bugs the property tests found, pinned with reasoning.
mod truncation_reintroduced_what_sanitising_removed {
    use super::*;

    /// Truncation used to yield `….exe.`, whose `extension()` is `Some("")`, so
    /// the "runs as a program" gate missed what Windows then created as `.exe`.
    #[test]
    fn a_crafted_name_cannot_hide_an_executable_extension() {
        // Sized against MAX_FILENAME_BYTES (150); the >16-char last segment is
        // not an extension, so the whole name gets cut blindly.
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
        assert_eq!(out, out.trim_end_matches(['.', ' ']));
    }

    /// Truncating between two dots around a multi-byte char re-created a `..`.
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

/// The tracked seed corpus must still reach its decoders: undecodable seeds
/// teach libFuzzer nothing, and this catches a seed going stale when a decoder
/// is tightened.
#[test]
fn the_tracked_seed_corpus_still_reaches_its_decoders() {
    use std::path::Path;

    fn seeds(target: &str) -> Vec<(String, Vec<u8>)> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fuzz/seeds")
            .join(target);
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                if e.path().is_file() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    out.push((name, std::fs::read(e.path()).unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    // Party frames are bincode (variant = LE u32): every seed must decode.
    let party = seeds("party_frame");
    assert!(!party.is_empty(), "no party_frame seeds found");
    let stale: Vec<&String> = party
        .iter()
        .filter(|(_, b)| {
            PartyRequest::from_bytes(b).is_none() && PartyResponse::from_bytes(b).is_none()
        })
        .map(|(n, _)| n)
        .collect();
    assert!(
        stale.is_empty(),
        "party_frame seeds that no longer decode: {stale:?}"
    );

    // Some protocol seeds deliberately sit past a cap and must be refused, so
    // assert only that some decode.
    let protocol = seeds("protocol_frame");
    assert!(!protocol.is_empty(), "no protocol_frame seeds found");
    let accepted: Vec<&String> = protocol
        .iter()
        .filter(|(_, b)| ProtocolMessage::from_plain_bytes(b).is_some())
        .map(|(n, _)| n)
        .collect();
    assert!(
        !accepted.is_empty(),
        "no protocol_frame seed decodes at all — the corpus is dead weight"
    );
    for (name, bytes) in &protocol {
        let _ = ProtocolMessage::from_plain_bytes(bytes);
        assert!(
            bytes.len() <= 128 * 1024,
            "seed {name} is larger than any cap"
        );
    }

    let proofs = seeds("identity_proof");
    assert!(!proofs.is_empty(), "no identity_proof seeds found");
    let ok = proofs
        .iter()
        .filter(|(_, b)| {
            messenger_core::util::decode_exact::<messenger_core::core::IdentityProof>(b).is_ok()
        })
        .count();
    assert!(
        ok >= proofs.len() - 1,
        "identity_proof seeds no longer decode ({ok} of {})",
        proofs.len()
    );
}
