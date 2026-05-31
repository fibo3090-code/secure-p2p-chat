use crate::gui::app_ui::App;
use eframe::egui;

pub fn render_help_window(app: &mut App, ctx: &egui::Context) {
    let mut open = app.active_dialog == crate::gui::app_ui::ActiveDialog::About;
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
    if !open && app.active_dialog == crate::gui::app_ui::ActiveDialog::About {
        app.active_dialog = crate::gui::app_ui::ActiveDialog::None;
    }
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
        ui.label(
            egui::RichText::new("Security Status: MEDIUM")
                .color(egui::Color32::from_rgb(255, 196, 0)),
        );
        ui.add_space(10.0);
        ui.label("A secure, private, and decentralized peer-to-peer messaging application.");
        ui.add_space(20.0);
    });

    ui.group(|ui| {
        ui.heading("Core Philosophy");
        ui.add_space(5.0);
        ui.label("• No Central Server: You own your data. Messages go directly from peer to peer.");
        ui.label("• End-to-End Encryption: Messages and file transfers run inside an authenticated encrypted session.");
        ui.label("• Privacy First: No phone numbers, no email. Just cryptographic identities.");
        ui.label(
            "• Forward Secrecy: Past conversations remain secure even if keys are compromised.",
        );
    });

    ui.add_space(10.0);
    ui.group(|ui| {
        ui.heading("Recent Security Improvements");
        ui.add_space(5.0);
        ui.label(
            "✅ Transcript-bound authenticated encryption for identity proof and transport packets",
        );
        ui.label("✅ Replay protection with sequence validation");
        ui.label("✅ Encrypted identity and encrypted chat history at rest");
        ui.label("✅ Signed invite links in current UI flows");
        ui.label("✅ Diagnostics bundle export and panic/crash support");
        ui.label("✅ Forward secrecy via X25519 + HKDF session establishment");
        ui.label("✅ Self-hosted relay-assisted transport for WAN/NAT-constrained peers");
    });
}

