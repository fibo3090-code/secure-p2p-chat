use eframe::egui::{style::Visuals, Color32, Rounding, Stroke};

// Define color palettes for both dark and light themes to ensure a consistent look and feel.

// Dark Theme Colors
pub const DARK_BACKGROUND: Color32 = Color32::from_rgb(24, 25, 26);
pub const DARK_PRIMARY_BACKGROUND: Color32 = Color32::from_rgb(30, 31, 32);
pub const DARK_SECONDARY_BACKGROUND: Color32 = Color32::from_rgb(44, 45, 46);
pub const DARK_TEXT_PRIMARY: Color32 = Color32::from_gray(220);

// Light Theme Colors
pub const LIGHT_BACKGROUND: Color32 = Color32::from_rgb(242, 243, 244);
pub const LIGHT_PRIMARY_BACKGROUND: Color32 = Color32::from_rgb(255, 255, 255);
pub const LIGHT_SECONDARY_BACKGROUND: Color32 = Color32::from_rgb(230, 231, 232);
pub const LIGHT_TEXT_PRIMARY: Color32 = Color32::from_gray(20);

// Shared Colors
pub const SUBTLE_TEXT_COLOR: Color32 = Color32::from_gray(160);
/// Brand accent ("control teal-indigo"): the single flat-tone color shared by
/// the Dark and Light themes across all three UIs (see design/tokens.json).
pub const ACCENT_PRIMARY: Color32 = Color32::from_rgb(0x3e, 0x8d, 0xd2);
pub const ACCENT_SECONDARY: Color32 = Color32::from_rgb(0, 100, 200);
/// Accent for the Midnight/Forest/Rose themes (design/tokens.json `themes.*.accent`).
/// These stay distinct from `ACCENT_PRIMARY` by design — alternate theme
/// personalities, not brand-compliance failures.
pub const MIDNIGHT_ACCENT: Color32 = Color32::from_rgb(0x8b, 0x7b, 0xff);
pub const FOREST_ACCENT: Color32 = Color32::from_rgb(0x34, 0xd3, 0x99);
pub const ROSE_ACCENT: Color32 = Color32::from_rgb(0xfb, 0x6f, 0x92);
pub const SUCCESS: Color32 = Color32::from_rgb(46, 204, 113);
pub const WARNING: Color32 = Color32::from_rgb(241, 196, 15);
pub const ERROR: Color32 = Color32::from_rgb(231, 76, 60);

// Spacing constants
pub const SPACING_SMALL: f32 = 4.0;
pub const SPACING_MEDIUM: f32 = 8.0;
pub const SPACING_LARGE: f32 = 16.0;
pub const SPACING_XLARGE: f32 = 24.0;

// Radius constants
pub const RADIUS_TIGHT: f32 = 4.0;
pub const RADIUS_DEFAULT: f32 = 8.0;
pub const RADIUS_LARGE: f32 = 10.0;

/// Returns a full set of dark visuals.
fn dark_visuals() -> Visuals {
    let mut visuals = Visuals::dark();
    // Modern Dark Theme: Deep Blue-Grey Backgrounds
    let bg = Color32::from_rgb(15, 23, 42); // Slate 900
    let primary = Color32::from_rgb(30, 41, 59); // Slate 800
    let secondary = Color32::from_rgb(51, 65, 85); // Slate 700
    let text = Color32::from_rgb(241, 245, 249); // Slate 100
    let accent = ACCENT_PRIMARY; // Brand teal-indigo (control)

    visuals.override_text_color = Some(text);
    visuals.widgets.noninteractive.bg_fill = bg;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, primary);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, text);
    visuals.widgets.noninteractive.rounding = Rounding::same(RADIUS_DEFAULT);

    visuals.widgets.inactive.bg_fill = primary;
    visuals.widgets.inactive.bg_stroke = Stroke::new(0.5_f32, secondary);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, text);
    visuals.widgets.inactive.rounding = Rounding::same(RADIUS_DEFAULT);

    visuals.widgets.hovered.bg_fill = secondary;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, accent.gamma_multiply(0.5));
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    visuals.widgets.hovered.rounding = Rounding::same(RADIUS_DEFAULT);

    visuals.widgets.active.bg_fill = accent;
    visuals.widgets.active.bg_stroke = Stroke::NONE;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    visuals.widgets.active.rounding = Rounding::same(RADIUS_DEFAULT);

    visuals.selection.bg_fill = accent.gamma_multiply(0.2); // Transparent accent for selection, reduced intensity
    visuals.selection.stroke = Stroke::new(1.0_f32, accent.gamma_multiply(0.5));

    visuals.window_rounding = Rounding::same(RADIUS_LARGE);
    // visuals.window_shadow = eframe::epaint::Shadow::big_dark(); // Removed as it causes build error
    visuals.window_shadow = eframe::epaint::Shadow {
        blur: 32.0,
        spread: 10.0,
        offset: egui::vec2(0.0, 0.0),
        color: Color32::from_black_alpha(96),
    };
    visuals
}

