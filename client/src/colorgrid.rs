//! Fingerprint colour-grid data.
//!
//! Turns a hex fingerprint into a deterministic 4×4 grid of colours — the
//! "safety grid" shown alongside the short authentication string during TOFU
//! verification, so two peers can compare a shape instead of 64 hex characters.
//!
//! Emits plain `(r, g, b)` triples rather than any toolkit's colour type. It
//! used to return egui `Color32`, which forced the *terminal* UI to depend on a
//! GUI toolkit; the frontends now convert at the edge. The palette and the
//! byte→colour mapping are frozen — changing either would make the same
//! fingerprint render differently on two versions of the app, which is exactly
//! the signal users are asked to compare.

/// An 8-bit RGB colour.
pub type Rgb = (u8, u8, u8);

/// The 16-colour palette, indexed by `fingerprint byte % 16`. Frozen: see the
/// module docs and `grid_is_stable_for_a_known_fingerprint`.
pub const PALETTE: [Rgb; 16] = [
    (230, 25, 75),   // Red
    (60, 180, 75),   // Green
    (255, 225, 25),  // Yellow
    (0, 130, 200),   // Blue
    (245, 130, 48),  // Orange
    (145, 30, 180),  // Purple
    (70, 240, 240),  // Cyan
    (240, 50, 230),  // Magenta
    (210, 245, 60),  // Lime
    (250, 190, 190), // Pink
    (0, 128, 128),   // Teal
    (230, 190, 255), // Lavender
    (170, 110, 40),  // Brown
    (255, 250, 200), // Beige
    (128, 0, 0),     // Maroon
    (128, 128, 0),   // Olive
];

/// Generate the 4×4 colour grid for a fingerprint.
///
/// A fingerprint that is not valid hex (or is shorter than 16 bytes) yields
/// black cells rather than an error: the grid is a comparison aid shown next to
/// the fingerprint itself, so a malformed one should look obviously wrong on
/// both ends instead of failing the verification screen.
pub fn generate_color_grid(fingerprint: &str) -> [[Rgb; 4]; 4] {
    let mut grid = [[(0, 0, 0); 4]; 4];
    let bytes = hex::decode(fingerprint).unwrap_or_else(|_| vec![0; 16]);

    for (i, row) in grid.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            let byte_index = i * 4 + j;
            if byte_index < bytes.len() {
                *cell = PALETTE[bytes[byte_index] as usize % PALETTE.len()];
            }
        }
    }

    grid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_is_four_by_four() {
        let grid = generate_color_grid(&"ab".repeat(32));
        assert_eq!(grid.len(), 4);
        assert_eq!(grid[0].len(), 4);
    }

    /// Known-answer test. The grid is a security signal users compare across
    /// two machines, so the mapping must never drift between versions.
    #[test]
    fn grid_is_stable_for_a_known_fingerprint() {
        // Bytes 0x00..0x0f → palette entries 0..15, in order.
        let fp = "000102030405060708090a0b0c0d0e0f";
        let grid = generate_color_grid(fp);
        for (i, row) in grid.iter().enumerate() {
            for (j, cell) in row.iter().enumerate() {
                assert_eq!(*cell, PALETTE[i * 4 + j], "cell ({i},{j}) drifted");
            }
        }
    }

    #[test]
    fn same_fingerprint_always_yields_the_same_grid() {
        let fp = "a1b2c3d4e5f60718293a4b5c6d7e8f90112233445566778899aabbccddeeff00";
        assert_eq!(generate_color_grid(fp), generate_color_grid(fp));
    }

    #[test]
    fn different_fingerprints_yield_different_grids() {
        let a = generate_color_grid(&"00".repeat(32));
        let b = generate_color_grid(&"11".repeat(32));
        assert_ne!(a, b);
    }

    /// Non-hex input must degrade to a blank grid, not panic — a malformed
    /// fingerprint can arrive from a peer.
    #[test]
    fn invalid_hex_degrades_to_black() {
        let grid = generate_color_grid("not hex at all");
        assert!(grid
            .iter()
            .flatten()
            .all(|c| *c == PALETTE[0] || *c == (0, 0, 0)));
        // Decoding failure substitutes 16 zero bytes → every cell is palette[0].
        assert_eq!(grid[0][0], PALETTE[0]);
    }

    /// A short fingerprint leaves the remaining cells black instead of
    /// indexing out of bounds.
    #[test]
    fn short_fingerprint_leaves_trailing_cells_black() {
        let grid = generate_color_grid("0001");
        assert_eq!(grid[0][0], PALETTE[0]);
        assert_eq!(grid[0][1], PALETTE[1]);
        assert_eq!(grid[3][3], (0, 0, 0));
    }
}
