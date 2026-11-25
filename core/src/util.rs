use anyhow::{anyhow, Context, Result};
use rand::RngCore;
use std::net::{SocketAddr, UdpSocket};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Write `bytes` to `path` atomically: a unique temp file in the same directory
/// (0600 from creation), fsync, rename over the destination, then fsync the
/// directory on Unix. `identity.json` is the only key to the history, so a
/// partial write must be impossible.
pub fn write_file_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;

    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| Path::new(".").to_path_buf());
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create directory {}", dir.display()))?;

    // Unique name, so concurrent writers never share a temp file.
    let file_name = path
        .file_name()
        .ok_or_else(|| anyhow!("{} has no file name", path.display()))?
        .to_string_lossy()
        .into_owned();
    let tmp = dir.join(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4()));

    let write_result = (|| -> Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp)
            .with_context(|| format!("failed to create {}", tmp.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to flush {}", tmp.display()))?;
        Ok(())
    })();

    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow::Error::new(e)
            .context(format!("failed to replace {} atomically", path.display())));
    }

    // Best-effort: some filesystems refuse to open a directory for sync.
    #[cfg(unix)]
    {
        if let Ok(d) = std::fs::File::open(&dir) {
            let _ = d.sync_all();
        }
    }

    Ok(())
}

#[cfg(test)]
mod atomic_write_tests {
    use super::write_file_atomic;

    #[test]
    fn writes_new_file_with_exact_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.json");
        write_file_atomic(&path, b"hello").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
    }

    #[test]
    fn replaces_existing_file_and_leaves_no_temp_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.json");
        write_file_atomic(&path, b"old content that is long").unwrap();
        write_file_atomic(&path, b"new").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            entries,
            vec!["f.json".to_string()],
            "temp files must not survive a successful write"
        );
    }

    #[test]
    fn creates_missing_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("deeper").join("f.json");
        write_file_atomic(&path, b"x").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"x");
    }

    /// The original must survive a failed write (parent is a regular file).
    #[test]
    fn a_failed_write_leaves_the_original_intact() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("not-a-dir");
        std::fs::write(&blocker, b"original").unwrap();
        let doomed = blocker.join("child.json");

        assert!(write_file_atomic(&doomed, b"never lands").is_err());
        assert_eq!(
            std::fs::read(&blocker).unwrap(),
            b"original",
            "an unrelated existing file must not be clobbered"
        );
    }

    #[cfg(unix)]
    #[test]
    fn file_is_owner_only_from_creation() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.json");
        write_file_atomic(&path, b"secret").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "identity files must not be world-readable"
        );
    }
}

/// Decode a bincode frame, refusing trailing bytes: plain `bincode::deserialize`
/// decodes `frame || junk` as `frame`, giving two byte strings one meaning.
/// Same options as `bincode::serialize` (fixint, little-endian) otherwise.
pub fn decode_exact<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    use bincode::Options;
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .reject_trailing_bytes()
        .deserialize(bytes)
        .map_err(|e| anyhow!("malformed frame: {e}"))
}

pub fn current_timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut buf);
    buf
}

pub fn to_hex(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

/// Longest filename [`sanitize_filename`] emits, in **bytes** (the OS limit is
/// 255 bytes); leaves room for the receiver's `tmp_<uuid>_` prefix and `_N`
/// collision suffixes.
pub const MAX_FILENAME_BYTES: usize = 150;

/// Windows device names, reserved in any directory and with any extension.
#[rustfmt::skip]
const WINDOWS_RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "conin$", "conout$",
    "com0", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9",
    "com\u{00B9}", "com\u{00B2}", "com\u{00B3}",
    "lpt0", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    "lpt\u{00B9}", "lpt\u{00B2}", "lpt\u{00B3}",
];

/// C0/C1 controls and bidi overrides, which must never survive into a filename
/// (U+202E renders `photo_gnp.exe` as `photo_exe.png`).
fn is_forbidden_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
                | '\u{200E}'
                | '\u{200F}'
                | '\u{061C}' // Arabic letter mark, a bidi mark like the two above
                | '\u{2028}' // line separator: not `is_control`, still a line break
                | '\u{2029}' // paragraph separator
        )
}