/// Returns a full set of light visuals.
fn light_visuals() -> Visuals {
    let mut visuals = Visuals::light();
    visuals.override_text_color = Some(LIGHT_TEXT_PRIMARY);
    // Keep light theme mostly standard but refined
    visuals.widgets.noninteractive.rounding = Rounding::same(RADIUS_DEFAULT);
    visuals.widgets.inactive.rounding = Rounding::same(RADIUS_DEFAULT);
    visuals.widgets.hovered.rounding = Rounding::same(RADIUS_DEFAULT);
    visuals.widgets.active.rounding = Rounding::same(RADIUS_DEFAULT);
    visuals.selection.bg_fill = ACCENT_PRIMARY.gamma_multiply(0.15);
    visuals.window_rounding = Rounding::same(RADIUS_LARGE);
    visuals.menu_rounding = Rounding::same(RADIUS_DEFAULT);
    visuals
}

/// Returns a full set of Midnight visuals (Deep Black/Purple).
fn midnight_visuals() -> Visuals {
    let mut visuals = Visuals::dark();
    let bg = Color32::from_rgb(0, 0, 0); // True Black
    let primary = Color32::from_rgb(10, 10, 15);
    let secondary = Color32::from_rgb(25, 25, 35);
    let text = Color32::from_rgb(220, 220, 255);
    let accent = MIDNIGHT_ACCENT;

    visuals.override_text_color = Some(text);
    visuals.widgets.noninteractive.bg_fill = bg;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, secondary);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, text);
    visuals.widgets.noninteractive.rounding = Rounding::same(RADIUS_DEFAULT);

    visuals.widgets.inactive.bg_fill = primary;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, text);
    visuals.widgets.inactive.rounding = Rounding::same(RADIUS_DEFAULT);

    visuals.widgets.hovered.rounding = Rounding::same(RADIUS_DEFAULT);

    visuals.widgets.active.bg_fill = accent;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    visuals.widgets.active.rounding = Rounding::same(RADIUS_DEFAULT);

    visuals.selection.bg_fill = accent.gamma_multiply(0.25);
    visuals.selection.stroke = Stroke::NONE;
    visuals.window_rounding = Rounding::same(RADIUS_LARGE);

    // visuals.window_shadow = eframe::epaint::Shadow::big_light(); // Glow effect
    visuals.window_shadow = eframe::epaint::Shadow {
        blur: 32.0,
        spread: 5.0,
        offset: egui::vec2(0.0, 0.0),
        color: Color32::from_rgb(100, 100, 255).gamma_multiply(0.5), // Violet glow
    };
    visuals
}

/// Returns a full set of Forest visuals.
fn forest_visuals() -> Visuals {
    let mut visuals = Visuals::dark();
    let _bg = Color32::from_rgb(20, 30, 20);
    let primary = Color32::from_rgb(30, 45, 30);
    let secondary = Color32::from_rgb(45, 60, 45);
    let text = Color32::from_rgb(220, 240, 220);
    let accent = FOREST_ACCENT;

    visuals.override_text_color = Some(text);
    visuals.widgets.noninteractive.bg_fill = primary;
    visuals.widgets.noninteractive.rounding = Rounding::same(RADIUS_DEFAULT);

    visuals.widgets.inactive.bg_fill = secondary;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, text);
    visuals.widgets.inactive.rounding = Rounding::same(RADIUS_DEFAULT);

    visuals.widgets.hovered.rounding = Rounding::same(RADIUS_DEFAULT);

    visuals.widgets.active.bg_fill = accent;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    visuals.widgets.active.rounding = Rounding::same(RADIUS_DEFAULT);

    visuals.selection.bg_fill = accent.gamma_multiply(0.25);
    visuals.window_rounding = Rounding::same(RADIUS_LARGE);

    visuals
}

