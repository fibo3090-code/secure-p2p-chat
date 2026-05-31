use anyhow::{anyhow, Result};
use rand::RngCore;
use std::net::{SocketAddr, UdpSocket};
use std::time::{SystemTime, UNIX_EPOCH};

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
