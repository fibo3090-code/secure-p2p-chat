//! Fingerprint color-grid rendering helper.
//!
//! Lives in the client crate because it produces egui [`Color32`] values for the
//! GUI/TUI fingerprint-verification visuals. The pure-data side (fingerprints,
//! hex) lives in `messenger_core::util`.

use eframe::egui::Color32;

/// Generate a 4x4 color grid from a fingerprint.
pub fn generate_color_grid(fingerprint: &str) -> [[Color32; 4]; 4] {
    let mut grid = [[Color32::BLACK; 4]; 4];
    let bytes = hex::decode(fingerprint).unwrap_or_else(|_| vec![0; 16]);

    let palette = [
        Color32::from_rgb(230, 25, 75),   // Red
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

    for (i, row) in grid.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            let byte_index = i * 4 + j;
            if byte_index < bytes.len() {
                let color_index = bytes[byte_index] as usize % palette.len();
                *cell = palette[color_index];
            }
        }
    }

    grid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_color_grid() {
        let fp = "abcdefgh12345678901234567890ijklmnop";
        let grid = generate_color_grid(fp);
        assert_eq!(grid.len(), 4);
        assert_eq!(grid[0].len(), 4);
    }
}
