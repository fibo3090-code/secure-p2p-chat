use crate::gui::app_ui::App;
use eframe::egui;

pub fn render_help_window(app: &mut App, ctx: &egui::Context) {
    let mut open = app.show_about;
    egui::Window::new("ℹ️ Help & Support")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(600.0)
        .default_height(500.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut app.help_tab, 0, "About");
                ui.selectable_value(&mut app.help_tab, 1, "Features");
                ui.selectable_value(&mut app.help_tab, 2, "FAQ");
                ui.selectable_value(&mut app.help_tab, 3, "Troubleshooting");
            });
            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                match app.help_tab {
                    0 => render_about_tab(ui),
                    1 => render_features_tab(ui),
                    2 => render_faq_tab(ui),
                    3 => render_troubleshooting_tab(ui),
                    _ => render_about_tab(ui),
                }
            });
        });
    app.show_about = open;
}

fn render_about_tab(ui: &mut egui::Ui) {
    ui.vertical_centered(|ui| {
        ui.add_space(20.0);
        ui.heading(egui::RichText::new("Encrypted P2P Messenger").size(24.0).strong());
        ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
        ui.add_space(10.0);
        ui.label("A secure, private, and unstoppable peer-to-peer messaging application.");
        ui.add_space(20.0);
    });

    ui.group(|ui| {
        ui.heading("Core Philosophy");
        ui.add_space(5.0);
        ui.label("• No Central Server: You own your data. Messages go directly from peer to peer.");
        ui.label("• End-to-End Encryption: Every message is encrypted. Only the recipient can read it.");
        ui.label("• Anonymity: No phone numbers, no email addresses required. Just cryptographic keys.");
    });
}

fn render_features_tab(ui: &mut egui::Ui) {
    let features = [
        ("🔒 Encryption", "RSA-2048-OAEP for key exchange, AES-256-GCM for messages."),
        ("🔐 Forward Secrecy", "X25519 ECDH + HKDF for ephemeral session keys."),
        ("📁 File Transfer", "Securely shear files of any size (up to configured limit)."),
        ("👥 Group Chats", "Create private groups with trusted contacts."),
        ("⌨️ Typing Indicators", "See when your friends are typing in real-time."),
        ("🎨 Customization", "Choose from multiple themes (Light, Dark, Midnight, Forest)."),
    ];

    for (title, desc) in features {
        ui.group(|ui| {
            ui.strong(title);
            ui.label(desc);
        });
        ui.add_space(5.0);
    }
}

fn render_faq_tab(ui: &mut egui::Ui) {
    ui.label(egui::RichText::new("Frequently Asked Questions").heading());
    ui.add_space(10.0);

    let faqs = [
        ("How do I connect to a friend?", "One person must 'Host', and the other must 'Connect' using the host's IP and Port. Or use the 'Invite Link' feature to share connection info easily."),
        ("Is it really secure?", "Yes. We use industry-standard cryptographic primitives (RSA, AES-GCM, X25519). Private keys never leave your device."),
        ("Where are files saved?", "Files are saved to your configured Download Directory (default: Downloads folder). You can change this in Settings."),
        ("Can I chat over the internet?", "Yes, if the Host port is forwarded or if you are using a VPN/Overlay network (like Hamachi, or Tailscale). By default, it works on Local LAN."),
    ];

    for (q, a) in faqs {
        ui.collapsing(egui::RichText::new(q).strong(), |ui| {
            ui.label(a);
        });
        ui.add_space(5.0);
    }
}

fn render_troubleshooting_tab(ui: &mut egui::Ui) {
    ui.label(egui::RichText::new("Troubleshooting Guide").heading());
    ui.add_space(10.0);

    ui.label("🔴 Cannot connect?");
    ui.label("1. Check if the Host is running (green 'Listening' status).");
    ui.label("2. Verify the IP address and Port.");
    ui.label("3. Check Windows Firewall (ensure 'chat-p2p' is allowed).");
    ui.label("4. Try pinging the other computer.");

    ui.add_space(10.0);
    ui.label("⚠️ Messages not delivering?");
    ui.label("• Ensure both parties are online.");
    ui.label("• Check the 'Connection' indicator in the chat.");

    ui.add_space(15.0);
    ui.label("Still having issues? Check the Log Terminal in Settings for detailed errors.");
}
