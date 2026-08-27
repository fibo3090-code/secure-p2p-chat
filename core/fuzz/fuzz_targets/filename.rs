//! Coverage-guided fuzzing of `sanitize_filename`.
//!
//! This one has already earned its place. The property suite found that the
//! function was not idempotent, and that single odd property was hiding a
//! security bug: a peer-chosen filename could be crafted so the executable-file
//! warning saw no extension while Windows created the file as an executable
//! (GHSA-6q3g-734c-22jm). The function decides what a peer's chosen name becomes
//! on disk, so its output is a security boundary.
//!
//! The invariants asserted here are the ones that make the name safe to join to
//! a directory and to inspect for a dangerous extension.
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

    // The name that is checked must be the name that reaches disk. Windows
    // silently drops trailing dots and spaces, which is exactly how the
    // executable-warning bypass worked.
    assert!(!out.ends_with('.'), "trailing dot survived: {out:?}");
    assert!(!out.ends_with(' '), "trailing space survived: {out:?}");

    // Bounded, so the receiver's `tmp_<uuid>_` prefix still fits under NAME_MAX.
    assert!(out.len() <= 255, "name too long: {} bytes", out.len());

    // Idempotent. This is the property that found the bug: if sanitising twice
    // differs from sanitising once, some invariant is being re-broken after it
    // was established, and the name on disk depends on how many times the
    // function happened to be called.
    assert_eq!(out, sanitize_filename(&out), "not a fixed point: {raw:?}");

    // No control characters, and no bidi overrides. U+202E (RIGHT-TO-LEFT
    // OVERRIDE) is the one that stings: `photo` + U+202E + `gnp.exe` *renders*
    // as `photo_exe.png`, so the file the user clicks is not the file they
    // think they are clicking. That spoof is the justification in this target's
    // own docstring, and until now the target did not check for it — only the
    // shallower property suite did.
    assert!(
        !out.chars().any(|c| c.is_control()),
        "control character survived: {out:?}"
    );
    for bidi in [
        '\u{202A}', '\u{202B}', '\u{202C}', '\u{202D}', '\u{202E}', '\u{2066}', '\u{2067}',
        '\u{2068}', '\u{2069}', '\u{200F}', '\u{200E}',
    ] {
        assert!(
            !out.contains(bidi),
            "bidi override U+{:04X} survived: {out:?}",
            bidi as u32
        );
    }

    // A Windows reserved device name must not be the stem. `CON`, `NUL`,
    // `COM1`… are reserved with or without an extension, and opening one is not
    // a file operation at all.
    const RESERVED: [&str; 22] = [
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    let stem = out.split('.').next().unwrap_or("").to_ascii_lowercase();
    assert!(
        !RESERVED.contains(&stem.as_str()),
        "reserved device name survived as the stem: {out:?}"
    );
});
