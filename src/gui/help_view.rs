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

            egui::ScrollArea::vertical().show(ui, |ui| match app.help_tab {
                0 => render_about_tab(ui),
                1 => render_features_tab(ui),
                2 => render_faq_tab(ui),
                3 => render_troubleshooting_tab(ui),
                _ => render_about_tab(ui),
            });
        });
    app.show_about = open;
}

fn render_about_tab(ui: &mut egui::Ui) {
    ui.vertical_centered(|ui| {
        ui.add_space(20.0);
        ui.heading(
            egui::RichText::new("Encrypted P2P Messenger")
                .size(24.0)
                .strong(),
        );
        ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
        ui.label(egui::RichText::new("Security Status: HIGH").color(egui::Color32::from_rgb(0, 200, 50)));
        ui.add_space(10.0);
        ui.label("A secure, private, and decentralized peer-to-peer messaging application.");
        ui.add_space(20.0);
    });

    ui.group(|ui| {
        ui.heading("Core Philosophy");
        ui.add_space(5.0);
        ui.label("• No Central Server: You own your data. Messages go directly from peer to peer.");
        ui.label(
            "• End-to-End Encryption: Every message is encrypted with military-grade cryptography.",
        );
        ui.label(
            "• Privacy First: No phone numbers, no email. Just cryptographic identities.",
        );
        ui.label(
            "• Forward Secrecy: Past conversations remain secure even if keys are compromised.",
        );
    });

    ui.add_space(10.0);
    ui.group(|ui| {
        ui.heading("Recent Security Improvements");
        ui.add_space(5.0);
        ui.label("✅ Protocol v3 Encrypted Identity (Metadata Protection)");
        ui.label("✅ DoS Protection (Rate Limiting & Timeouts)");
        ui.label("✅ Secure Memory Wiping (Zeroize)");
        ui.label("✅ Encrypted chat history at rest (ChaCha20-Poly1305)");
        ui.label("✅ Replay attack protection with sequence numbers");
        ui.label("✅ Counter-based nonces for AES-GCM");
        ui.label("✅ Fingerprint verification enforcement");
        ui.label("✅ Thread-safe implementation (no unsafe code)");
    });
}