/// True when `name` contains an invisible or text-direction-changing character
/// (control, bidi, zero-width, joiners): `ad\u{200B}min` renders as `admin`.
/// Names other people read (usernames, channel names) refuse them.
pub fn has_hidden_characters(name: &str) -> bool {
    name.chars().any(|c| {
        is_forbidden_char(c)
            || matches!(
                c,
                '\u{00AD}'                      // soft hyphen
                    | '\u{180E}'                // Mongolian vowel separator
                    | '\u{200B}'..='\u{200D}'  // zero-width space / (non-)joiner
                    | '\u{2060}'..='\u{2064}'  // word joiner, invisible operators
                    | '\u{FEFF}'               // byte-order mark / zero-width no-break space
                    | '\u{034F}'               // combining grapheme joiner
                    | '\u{115F}' | '\u{1160}' | '\u{3164}' | '\u{FFA0}' // Hangul fillers (blank)
                    | '\u{FE00}'..='\u{FE0F}' // variation selectors
                    | '\u{E0000}'..='\u{E007F}' // tag characters (CRYPTO-12)
            )
    })
}

/// Tag a file received from a peer the way a browser tags a download, so the
/// OS applies its own protections when it is opened (TAURI-06): on Windows a
/// `Zone.Identifier` stream with `ZoneId=3` (Internet) is what makes
/// SmartScreen check an executable and Office open a document in Protected
/// View. Best effort — a filesystem without alternate data streams (FAT, some
/// network shares) simply does not get the tag.
///
/// macOS needs no code: the desktop bundle sets `LSFileQuarantineEnabled`, so
/// the system quarantines every file the app writes. Linux has no equivalent.
pub fn mark_as_downloaded(path: &Path) {
    #[cfg(windows)]
    {
        let mut stream = path.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        if let Err(e) = std::fs::write(&stream, b"[ZoneTransfer]\r\nZoneId=3\r\n") {
            tracing::debug!(path = %path.display(), "could not mark a received file as downloaded: {e}");
        }
    }
    #[cfg(not(windows))]
    let _ = path;
}

/// Reduce a peer-supplied name to something safe to create and honest to
/// display: no separators, control/bidi characters, `..`, Windows device names
/// or trailing dots/spaces, within a byte budget that keeps the extension.
pub fn sanitize_filename(filename: &str) -> String {
    // Map path separators, shell-hostile, control and bidi characters to `_`.
    let mapped: String = filename
        .replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_")
        .chars()
        .map(|c| if is_forbidden_char(c) { '_' } else { c })
        .collect();

    // Truncate *then* tidy: tidying first let truncation re-create a trailing
    // dot (`AAA….exe.`, which Windows turns into a `.exe` the open gate missed)
    // or a `..` from two dots around a cut multi-byte char.
    let mut sanitized = tidy_edges(&truncate_filename_bytes(&mapped, MAX_FILENAME_BYTES));

    // Reserved names are reserved only as the stem, so prefixing defuses them.
    // Win32 ignores trailing spaces and dots in the stem, so `CON .txt` is the
    // console too (CRYPTO-13).
    let stem = sanitized
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches([' ', '.'])
        .to_ascii_lowercase();
    if WINDOWS_RESERVED.contains(&stem.as_str()) {
        sanitized = format!("_{sanitized}");
        // The prefix can push the name over budget; re-truncate and re-tidy.
        if sanitized.len() > MAX_FILENAME_BYTES {
            sanitized = tidy_edges(&truncate_filename_bytes(&sanitized, MAX_FILENAME_BYTES));
        }
    }

    if sanitized.is_empty() {
        "file".to_string()
    } else {
        sanitized
    }
}

/// Collapse `..` and strip leading/trailing dots and spaces (which the OS would
/// silently discard), so the name on disk is the name that was checked. Only
/// shrinks its input, so it can run after truncation, and is idempotent
/// (asserted by `sanitizing_is_idempotent` in `core/tests/fuzz_parsers.rs`).
fn tidy_edges(name: &str) -> String {
    let mut out = name.to_string();
    while out.contains("..") {
        out = out.replace("..", "_");
    }
    out.trim_matches(|c: char| c == '.' || c == ' ').to_string()
}

/// Cut `name` to at most `budget` bytes on a char boundary, keeping a short
/// extension so the file still opens.
fn truncate_filename_bytes(name: &str, budget: usize) -> String {
    if name.len() <= budget {
        return name.to_string();
    }

    // Only a short trailing segment counts as an extension.
    let ext = name
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .filter(|ext| !ext.is_empty() && ext.len() <= 16)
        .unwrap_or("");
    let stem = if ext.is_empty() {
        name
    } else {
        &name[..name.len() - ext.len() - 1]
    };

    let stem_budget = budget.saturating_sub(if ext.is_empty() { 0 } else { ext.len() + 1 });
    let mut end = stem_budget.min(stem.len());
    while end > 0 && !stem.is_char_boundary(end) {
        end -= 1;
    }
    let stem = &stem[..end];

    if ext.is_empty() {
        stem.to_string()
    } else {
        format!("{stem}.{ext}")
    }
}

