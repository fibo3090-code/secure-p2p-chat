# 🔒 Encrypted P2P Messenger

[![Version](https://img.shields.io/badge/version-1.7.7-blue)](CHANGELOG.md)
[![License](https://img.shields.io/badge/license-MIT-orange)](#-license)
[![Security](https://img.shields.io/badge/security-audited-success)](SECURITY.md)
[![Rust](https://img.shields.io/badge/rust-1.70+-orange)](https://www.rust-lang.org/)

> **Secure, private, peer-to-peer messaging with end-to-end encryption and forward secrecy.**

A modern desktop application for encrypted messaging over local networks (LAN) or VPN, built with **Rust** and **egui**. It implements industry-standard encryption and has no central server.

[Quick Start](#-quick-start) • [Documentation](#-documentation) • [Contributing](CONTRIBUTING.md)

---

## 🎯 What is this?

Encrypted P2P Messenger is a **desktop application** for secure messaging built with these principles:

- **Privacy First**: No central server, no data collection, no tracking. Your conversations are your own.
- **End-to-End Encryption**: AES-256-GCM authenticated encryption for all traffic; RSA/Ed25519 for identity.
- **Forward Secrecy**: X25519 ECDH ensures past messages stay secure even if long-term keys are compromised.
- **Peer-to-Peer**: Direct connections on your LAN or VPN—no central server as a single point of failure.
- **Open Source**: Transparent, auditable, and free forever.

---

## ✨ Key Features

- **Security & Privacy**
  - **Metadata Protection**: Protocol v3 hides identities from passive observers during the handshake.
  - **Password-Protected Identity**: Private keys are encrypted on disk (Argon2 + ChaCha20-Poly1305).
  - **Session Key Rotation**: Automatic re-keying every 100 messages or 5 minutes.
  - **Replay Protection**: Transport-layer sequence numbers prevent message injection.
  - **DoS Mitigation**: Global rate limiting and strict handshake timeouts.

- **User Experience**
  - **Local Discovery**: Automatically find peers on your LAN via mDNS (Bonjour/Avahi).
  - **Rich Messaging**: Typing indicators, emojis, and desktop notifications.
  - **File Transfer**: Drag-and-drop file sharing with chunked transmission and progress tracking.
  - **Invite Links & QR Codes**: Share contact info via `chat-p2p://invite/...` or visual QR codes.
  - **Zero-Config**: No accounts needed; uses a Trust-on-First-Use (TOFU) model.

---

## 💻 System Requirements & Compatibility

| Platform | Status | Prerequisites |
|----------|--------|---------------|
| **Windows** | ✅ Supported | Requires [Bonjour Print Services](https://support.apple.com/kb/DL999) for peer discovery. |
| **Linux** | ⚠️ Supported | Requires `avahi-daemon` and libraries (`libgtk-3-dev`, `libxcb-dev`, `libfontconfig-dev`). |
| **macOS** | ⚠️ Experimental | Works natively with built-in Bonjour; lacks automated packaging. |

---

## 🚀 Quick Start

### 1. Installation

To build from source, ensure you have [Rust 1.70+](https://rustup.rs/) installed.

```bash
# Clone the repository
git clone <repository-url>
cd chat-p2p

# Build release version
cargo build --release

# Run the GUI
cargo run --release

# Run the TUI
cargo run --release -- --tui
```

### 2. First Run (Unlock Identity)

On the first launch, you will be prompted to set a password for your new identity. This password encrypts your private keys at rest and is required every time you open the app. **This cannot be bypassed.**

### 3. Verify Fingerprints (CRITICAL)

To protect against **Man-in-the-Middle (MITM)** attacks:

1. Connect to a peer.
2. Compare the 64-character fingerprint (or the colored grid) shown in the app with the one your peer provides over a **separate, secure channel** (e.g., phone call, in-person).
3. Only click **Trust** if they match exactly.

---

## 📚 Documentation

### Core Guides

- **[DEVELOPMENT_PLAN.md](DEVELOPMENT_PLAN.md)**: Unified project roadmap, security initiatives, and backlog.
- **[DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md)**: Architecture details, protocol specs, and build instructions.
- **[SECURITY.md](SECURITY.md)**: Security policy, audit history, and vulnerability disclosure.
- **[DESIGN_NOTES.md](DESIGN_NOTES.md)**: UI/UX principles, design patterns, and visual language.

### Technical Specs

- **[docs/03_architecture.md](docs/03_architecture.md)**: Detailed system component mapping.
- **[docs/04_protocol.md](docs/04_protocol.md)**: Handshake, framing, and message format specifications.

---

## 🤝 Contributing

We welcome contributions! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for details on how to report bugs, suggest features, and submit pull requests.

---

## 📜 License

This project is licensed under the **MIT License** - see [LICENSE.md](LICENSE.md) for details.