fn render_features_tab(ui: &mut egui::Ui) {
    ui.heading("Security Features");
    ui.add_space(5.0);
    
    let security_features = [
        (
            "🔒 Military-Grade Encryption",
            "RSA-2048-OAEP for identity, AES-256-GCM for messages, ChaCha20-Poly1305 for storage.",
        ),
        (
            "🔐 Forward Secrecy",
            "X25519 ECDH + HKDF-SHA256 for ephemeral session keys. Past messages stay secure.",
        ),
        (
            "🛡️ Replay Protection",
            "Sequence numbers prevent message replay attacks.",
        ),
        (
            "🔑 Password Protection",
            "Argon2 key derivation for identity encryption.",
        ),
        (
            "✅ Fingerprint Verification",
            "Manual verification prevents man-in-the-middle attacks.",
        ),
    ];

    for (title, desc) in security_features {
        ui.group(|ui| {
            ui.strong(title);
            ui.label(desc);
        });
        ui.add_space(5.0);
    }

    ui.add_space(10.0);
    ui.heading("Communication Features");
    ui.add_space(5.0);

    let comm_features = [
        (
            "💬 Secure Messaging",
            "Real-time encrypted text messaging with delivery confirmation.",
        ),
        (
            "📁 File Transfer",
            "Securely share files up to 2GB with encryption and integrity checks.",
        ),
        (
            "⌨️ Typing Indicators",
            "See when contacts are typing in real-time.",
        ),
        (
            "🔔 Desktop Notifications",
            "Get notified of new messages even when minimized.",
        ),
        (
            "📋 Invite Links",
            "Easy connection sharing with chat-p2p:// links.",
        ),
        (
            "🔄 Auto-Reconnect",
            "Automatic reconnection with exponential backoff.",
        ),
    ];

    for (title, desc) in comm_features {
        ui.group(|ui| {
            ui.strong(title);
            ui.label(desc);
        });
        ui.add_space(5.0);
    }

    ui.add_space(10.0);
    ui.heading("Interface Features");
    ui.add_space(5.0);

    let ui_features = [
        (
            "🎨 Multiple Themes",
            "Light, Dark, Midnight, and Forest themes.",
        ),
        (
            "😊 Emoji Support",
            "Built-in emoji picker for expressive messaging.",
        ),
        (
            "📝 Chat History",
            "Persistent encrypted chat history with search.",
        ),
        (
            "👤 Contact Management",
            "Organize contacts with names, addresses, and notes.",
        ),
    ];

    for (title, desc) in ui_features {
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
        (
            "How do I connect to a friend?",
            "One person must 'Host' (listen for connections), and the other must 'Connect' using the host's IP address and port. The easiest way is to use the 'Invite Link' feature - the host generates a link and shares it, then the other person pastes it in the Invite Link tab.",
        ),
        (
            "Is it really secure?",
            "Yes. We use military-grade cryptography: RSA-2048-OAEP for identity, AES-256-GCM for messages, X25519 ECDH for forward secrecy, and ChaCha20-Poly1305 for storage. All critical vulnerabilities have been fixed (100% critical, 80% high-priority). Private keys never leave your device and can be password-protected with Argon2.",
        ),
        (
            "What is fingerprint verification?",
            "Each user has a unique cryptographic fingerprint (64-character hex string). Before chatting, both users should verify they see the same fingerprint for each other - this prevents man-in-the-middle attacks. You can verify via phone, video call, or in person.",
        ),
        (
            "Where are my messages stored?",
            "Chat history is stored locally on your device in an encrypted file (history.json.enc) using ChaCha20-Poly1305 encryption. Only you can decrypt it with your password. Messages are never sent to any server.",
        ),
        (
            "Where are files saved?",
            "Received files are saved to your configured Download Directory (default: Downloads folder). You can change this in Settings → Download Directory.",
        ),
        (
            "Can I chat over the internet?",
            "Yes, but it requires port forwarding on the host's router, or using a VPN/overlay network (like Tailscale, Hamachi, ZeroTier, or WireGuard). By default, it works on your local network (LAN).",
        ),
        (
            "What does 'Forward Secrecy' mean?",
            "Forward secrecy means that even if your long-term private key is compromised in the future, past conversations remain secure. This is achieved using ephemeral X25519 keys that are generated fresh for each session and destroyed after use.",
        ),
        (
            "How do I enable auto-host?",
            "Go to Settings and enable 'Auto-host on startup'. The application will automatically start listening for connections when you launch it. You can also enable 'Auto-reconnect' to automatically reconnect to known contacts.",
        ),
        (
            "Can I use this on multiple devices?",
            "Yes, but each device has its own identity (private key). You'll need to verify fingerprints separately for each device. To use the same identity across devices, you can export/import your identity file (identity.json).",
        ),
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

    ui.collapsing(egui::RichText::new("🔴 Cannot connect to peer").strong(), |ui| {
        ui.label("1. Verify the host is running and shows 'Listening on port XXXX' status.");
        ui.label("2. Double-check the IP address and port number (format: 192.168.1.100:8080).");
        ui.label("3. Check firewall settings:");
        ui.label("   • Windows: Allow 'chat-p2p.exe' through Windows Defender Firewall");
        ui.label("   • Linux: sudo ufw allow <port>/tcp");
        ui.label("   • macOS: System Preferences → Security & Privacy → Firewall");
        ui.label("4. If connecting over internet, ensure port forwarding is configured on router.");
        ui.label("5. Try pinging the host: ping <IP_ADDRESS>");
        ui.label("6. Verify both users are on the same network (or using VPN/port forwarding).");
    });
    ui.add_space(5.0);

    ui.collapsing(egui::RichText::new("⚠️ Messages not delivering").strong(), |ui| {
        ui.label("• Check the connection status indicator in the chat header.");
        ui.label("• Ensure both parties are online and connected.");
        ui.label("• If status shows 'Reconnecting', wait for automatic reconnection.");
        ui.label("• Try manually reconnecting: click contact → Connect button.");
        ui.label("• Check the Log Terminal (Settings → Show Log Terminal) for errors.");
    });
    ui.add_space(5.0);

    ui.collapsing(egui::RichText::new("🔑 Fingerprint verification failed").strong(), |ui| {
        ui.label("• Ensure you're comparing the SAME fingerprint on both devices.");
        ui.label("• Fingerprints are case-insensitive but must match exactly.");
        ui.label("• If fingerprints don't match, DO NOT proceed - this indicates a security issue.");
        ui.label("• Try reconnecting and verify fingerprints again.");
        ui.label("• Verify through a trusted channel (phone call, video, in person).");
    });
    ui.add_space(5.0);

    ui.collapsing(egui::RichText::new("📁 File transfer issues").strong(), |ui| {
        ui.label("• Maximum file size is 2GB - larger files will be rejected.");
        ui.label("• Ensure you have enough disk space in your Downloads folder.");
        ui.label("• Check file permissions on the Downloads directory.");
        ui.label("• Large files may take time - check the progress indicator.");
        ui.label("• If transfer fails, try again or use a smaller file.");
    });
    ui.add_space(5.0);

    ui.collapsing(egui::RichText::new("🔐 Identity/password issues").strong(), |ui| {
        ui.label("• If you forgot your password, you cannot recover your identity.");
        ui.label("• You'll need to create a new identity and re-verify with contacts.");
        ui.label("• To backup your identity: copy identity.json from the app data folder.");
        ui.label("• To restore: replace identity.json and restart the application.");
        ui.label("• Password uses Argon2 - there's no way to bypass or reset it.");
    });
    ui.add_space(5.0);

    ui.collapsing(egui::RichText::new("💾 Chat history not loading").strong(), |ui| {
        ui.label("• If using encrypted history, ensure you enter the correct password.");
        ui.label("• Check that history.json.enc exists in the app data folder.");
        ui.label("• Corrupted history file: restore from backup or start fresh.");
        ui.label("• Old unencrypted history.json will be migrated on first run.");
    });
    ui.add_space(5.0);

    ui.collapsing(egui::RichText::new("🌐 Internet/WAN connectivity").strong(), |ui| {
        ui.label("For internet connectivity, you need:");
        ui.label("1. Port forwarding on the host's router (forward external port → internal port).");
        ui.label("2. Host's public IP address (check whatismyip.com).");
        ui.label("3. Or use a VPN solution like Tailscale, ZeroTier, or WireGuard.");
        ui.label("4. Dynamic DNS if host's IP changes frequently.");
        ui.label("\nEasiest solution: Use Tailscale (free, no port forwarding needed).");
    });

    ui.add_space(15.0);
    ui.separator();
    ui.add_space(10.0);
    ui.label(egui::RichText::new("Still having issues?").strong());
    ui.label("• Check the Log Terminal in Settings for detailed error messages.");
    ui.label("• Review the documentation: README.md, SECURITY.md, DEVELOPER_GUIDE.md");
    ui.label("• Report bugs on GitHub with log output and steps to reproduce.");
    ui.label("• Security issues: see SECURITY.md for responsible disclosure.");
}