/// Parse `host:port`, `[v6]:port`, or a bare host / v4 / v6 / `[v6]` when
/// `default_port` is given. Every shape is held to the same rules: non-empty
/// host without spaces or control chars, port plain digits in `1..=65535`, and
/// no URLs.
pub fn parse_host_port(input: &str, default_port: Option<u16>) -> Result<(String, u16)> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("Host cannot be empty"));
    }
    if trimmed.contains("://") {
        return Err(anyhow!("Enter an address like host:port, not a URL"));
    }

    let (host, port) = if let Some(rest) = trimmed.strip_prefix('[') {
        let (host, suffix) = rest
            .split_once(']')
            .ok_or_else(|| anyhow!("Invalid IPv6 address format; missing closing ']'"))?;
        let port = match suffix {
            "" => None,
            _ => Some(
                suffix
                    .strip_prefix(':')
                    .ok_or_else(|| anyhow!("Invalid IPv6 endpoint suffix"))?,
            ),
        };
        (host, port)
    } else if trimmed.matches(':').count() > 1 {
        // Several colons, no brackets: only a bare IPv6 address, which has no port.
        if !is_ipv6_literal(trimmed) {
            return Err(anyhow!(
                "Not a valid address; write an IPv6 address with a port as [addr]:port"
            ));
        }
        (trimmed, None)
    } else {
        match trimmed.rsplit_once(':') {
            Some((host, port)) => (host.trim(), Some(port)),
            None => (trimmed, None),
        }
    };

    if host.is_empty() {
        return Err(anyhow!("Host cannot be empty"));
    }
    if host.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(anyhow!("Host cannot contain spaces"));
    }
    let port = match port {
        Some(port) => parse_port(port)?,
        None => {
            default_port.ok_or_else(|| anyhow!("Invalid address format; expected host:port"))?
        }
    };
    Ok((host.to_string(), port))
}

/// Plain digits, no sign, and not 0 (cannot be dialled).
fn parse_port(text: &str) -> Result<u16> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(anyhow!("Invalid port"));
    }
    match text.parse::<u16>() {
        Ok(0) => Err(anyhow!("Port 0 cannot be dialled")),
        Ok(port) => Ok(port),
        Err(_) => Err(anyhow!("Invalid port (must be 1-65535)")),
    }
}

/// IPv6 address, optionally with a `%zone` suffix.
fn is_ipv6_literal(text: &str) -> bool {
    let address = text.split_once('%').map_or(text, |(address, _)| address);
    address.parse::<std::net::Ipv6Addr>().is_ok()
}

/// `host:port`, bracketing IPv6 literals.
pub fn format_host_port(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{}]:{}", host, port)
    } else {
        format!("{}:{}", host, port)
    }
}

/// Human-readable file size.
pub fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;

    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }

    format!("{:.2} {}", size, UNITS[unit_idx])
}

/// Canonical fingerprint spelling (trimmed lowercase hex, as `fingerprint_pubkey`
/// emits). Normalise human-supplied fingerprints at the boundary: trust checks
/// compare exactly, so an uppercase one stored verbatim blocked/verified nothing.
pub fn canonical_fingerprint(fp: &str) -> String {
    fp.trim().to_ascii_lowercase()
}

/// Format fingerprint for display (first 8 + last 8 chars)
pub fn format_fingerprint_short(fp: &str) -> String {
    if fp.len() > 16 {
        format!("{}...{}", &fp[..8], &fp[fp.len() - 8..])
    } else {
        fp.to_string()
    }
}

/// Extension -> MIME guess for display only; never used for security decisions.
pub fn guess_mime(path: &std::path::Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" | "log" | "md" => "text/plain",
        "json" => "application/json",
        "zip" => "application/zip",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        _ => "application/octet-stream",
    }
}

/// Best-effort primary local IPv4, learned via a UDP "connect" (sends nothing).
pub fn primary_local_ipv4() -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    match socket.local_addr().ok()? {
        SocketAddr::V4(v4) => Some(v4.ip().to_string()),
        SocketAddr::V6(_) => None,
    }
}