fn render_features_tab(ui: &mut egui::Ui) {
    ui.heading("Security Features");
    ui.add_space(5.0);

    let security_features = [
        (
            "🔒 Transport Encryption",
            "RSA-2048 identity keys, AES-256-GCM transport encryption, and ChaCha20-Poly1305 for encrypted local storage.",
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
            "Securely share files up to 10 GiB with encryption and integrity checks.",
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
            "Signed invite links simplify connection sharing and can optionally carry relay route data.",
        ),
        (
            "🔄 Auto-Reconnect",
            "Automatic reconnect to known contacts when enabled in settings.",
        ),
        (
            "🌐 Relay-Assisted Connectivity",
            "Self-hosted relay transport can help when direct TCP is inconvenient across the internet.",
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
            "The app uses RSA-2048 identity keys, AES-256-GCM for transport, X25519 ECDH for forward secrecy, and ChaCha20-Poly1305 for encrypted local storage. Identity proof and transport encryption are authenticated, but the project still documents a medium overall risk posture rather than claiming a completed audit.",
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
            "Yes. You can use direct TCP with port forwarding or a VPN/overlay network, and the app also supports a self-hosted relay mode for WAN or NAT-constrained peers.",
        ),
        (
            "What does 'Forward Secrecy' mean?",
            "Forward secrecy means that even if your long-term private key is compromised in the future, past conversations remain secure. This is achieved using ephemeral X25519 keys that are generated fresh for each session and destroyed after use.",
        ),
        (
            "How do I enable auto-host?",
            "Go to Settings and enable 'Auto-host on startup'. The application will automatically start listening for connections when you launch it. You can also enable auto-connect to retry known contact addresses from saved history.",
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

    ui.collapsing(
        egui::RichText::new("🔴 Cannot connect to peer").strong(),
        |ui| {
            ui.label("1. Verify the host is running and shows 'Listening on port XXXX' status.");
            ui.label(
                "2. Double-check the IP address and port number (format: 192.168.1.100:8080).",
            );
            ui.label("3. Check firewall settings:");
            ui.label("   • Windows: Allow 'chat-p2p.exe' through Windows Defender Firewall");
            ui.label("   • Linux: sudo ufw allow <port>/tcp");
            ui.label("   • macOS: System Preferences → Security & Privacy → Firewall");
            ui.label("4. For WAN use, prefer a self-hosted relay or a VPN/overlay network if direct TCP is unreliable.");
            ui.label("5. Try pinging the host: ping <IP_ADDRESS>");
            ui.label("6. Verify both users are on the same network or are using a reachable relay/VPN path.");
        },
    );
    ui.add_space(5.0);

    ui.collapsing(
        egui::RichText::new("⚠️ Messages not delivering").strong(),
        |ui| {
            ui.label("• Check the connection status indicator in the chat header.");
            ui.label("• Ensure both parties are online and connected.");
            ui.label("• Large messages are chunked automatically, but older peers that do not understand chunked text will drop them.");
            ui.label(
                "• If status shows 'Reconnecting', wait for the next retry or reconnect manually.",
            );
            ui.label("• Try manually reconnecting: click contact → Connect button.");
            ui.label("• Check the Log Terminal (Settings → Show Log Terminal) for errors.");
        },
    );
    ui.add_space(5.0);

    ui.collapsing(
        egui::RichText::new("🔑 Fingerprint verification failed").strong(),
        |ui| {
            ui.label("• Ensure you're comparing the SAME fingerprint on both devices.");
            ui.label("• Fingerprints are case-insensitive but must match exactly.");
            ui.label(
                "• If fingerprints don't match, DO NOT proceed - this indicates a security issue.",
            );
            ui.label("• Try reconnecting and verify fingerprints again.");
            ui.label("• Verify through a trusted channel (phone call, video, in person).");
        },
    );
    ui.add_space(5.0);

    ui.collapsing(
        egui::RichText::new("📁 File transfer issues").strong(),
        |ui| {
            ui.label("• Maximum file size is 10 GiB - larger files will be rejected.");
            ui.label("• Ensure you have enough disk space in your Downloads folder.");
            ui.label("• Received files are always saved to your configured Downloads directory.");
            ui.label("• You can send files from any folder, but the source file must be a real local file.");
            ui.label("• Cloud-only files from OneDrive/iCloud/Dropbox may need to be marked for offline use first.");
            ui.label("• Check read permission on the source file and write permission on the Downloads directory.");
            ui.label("• Large files may take time - check the progress indicator.");
            ui.label("• If transfer fails, try again or use a smaller file.");
        },
    );
    ui.add_space(5.0);

    ui.collapsing(
        egui::RichText::new("🔐 Identity/password issues").strong(),
        |ui| {
            ui.label("• If you forgot your password, you cannot recover your identity.");
            ui.label("• You'll need to create a new identity and re-verify with contacts.");
            ui.label("• To back up your identity: copy identity.json from the app data folder.");
            ui.label("• To restore: replace identity.json and restart the application.");
            ui.label("• Removing password protection is not supported.");
            ui.label("• Password uses Argon2 - there's no way to bypass or reset it.");
        },
    );
    ui.add_space(5.0);

    ui.collapsing(
        egui::RichText::new("💾 Chat history not loading").strong(),
        |ui| {
            ui.label("• If using encrypted history, ensure you enter the correct password.");
            ui.label("• Check that history.json.enc exists in the app data folder.");
            ui.label("• Corrupted history file: restore from backup or start fresh.");
            ui.label("• Old unencrypted history.json will be migrated on first run.");
        },
    );
    ui.add_space(5.0);

    ui.collapsing(
        egui::RichText::new("🌐 Internet/WAN connectivity").strong(),
        |ui| {
            ui.label("Practical options for internet connectivity:");
            ui.label("1. Direct TCP with router port forwarding.");
            ui.label("2. A VPN or overlay network such as Tailscale, ZeroTier, or WireGuard.");
            ui.label("3. A self-hosted relay server started with `--relay-server`.");
            ui.label("Relay improves reachability, but it is not an anonymity layer.");
        },
    );

    ui.add_space(15.0);
    ui.separator();
    ui.add_space(10.0);
    ui.label(egui::RichText::new("Still having issues?").strong());
    ui.label("• Check the Log Terminal in Settings for detailed error messages.");
    ui.label("• Export a diagnostics bundle from Settings before filing a bug.");
    ui.label(
        "• Review the documentation: README.md, docs/TUTORIAL.md, docs/USER_GUIDE.md, SECURITY.md",
    );
    ui.label("• Report bugs on GitHub with log output and steps to reproduce.");
    ui.label("• Security issues: see SECURITY.md for responsible disclosure.");
}
