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
    visuals.override_text_color = Some(DARK_TEXT_PRIMARY);
    visuals.widgets.noninteractive.bg_fill = DARK_PRIMARY_BACKGROUND;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, DARK_SECONDARY_BACKGROUND);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, DARK_TEXT_PRIMARY);
    visuals.widgets.noninteractive.rounding = Rounding::same(4.0);

    visuals.widgets.inactive.bg_fill = DARK_SECONDARY_BACKGROUND;
    visuals.widgets.inactive.bg_stroke = Stroke::NONE;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, DARK_TEXT_PRIMARY);
    visuals.widgets.inactive.rounding = Rounding::same(4.0);

    visuals.widgets.hovered.bg_fill = Color32::from_gray(60);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_gray(80));
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, DARK_TEXT_PRIMARY);
    visuals.widgets.hovered.rounding = Rounding::same(4.0);

    visuals.widgets.active.bg_fill = ACCENT_PRIMARY;
    visuals.widgets.active.bg_stroke = Stroke::NONE;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.widgets.active.rounding = Rounding::same(4.0);

    visuals.selection.bg_fill = ACCENT_PRIMARY;
    visuals.selection.stroke = Stroke::new(1.0, DARK_TEXT_PRIMARY);

    visuals.window_rounding = Rounding::same(6.0);
    visuals.window_shadow = eframe::epaint::Shadow::default();
    visuals
}

/// Returns a full set of light visuals.
fn light_visuals() -> Visuals {
    let mut visuals = Visuals::light();
    visuals.override_text_color = Some(LIGHT_TEXT_PRIMARY);
    visuals.widgets.noninteractive.bg_fill = LIGHT_PRIMARY_BACKGROUND;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LIGHT_SECONDARY_BACKGROUND);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, LIGHT_TEXT_PRIMARY);
    visuals.widgets.noninteractive.rounding = Rounding::same(4.0);

    visuals.widgets.inactive.bg_fill = LIGHT_SECONDARY_BACKGROUND;
    visuals.widgets.inactive.bg_stroke = Stroke::NONE;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, LIGHT_TEXT_PRIMARY);
    visuals.widgets.inactive.rounding = Rounding::same(4.0);

    visuals.widgets.hovered.bg_fill = Color32::from_gray(220);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_gray(200));
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, LIGHT_TEXT_PRIMARY);
    visuals.widgets.hovered.rounding = Rounding::same(4.0);

    visuals.widgets.active.bg_fill = ACCENT_PRIMARY;
    visuals.widgets.active.bg_stroke = Stroke::NONE;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.widgets.active.rounding = Rounding::same(4.0);

    visuals.selection.bg_fill = ACCENT_PRIMARY;
    visuals.selection.stroke = Stroke::new(1.0, LIGHT_TEXT_PRIMARY);

    visuals.window_rounding = Rounding::same(6.0);
    visuals.window_shadow = eframe::epaint::Shadow::default();
    visuals
}


pub fn apply_custom_visuals(theme: &crate::types::Theme) -> Visuals {
    match theme {
        crate::types::Theme::Light => light_visuals(),
        crate::types::Theme::Dark => dark_visuals(),
    }
}