#[cfg(test)]
mod tests {
    /// CRYPTO-12: characters that render as nothing, or change how the text
    /// around them is drawn, but slipped past the name checks — a username of
    /// Hangul fillers looks blank, `admin` plus a tag character looks like
    /// `admin`.
    #[test]
    fn names_hiding_these_characters_are_refused() {
        for c in [
            '\u{061C}', // Arabic letter mark (bidi)
            '\u{2028}',
            '\u{2029}', // line / paragraph separator
            '\u{115F}',
            '\u{1160}',
            '\u{3164}',
            '\u{FFA0}', // Hangul fillers
            '\u{034F}', // combining grapheme joiner
            '\u{FE00}',
            '\u{FE0F}', // variation selectors
            '\u{E0001}',
            '\u{E0061}',
            '\u{E007F}', // tag characters
        ] {
            let name = format!("ad{c}min");
            assert!(
                super::has_hidden_characters(&name),
                "U+{:04X} passed",
                c as u32
            );
        }
        assert!(!super::has_hidden_characters("admin"));
        assert!(!super::has_hidden_characters("Zoë 李"));
        // A filename keeps neither a line break nor a bidi mark: either one
        // rearranges the file card that draws it.
        for c in ['\u{2028}', '\u{2029}', '\u{061C}'] {
            let cleaned = super::sanitize_filename(&format!("report{c}.pdf"));
            assert!(
                !cleaned.contains(c),
                "U+{:04X} kept in {cleaned:?}",
                c as u32
            );
        }
    }

    /// CRYPTO-13: Windows reserves more device names than the list had, and
    /// ignores trailing spaces and dots in the stem: `CON .txt` is `CON`.
    #[test]
    fn every_reserved_windows_device_name_is_defused() {
        for name in [
            "COM0.txt",
            "LPT0.log",
            "COM\u{00B9}.txt",
            "COM\u{00B2}",
            "COM\u{00B3}.dat",
            "LPT\u{00B9}.txt",
            "LPT\u{00B2}",
            "lpt\u{00B3}.x",
            "CONIN$.txt",
            "conout$",
            "CON .txt",
            "nul. .txt",
            "Aux  .md",
        ] {
            let cleaned = super::sanitize_filename(name);
            assert!(cleaned.starts_with('_'), "{name:?} became {cleaned:?}");
        }
        for name in ["console.txt", "com10.txt", "lpt.txt", "nullify.md"] {
            assert_eq!(super::sanitize_filename(name), name);
        }
    }