/// Returns a full set of Rose visuals.
fn rose_visuals() -> Visuals {
    let mut visuals = Visuals::dark();
    let primary = Color32::from_rgb(30, 16, 22); // matches desktop rose --s1 #1e1016
    let secondary = Color32::from_rgb(39, 21, 30); // matches desktop rose --s2 #27151e
    let text = Color32::from_rgb(246, 232, 239); // matches desktop rose --text #f6e8ef
    let accent = ROSE_ACCENT;

    visuals.override_text_color = Some(text);
    visuals.widgets.noninteractive.bg_fill = primary;
    visuals.widgets.noninteractive.rounding = Rounding::same(RADIUS_DEFAULT);

    visuals.widgets.inactive.bg_fill = secondary;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, text);
    visuals.widgets.inactive.rounding = Rounding::same(RADIUS_DEFAULT);

    visuals.widgets.hovered.rounding = Rounding::same(RADIUS_DEFAULT);

    visuals.widgets.active.bg_fill = accent;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    visuals.widgets.active.rounding = Rounding::same(RADIUS_DEFAULT);

    visuals.selection.bg_fill = accent.gamma_multiply(0.25);
    visuals.window_rounding = Rounding::same(RADIUS_LARGE);

    visuals
}

pub fn apply_custom_visuals(theme: &crate::types::Theme) -> Visuals {
    match theme {
        crate::types::Theme::Light => light_visuals(),
        crate::types::Theme::Dark => dark_visuals(),
        crate::types::Theme::Midnight => midnight_visuals(),
        crate::types::Theme::Forest => forest_visuals(),
        crate::types::Theme::Rose => rose_visuals(),
    }
}

#[cfg(test)]
mod token_drift_tests {
    //! Guards against `design/tokens.json` (the canonical token source, see
    //! docs/05_platform_spec.md "Visual language") drifting from what egui
    //! actually renders. There's no build-time codegen tying the two
    //! together, so this test is the drift check: if someone edits one and
    //! not the other, this fails instead of the mismatch going unnoticed.
    use super::*;
    use std::path::Path;

    fn hex_to_color32(hex: &str) -> Color32 {
        let hex = hex.trim_start_matches('#');
        let r = u8::from_str_radix(&hex[0..2], 16).unwrap();
        let g = u8::from_str_radix(&hex[2..4], 16).unwrap();
        let b = u8::from_str_radix(&hex[4..6], 16).unwrap();
        Color32::from_rgb(r, g, b)
    }

    fn load_tokens() -> serde_json::Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../design/tokens.json");
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {}", path.display(), e));
        serde_json::from_str(&raw).expect("design/tokens.json must be valid JSON")
    }

    #[test]
    fn egui_accents_match_design_tokens() {
        let tokens = load_tokens();
        let theme_accent =
            |name: &str| hex_to_color32(tokens["themes"][name]["accent"].as_str().unwrap());

        assert_eq!(ACCENT_PRIMARY, theme_accent("dark"), "dark accent drifted");
        assert_eq!(
            ACCENT_PRIMARY,
            theme_accent("light"),
            "light accent drifted"
        );
        assert_eq!(
            MIDNIGHT_ACCENT,
            theme_accent("midnight"),
            "midnight accent drifted"
        );
        assert_eq!(
            FOREST_ACCENT,
            theme_accent("forest"),
            "forest accent drifted"
        );
        assert_eq!(ROSE_ACCENT, theme_accent("rose"), "rose accent drifted");
    }

    #[test]
    fn egui_semantic_colors_match_design_tokens() {
        let tokens = load_tokens();
        let semantic = |name: &str| hex_to_color32(tokens["semantic"][name].as_str().unwrap());

        assert_eq!(SUCCESS, semantic("success"), "success color drifted");
        assert_eq!(WARNING, semantic("warning"), "warning color drifted");
        assert_eq!(ERROR, semantic("error"), "error color drifted");
    }
}
