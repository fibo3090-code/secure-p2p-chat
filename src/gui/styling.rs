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
pub const ACCENT_PRIMARY: Color32 = Color32::from_rgb(0, 140, 255);
pub const ACCENT_SECONDARY: Color32 = Color32::from_rgb(0, 100, 200);
pub const SUCCESS: Color32 = Color32::from_rgb(46, 204, 113);
pub const WARNING: Color32 = Color32::from_rgb(241, 196, 15);
pub const ERROR: Color32 = Color32::from_rgb(231, 76, 60);

// Spacing constants
pub const SPACING_SMALL: f32 = 5.0;
pub const SPACING_MEDIUM: f32 = 10.0;
pub const SPACING_LARGE: f32 = 15.0;

/// Returns a full set of dark visuals.
fn dark_visuals() -> Visuals {
    let mut visuals = Visuals::dark();
    // Modern Dark Theme: Deep Blue-Grey Backgrounds
    let bg = Color32::from_rgb(15, 23, 42); // Slate 900
    let primary = Color32::from_rgb(30, 41, 59); // Slate 800
    let secondary = Color32::from_rgb(51, 65, 85); // Slate 700
    let text = Color32::from_rgb(241, 245, 249); // Slate 100
    let accent = Color32::from_rgb(56, 189, 248); // Sky 400

    visuals.override_text_color = Some(text);
    visuals.widgets.noninteractive.bg_fill = bg;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, primary);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, text);
    visuals.widgets.noninteractive.rounding = Rounding::same(8.0); // Softer corners

    visuals.widgets.inactive.bg_fill = primary;
    visuals.widgets.inactive.bg_stroke = Stroke::new(0.5, secondary);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, text);
    visuals.widgets.inactive.rounding = Rounding::same(6.0);

    visuals.widgets.hovered.bg_fill = secondary;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, accent.gamma_multiply(0.5));
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.widgets.hovered.rounding = Rounding::same(6.0);

    visuals.widgets.active.bg_fill = accent;
    visuals.widgets.active.bg_stroke = Stroke::NONE;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.widgets.active.rounding = Rounding::same(6.0);

    visuals.selection.bg_fill = accent.gamma_multiply(0.3); // Transparent accent for selection
    visuals.selection.stroke = Stroke::new(1.0, accent);

    visuals.window_rounding = Rounding::same(12.0);
    // visuals.window_shadow = eframe::epaint::Shadow::big_dark(); // Removed as it causes build error
    visuals.window_shadow = eframe::epaint::Shadow {
        extrusion: 32.0,
        color: Color32::from_black_alpha(96),
    };
    visuals
}

/// Returns a full set of light visuals.
fn light_visuals() -> Visuals {
    let mut visuals = Visuals::light();
    visuals.override_text_color = Some(LIGHT_TEXT_PRIMARY);
    // Keep light theme mostly standard but refined
    visuals.widgets.noninteractive.rounding = Rounding::same(6.0);
    visuals.widgets.inactive.rounding = Rounding::same(6.0);
    visuals.menu_rounding = Rounding::same(6.0);
    visuals
}

/// Returns a full set of Midnight visuals (Deep Black/Purple).
fn midnight_visuals() -> Visuals {
    let mut visuals = Visuals::dark();
    let bg = Color32::from_rgb(0, 0, 0); // True Black
    let primary = Color32::from_rgb(10, 10, 15);
    let secondary = Color32::from_rgb(25, 25, 35);
    let text = Color32::from_rgb(220, 220, 255);
    let accent = Color32::from_rgb(124, 58, 237); // Violet 600

    visuals.override_text_color = Some(text);
    visuals.widgets.noninteractive.bg_fill = bg;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, secondary);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, text);
    visuals.widgets.noninteractive.rounding = Rounding::same(8.0);

    visuals.widgets.inactive.bg_fill = primary;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, text);
    visuals.widgets.inactive.rounding = Rounding::same(6.0);

    visuals.widgets.active.bg_fill = accent;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.widgets.active.rounding = Rounding::same(6.0);

    visuals.selection.bg_fill = accent;
    visuals.selection.stroke = Stroke::NONE;

    // visuals.window_shadow = eframe::epaint::Shadow::big_light(); // Glow effect
    visuals.window_shadow = eframe::epaint::Shadow {
        extrusion: 32.0,
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
    let accent = Color32::from_rgb(46, 204, 113); // Green

    visuals.override_text_color = Some(text);
    visuals.widgets.noninteractive.bg_fill = primary;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, secondary);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, text);

    visuals.widgets.inactive.bg_fill = secondary;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, text);

    visuals.widgets.active.bg_fill = accent;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.selection.bg_fill = accent;

    visuals
}


pub fn apply_custom_visuals(theme: &crate::types::Theme) -> Visuals {
    match theme {
        crate::types::Theme::Light => light_visuals(),
        crate::types::Theme::Dark => dark_visuals(),
        crate::types::Theme::Midnight => midnight_visuals(),
        crate::types::Theme::Forest => forest_visuals(),
    }
}

