use eframe::egui::{style::Visuals, Color32, Rounding, Stroke};

// Modern color palette inspired by popular messaging apps

pub const BACKGROUND: Color32 = Color32::from_rgb(24, 25, 26); // Very dark grey
pub const PRIMARY_BACKGROUND: Color32 = Color32::from_rgb(30, 31, 32); // Dark grey
pub const SECONDARY_BACKGROUND: Color32 = Color32::from_rgb(44, 45, 46); // Grey

pub const TEXT_PRIMARY: Color32 = Color32::from_gray(220); // Off-white
pub const SUBTLE_TEXT_COLOR: Color32 = Color32::from_gray(160); // Grey

pub const ACCENT_PRIMARY: Color32 = Color32::from_rgb(0, 140, 255); // Bright blue
pub const ACCENT_SECONDARY: Color32 = Color32::from_rgb(0, 100, 200); // Darker blue

pub const SUCCESS: Color32 = Color32::from_rgb(46, 204, 113); // Green
pub const WARNING: Color32 = Color32::from_rgb(241, 196, 15); // Yellow
pub const ERROR: Color32 = Color32::from_rgb(231, 76, 60); // Red

pub fn apply_custom_visuals() -> Visuals {
    let mut visuals = Visuals::dark();

    visuals.override_text_color = Some(TEXT_PRIMARY);

    visuals.widgets.noninteractive.bg_fill = PRIMARY_BACKGROUND;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, SECONDARY_BACKGROUND);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    visuals.widgets.noninteractive.rounding = Rounding::same(4.0);

    visuals.widgets.inactive.bg_fill = SECONDARY_BACKGROUND;
    visuals.widgets.inactive.bg_stroke = Stroke::NONE;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    visuals.widgets.inactive.rounding = Rounding::same(4.0);

    visuals.widgets.hovered.bg_fill = Color32::from_gray(60);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_gray(80));
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    visuals.widgets.hovered.rounding = Rounding::same(4.0);

    visuals.widgets.active.bg_fill = ACCENT_PRIMARY;
    visuals.widgets.active.bg_stroke = Stroke::NONE;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.widgets.active.rounding = Rounding::same(4.0);

    visuals.selection.bg_fill = ACCENT_PRIMARY;
    visuals.selection.stroke = Stroke::new(1.0, TEXT_PRIMARY);

    visuals.window_rounding = Rounding::same(6.0);
    visuals.window_shadow = eframe::epaint::Shadow::NONE;

    visuals
}