    use super::*;

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("normal.txt"), "normal.txt");
        let trav = sanitize_filename("../../../etc/passwd");
        assert!(!trav.contains(".."));
        assert!(!trav.contains('/'));
        assert!(!trav.contains('\\'));
        assert!(!trav.is_empty());
        assert_eq!(
            sanitize_filename("file:with*bad?chars"),
            "file_with_bad_chars"
        );
    }

    #[test]
    fn sanitize_filename_budgets_bytes_not_chars() {
        let ascii = sanitize_filename(&format!("{}.pdf", "a".repeat(400)));
        assert!(
            ascii.len() <= MAX_FILENAME_BYTES,
            "{} bytes is over budget",
            ascii.len()
        );
        assert!(ascii.ends_with(".pdf"), "the extension must survive");

        // Multi-byte: the cut must land on a char boundary.
        let wide = sanitize_filename(&format!("{}.txt", "é".repeat(400)));
        assert!(wide.len() <= MAX_FILENAME_BYTES);
        assert!(wide.ends_with(".txt"));

        // Leaves room for the receiver's `tmp_<uuid>_` prefix under NAME_MAX.
        assert!(MAX_FILENAME_BYTES + "tmp_".len() + 36 + 1 < 255);
    }

    #[test]
    fn sanitize_filename_defuses_windows_device_names() {
        for reserved in ["CON", "nul", "Com1.txt", "LPT9.log", "aux"] {
            let safe = sanitize_filename(reserved);
            let stem = safe.split('.').next().unwrap().to_ascii_lowercase();
            assert!(
                !WINDOWS_RESERVED.contains(&stem.as_str()),
                "{reserved} sanitized to {safe}, still a device name"
            );
        }
        // An ordinary name that merely starts with those letters is untouched.
        assert_eq!(sanitize_filename("console.log"), "console.log");
    }

    /// U+202E makes `photo_gnp.exe` render as `photo_exe.png`.
    #[test]
    fn sanitize_filename_strips_control_and_bidi_characters() {
        let spoofed = sanitize_filename("photo_\u{202E}gnp.exe");
        assert!(!spoofed.contains('\u{202E}'));
        assert!(spoofed.ends_with(".exe"));

        let newline = sanitize_filename("invoice\r\n.pdf");
        assert!(!newline.contains('\r') && !newline.contains('\n'));

        for c in ['\u{202A}', '\u{2066}', '\u{200F}', '\u{0007}'] {
            let out = sanitize_filename(&format!("a{c}b.txt"));
            assert!(!out.contains(c), "{c:?} survived sanitization");
        }
    }

    /// Windows silently drops trailing dots and spaces (`evil.exe .` -> `evil.exe`).
    #[test]
    fn sanitize_filename_strips_trailing_dots_and_spaces() {
        assert_eq!(sanitize_filename("report.pdf . "), "report.pdf");
        // Nothing left to keep must still yield a creatable, non-empty name.
        for nothing in ["...", "   ", ". . .", ""] {
            let out = sanitize_filename(nothing);
            assert!(!out.is_empty(), "{nothing:?} sanitized to an empty name");
            assert!(!out.starts_with('.') && !out.ends_with('.'), "{out:?}");
        }
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(0), "0.00 B");
        assert_eq!(format_size(1023), "1023.00 B");
        assert_eq!(format_size(1024), "1.00 KB");
        assert_eq!(format_size(1024 * 1024), "1.00 MB");
        assert_eq!(format_size(1024 * 1024 * 1024), "1.00 GB");
    }

    #[test]
    fn test_format_fingerprint_short() {
        let long_fp = "abcdefgh12345678901234567890ijklmnop";
        let short = format_fingerprint_short(long_fp);
        assert!(short.contains("..."));
        assert!(short.starts_with("abcdefgh"));
    }

    /// What users actually paste into "address" fields, against the one
    /// parser every connect path goes through (TUI, contacts, invites, the
    /// relay and the community client).
    #[test]
    fn parse_host_port_accepts_what_can_be_dialled() {
        for (input, default, host, port) in [
            ("127.0.0.1:12345", None, "127.0.0.1", 12345),
            ("example.com:80", None, "example.com", 80),
            ("  example.com:80 \n", None, "example.com", 80),
            ("127.0.0.1:1", None, "127.0.0.1", 1),
            ("[::1]:65535", None, "::1", 65535),
            ("[fe80::1%eth0]:80", None, "fe80::1%eth0", 80),
            ("::1", Some(12345), "::1", 12345),
            ("2001:db8::7", Some(9), "2001:db8::7", 9),
            ("example.local", Some(9000), "example.local", 9000),
            ("[::1]", Some(7), "::1", 7),
        ] {
            assert_eq!(
                parse_host_port(input, default).unwrap_or_else(|e| panic!("{input:?}: {e}")),
                (host.to_string(), port),
                "{input:?}"
            );
        }
    }

    #[test]
    fn parse_host_port_refuses_what_cannot_be_dialled() {
        let refused = |input: &str, default: Option<u16>| {
            if let Ok(parsed) = parse_host_port(input, default) {
                panic!("{input:?} (default {default:?}) was accepted as {parsed:?}");
            }
        };
        for default in [None, Some(12345)] {
            // Nothing to dial.
            for input in ["", "   ", ":80", "[]:80", "[]"] {
                refused(input, default);
            }
            // Pasted URLs and garbage are not hosts.
            for input in [
                "http://example.com:8080",
                "chat-p2p://invite/abc",
                "example.com:80:90",
                "ex ample.com:80",
                "[::1]x",
                "[::1:80",
            ] {
                refused(input, default);
            }
        }
        // Ports that cannot be dialled.
        for input in [
            "example.com:0",
            "example.com:65536",
            "example.com:-1",
            "example.com:+80",
            "example.com:",
            "[::1]:0",
        ] {
            refused(input, None);
        }
        // A bare IPv6 address with no default port is ambiguous, not "::" + port 1.
        refused("::1", None);
    }

    #[test]
    fn test_format_host_port_brackets_ipv6() {
        assert_eq!(format_host_port("::1", 12345), "[::1]:12345");
        assert_eq!(format_host_port("127.0.0.1", 12345), "127.0.0.1:12345");
    }
}

/// Resolves when the process is asked to stop: Ctrl+C (SIGINT) anywhere, and
/// SIGTERM on Unix (what `docker stop`, systemd and `kill` send). The server
/// and the relay select on it, so they log and return instead of dying
/// mid-write — and so an instrumented test binary writes its coverage on the
/// way out, which a killed one never did.
pub async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
