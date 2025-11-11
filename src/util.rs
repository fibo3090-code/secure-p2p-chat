use rand::RngCore;
use std::time::{SystemTime, UNIX_EPOCH};
use eframe::egui::Color32;

/// Get current timestamp in milliseconds since Unix epoch
pub fn current_timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
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
    filename
        .replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_")
        .chars()
        .take(255)
        .collect()
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

/// Generate a 4x4 color grid from a fingerprint
pub fn generate_color_grid(fingerprint: &str) -> [[Color32; 4]; 4] {
    let mut grid = [[Color32::BLACK; 4]; 4];
    let bytes = hex::decode(fingerprint).unwrap_or_else(|_| vec![0; 16]);

    let palette = [
        Color32::from_rgb(230, 25, 75),    // Red
        Color32::from_rgb(60, 180, 75),   // Green
        Color32::from_rgb(255, 225, 25),  // Yellow
        Color32::from_rgb(0, 130, 200),   // Blue
        Color32::from_rgb(245, 130, 48),  // Orange
        Color32::from_rgb(145, 30, 180),  // Purple
        Color32::from_rgb(70, 240, 240),  // Cyan
        Color32::from_rgb(240, 50, 230),  // Magenta
        Color32::from_rgb(210, 245, 60),  // Lime
        Color32::from_rgb(250, 190, 190), // Pink
        Color32::from_rgb(0, 128, 128),   // Teal
        Color32::from_rgb(230, 190, 255), // Lavender
        Color32::from_rgb(170, 110, 40),  // Brown
        Color32::from_rgb(255, 250, 200), // Beige
        Color32::from_rgb(128, 0, 0),     // Maroon
        Color32::from_rgb(128, 128, 0),   // Olive
    ];

    for i in 0..4 {
        for j in 0..4 {
            let byte_index = i * 4 + j;
            if byte_index < bytes.len() {
                let color_index = bytes[byte_index] as usize % palette.len();
                grid[i][j] = palette[color_index];
            }
        }
    }

    grid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("normal.txt"), "normal.txt");
        assert_eq!(
            sanitize_filename("../../../etc/passwd"),
            ".._.._.._etc_passwd"
        );
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
    fn test_generate_color_grid() {
        let fp = "abcdefgh12345678901234567890ijklmnop";
        let grid = generate_color_grid(fp);
        assert_eq!(grid.len(), 4);
        assert_eq!(grid[0].len(), 4);
    }
}
