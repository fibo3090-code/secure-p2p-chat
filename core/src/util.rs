use anyhow::{anyhow, Context, Result};
use rand::RngCore;
use std::net::{SocketAddr, UdpSocket};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Write `bytes` to `path` so that the file is either the old content or the
/// new content, never a truncated mix of the two.
///
/// Writes to a uniquely-named temporary file **in the same directory** (a
/// rename is only atomic within one filesystem), `fsync`s it so the data is on
/// disk before anything points at it, then renames over the destination. On
/// Unix the containing directory is `fsync`ed too, so the rename itself
/// survives a power loss; the file is created 0600 from the start rather than
/// being widened and then narrowed.
///
/// This exists because the in-place `truncate + write` it replaced could leave
/// `identity.json` empty after a crash, full disk, or power loss — and that
/// file is the only key to the user's encrypted history, so a partial write is
/// indistinguishable from destroying the account.
pub fn write_file_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;

    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| Path::new(".").to_path_buf());
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create directory {}", dir.display()))?;

    // Unique name: two processes writing the same file must not share a temp
    // and corrupt each other's write.
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
        // Durability before visibility: without this the rename can land while
        // the data is still only in the page cache.
        file.sync_all()
            .with_context(|| format!("failed to flush {}", tmp.display()))?;
        Ok(())
    })();

    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    // `rename` replaces the destination atomically on both Unix and Windows.
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow::Error::new(e)
            .context(format!("failed to replace {} atomically", path.display())));
    }

    // Make the rename itself durable. Best-effort: some filesystems refuse to
    // open a directory for sync, and a missing directory fsync is a much
    // smaller risk than the partial write this function exists to prevent.
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

    /// The original file must survive a failed write, since it is the only copy
    /// of the user's identity.
    #[test]
    fn a_failed_write_leaves_the_original_intact() {
        let dir = tempfile::tempdir().unwrap();
        // A path whose "parent" is an existing regular file: creating the temp
        // inside it cannot succeed.
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

/// Get current timestamp in milliseconds since Unix epoch
pub fn current_timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Generate random bytes
pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut buf);
    buf
}

/// Convert bytes to hex string
pub fn to_hex(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

/// Sanitize filename to prevent path traversal attacks
pub fn sanitize_filename(filename: &str) -> String {
    let mut sanitized = filename
        .replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_")
        .chars()
        .take(255)
        .collect::<String>();

    // Collapse any path traversal patterns
    while sanitized.contains("..") {
        sanitized = sanitized.replace("..", "_");
    }

    if sanitized.is_empty() {
        "file".to_string()
    } else {
        sanitized
    }
}

/// Parse a user-facing endpoint string into `(host, port)`.
///
/// Supports:
/// - `hostname:1234`
/// - `127.0.0.1:1234`
/// - `[::1]:1234`
/// - `hostname` / `127.0.0.1` / `::1` when `default_port` is provided
pub fn parse_host_port(input: &str, default_port: Option<u16>) -> Result<(String, u16)> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("Host cannot be empty"));
    }

    if let Some(rest) = trimmed.strip_prefix('[') {
        let end = rest
            .find(']')
            .ok_or_else(|| anyhow!("Invalid IPv6 address format; missing closing ']'"))?;
        let host = &rest[..end];
        let suffix = &rest[end + 1..];
        if let Some(port_str) = suffix.strip_prefix(':') {
            let port = port_str
                .parse::<u16>()
                .map_err(|_| anyhow!("Invalid port"))?;
            return Ok((host.to_string(), port));
        }
        if suffix.is_empty() {
            if let Some(port) = default_port {
                return Ok((host.to_string(), port));
            }
            return Err(anyhow!("Missing port"));
        }
        return Err(anyhow!("Invalid IPv6 endpoint suffix"));
    }

    if trimmed.matches(':').count() > 1 {
        if let Some(port) = default_port {
            return Ok((trimmed.to_string(), port));
        }
        return Err(anyhow!(
            "IPv6 addresses must be bracketed when a port is required"
        ));
    }

    if let Some((host, port_str)) = trimmed.rsplit_once(':') {
        if !host.trim().is_empty() {
            let port = port_str
                .parse::<u16>()
                .map_err(|_| anyhow!("Invalid port"))?;
            return Ok((host.trim().to_string(), port));
        }
    }

    if let Some(port) = default_port {
        return Ok((trimmed.to_string(), port));
    }

    Err(anyhow!("Invalid address format; expected host:port"))
}

/// Render a normalized address string, adding brackets around IPv6 literals.
pub fn format_host_port(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{}]:{}", host, port)
    } else {
        format!("{}:{}", host, port)
    }
}

/// Format file size in human-readable format
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

/// Format fingerprint for display (first 8 + last 8 chars)
pub fn format_fingerprint_short(fp: &str) -> String {
    if fp.len() > 16 {
        format!("{}...{}", &fp[..8], &fp[fp.len() - 8..])
    } else {
        fp.to_string()
    }
}

/// A minimal extension → MIME guess for common file types; defaults to
/// `application/octet-stream`. Display metadata only — never used for security
/// decisions.
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

/// Best-effort discovery of the primary local IPv4 address.
/// Uses a UDP "connect" to a public resolver to learn the outbound interface.
pub fn primary_local_ipv4() -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    // Destination is never actually contacted for UDP connect; safe placeholder.
    socket.connect("8.8.8.8:80").ok()?;
    match socket.local_addr().ok()? {
        SocketAddr::V4(v4) => Some(v4.ip().to_string()),
        SocketAddr::V6(_) => None,
    }
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn test_parse_host_port_ipv4_and_hostname() {
        assert_eq!(
            parse_host_port("127.0.0.1:12345", None).unwrap(),
            ("127.0.0.1".to_string(), 12345)
        );
        assert_eq!(
            parse_host_port("example.local", Some(9000)).unwrap(),
            ("example.local".to_string(), 9000)
        );
    }

    #[test]
    fn test_parse_host_port_ipv6() {
        assert_eq!(
            parse_host_port("[::1]:12345", None).unwrap(),
            ("::1".to_string(), 12345)
        );
        assert_eq!(
            parse_host_port("::1", Some(12345)).unwrap(),
            ("::1".to_string(), 12345)
        );
        assert!(parse_host_port("::1", None).is_err());
    }

    #[test]
    fn test_format_host_port_brackets_ipv6() {
        assert_eq!(format_host_port("::1", 12345), "[::1]:12345");
        assert_eq!(format_host_port("127.0.0.1", 12345), "127.0.0.1:12345");
    }
}
