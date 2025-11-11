use crate::gui::app_ui::App;
use eframe::egui;

pub fn render_sidebar(app: &mut App, ui: &mut egui::Ui) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.heading("💬 Chats");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("➕").on_hover_text("New connection").clicked() {
                ui.menu_button("", |ui| {
                    if ui.button("🎤 Host Connection").clicked() {
                        app.show_host_dialog = true;
                        ui.close_menu();
                    }
                    if ui.button("🔌 Connect to Host").clicked() {
                        app.show_connect_dialog = true;
                        ui.close_menu();
                    }
                });
            }
        });
    });
    ui.separator();

    egui::ScrollArea::vertical().show(ui, |ui| {
        if let Ok(manager) = app.chat_manager.try_lock() {
            let mut chats: Vec<_> = manager.chats.values().collect();
            chats.sort_by(|a, b| b.created_at.cmp(&a.created_at));

            if chats.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(50.0);
                    ui.label(
                        egui::RichText::new("No active chats")
                            .color(crate::gui::styling::SUBTLE_TEXT_COLOR),
                    );
                    ui.label(
                        egui::RichText::new("Click ➕ to start a connection")
                            .color(crate::gui::styling::SUBTLE_TEXT_COLOR),
                    );
                });
            }

            for chat in chats {
                let is_selected = app.selected_chat == Some(chat.id);
                let chat_id = chat.id;

                let response = crate::gui::widgets::chat_list_item(ui, chat, is_selected);
                if response.clicked() {
                    app.selected_chat = Some(chat_id);
                }

                response.context_menu(|ui| {
                    if ui.button("✏️ Rename chat").clicked() {
                        app.rename_chat_id = Some(chat_id);
                        if let Ok(manager) = app.chat_manager.try_lock() {
                            if let Some(chat) = manager.get_chat(chat_id) {
                                app.rename_input = chat.title.clone();
                            }
                        }
                        app.show_rename_dialog = true;
                        ui.close_menu();
                    }
                    if ui.button("🗑 Delete chat").clicked() {
                        app.chat_to_delete = Some(chat_id);
                        ui.close_menu();
                    }
                });
            }
        }
    });
}
