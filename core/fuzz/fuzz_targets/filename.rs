//! Coverage-guided fuzzing of `sanitize_filename`, whose output (what a peer's
//! chosen name becomes on disk) is a security boundary: non-idempotence once hid
//! an executable-warning bypass (GHSA-6q3g-734c-22jm). The invariants are the ones
//! that make the name safe to join to a directory and to check for a dangerous
//! extension.
#![no_main]

use libfuzzer_sys::fuzz_target;
use messenger_core::util::sanitize_filename;

fuzz_target!(|data: &[u8]| {
    let Ok(raw) = std::str::from_utf8(data) else {
        return;
    };
    let out = sanitize_filename(raw);

    // Never escapes the directory it is joined to.
    assert!(!out.contains('/'), "path separator survived: {out:?}");
    assert!(!out.contains('\\'), "path separator survived: {out:?}");
    assert!(!out.contains(".."), "traversal component survived: {out:?}");
    assert!(out != "." && out != "..", "traversal survived: {out:?}");

    // Never empty: an empty name joins to the directory itself.
    assert!(!out.is_empty(), "empty name for {raw:?}");

    // The name checked must be the name on disk: Windows silently drops
    // trailing dots and spaces (how the bypass worked).
    assert!(!out.ends_with('.'), "trailing dot survived: {out:?}");
    assert!(!out.ends_with(' '), "trailing space survived: {out:?}");

    // Bounded, so the receiver's `tmp_<uuid>_` prefix still fits under NAME_MAX.
    assert!(out.len() <= 255, "name too long: {} bytes", out.len());

    // Idempotent (the property that found the bug): otherwise an invariant is
    // re-broken after being established.
    assert_eq!(out, sanitize_filename(&out), "not a fixed point: {raw:?}");

    // No control characters or bidi overrides: U+202E renders `photo_gnp.exe` as
    // `photo_exe.png`, defeating the warning by display alone.
    assert!(
        !out.chars().any(|c| c.is_control()),
        "control character survived: {out:?}"
    );
    assert!(
        !out.chars().any(|c| matches!(
            c,
            '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}'
        )),
        "bidi override survived: {out:?}"
    );

    // Not a Windows device name (`CON`, `NUL`, `COM1`…: devices at every path).
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = out.split('.').next().unwrap_or(&out).to_ascii_uppercase();
    assert!(
        !RESERVED.contains(&stem.as_str()),
        "Windows device name survived: {out:?}"
    );
});
