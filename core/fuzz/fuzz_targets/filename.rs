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
});
