use crate::gui::app_ui::App;
use eframe::egui;

pub fn render_sidebar(app: &mut App, ui: &mut egui::Ui) {
    ui.add_space(crate::gui::styling::SPACING_MEDIUM);
    ui.horizontal(|ui| {
        ui.heading("💬 Chats");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button("➕", |ui| {
                if ui.button("🎤 Host Connection").clicked() {
                    tracing::info!("Host connection dialog opened");
                    app.active_dialog = crate::gui::app_ui::ActiveDialog::Host;
                    ui.close_menu();
                }
                if ui.button("🔌 Connect to Host").clicked() {
                    tracing::info!("Connect to host dialog opened");
                    app.active_dialog = crate::gui::app_ui::ActiveDialog::Connect;
                    ui.close_menu();
                }
            })
            .response
            .on_hover_text("New connection");
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
                        tracing::info!("Rename chat context menu clicked for chat_id: {}", chat_id);
                        app.rename_chat_id = Some(chat_id);
                        app.rename_input = chat.title.clone();
                        app.active_dialog = crate::gui::app_ui::ActiveDialog::RenameChat;
                        ui.close_menu();
                    }
                    if ui.button("🗑 Delete chat").clicked() {
                        tracing::info!("Delete chat context menu clicked for chat_id: {}", chat_id);
                        app.chat_to_delete = Some(chat_id);
                        app.active_dialog = crate::gui::app_ui::ActiveDialog::DeleteChat;
                        ui.close_menu();
                    }
                });
            }
        }
    });
}
