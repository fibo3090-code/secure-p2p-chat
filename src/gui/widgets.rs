use eframe::egui;
use egui::{Color32, Rect, Response, Sense, Ui, Vec2, Widget};
use chrono::Local;
use crate::types::Message;
use crate::types::Chat;

pub struct ColorGrid {
    grid: [[Color32; 4]; 4],
}

impl ColorGrid {
    pub fn new(grid: [[Color32; 4]; 4]) -> Self {
        Self { grid }
    }
}

impl Widget for ColorGrid {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, response) = ui.allocate_exact_size(Vec2::new(100.0, 100.0), Sense::hover());
        if ui.is_rect_visible(rect) {
            let painter = ui.painter_at(rect);
            let cell_size = rect.width() / 4.0;
            for i in 0..4 {
                for j in 0..4 {
                    let cell_rect = Rect::from_min_size(
                        rect.min + Vec2::new(j as f32 * cell_size, i as f32 * cell_size),
                        Vec2::new(cell_size, cell_size),
                    );
                    painter.rect_filled(cell_rect, 0.0, self.grid[i][j]);
                }
            }
        }
        response
    }
}

/// Utility: derive a stable color from a fingerprint string
pub fn fingerprint_to_color(fingerprint: &str) -> egui::Color32 {
    let hash = fingerprint
        .bytes()
        .take(3)
        .fold(0u32, |acc, b| acc.wrapping_mul(256).wrapping_add(b as u32));
    let r = ((hash >> 16) & 0xFF) as u8;
    let g = ((hash >> 8) & 0xFF) as u8;
    let b = (hash & 0xFF) as u8;
    let r = r.max(80);
    let g = g.max(80);
    let b = b.max(80);
    Color32::from_rgb(r, g, b)
}

/// Get initials (1-2 letters) for a display name
pub fn get_initials(name: &str) -> String {
    name.split_whitespace()
        .take(2)
        .filter_map(|word| word.chars().next())
        .collect::<String>()
        .to_uppercase()
}

/// Format a timestamp relative (today/time, yesterday, weekday, or date)
pub fn format_timestamp_relative(dt: &chrono::DateTime<chrono::Utc>) -> String {
    let local: chrono::DateTime<Local> = dt.with_timezone(&Local);
    let now = Local::now();

    let days_diff = (now.date_naive() - local.date_naive()).num_days();

    match days_diff {
        0 => local.format("%H:%M").to_string(),
        1 => format!("Yesterday {}", local.format("%H:%M")),
        2..=6 => local.format("%A %H:%M").to_string(),
        _ => local.format("%Y-%m-%d %H:%M").to_string(),
    }
}

/// Render a single chat list item and return the response for click/context menu
pub fn chat_list_item(ui: &mut Ui, chat: &Chat, is_selected: bool) -> Response {
    use egui::{Align2, FontId};

    let desired = Vec2::new(ui.available_width(), 60.0);
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click());

    // Background for selection
    if is_selected {
        ui.painter().rect_filled(rect, 6.0, crate::gui::styling::ACCENT_PRIMARY);
    }

    // Avatar
    let avatar_rect = Rect::from_min_size(rect.min + Vec2::new(8.0, 8.0), Vec2::new(44.0, 44.0));
    let color = match &chat.peer_fingerprint {
        Some(fp) => fingerprint_to_color(fp),
        None => Color32::LIGHT_GRAY,
    };
    ui.painter().rect_filled(avatar_rect, 10.0, color);
    let initials = get_initials(&chat.title);
    ui.painter().text(
        avatar_rect.center(),
        Align2::CENTER_CENTER,
        initials,
        FontId::proportional(16.0),
        Color32::WHITE,
    );

    // Title
    let title_pos = rect.min + Vec2::new(64.0, 6.0);
    ui.painter().text(
        title_pos,
        Align2::LEFT_TOP,
        &chat.title,
        FontId::proportional(15.0),
        crate::gui::styling::TEXT_PRIMARY,
    );

    // Timestamp of last message
    let ts = chat.messages.last().map(|m: &Message| format_timestamp_relative(&m.timestamp)).unwrap_or_default();
    ui.painter().text(
        rect.max + Vec2::new(-8.0, -22.0),
        Align2::RIGHT_TOP,
        ts,
        FontId::proportional(12.0),
        crate::gui::styling::SUBTLE_TEXT_COLOR,
    );

    response
}