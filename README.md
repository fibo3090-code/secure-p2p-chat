# 🔒 Encrypted P2P Messenger

[![Version](https://img.shields.io/badge/version-1.3.0-blue)](https://github.com/yourusername/chat-p2p)
[![License](https://img.shields.io/badge/license-MIT-orange)](LICENSE)
[![Security](https://img.shields.io/badge/security-audited-success)](docs/developer_guide/security.md)
[![Rust](https://img.shields.io/badge/rust-1.70+-orange)](https://www.rust-lang.org/)

> **Secure, private, peer-to-peer messaging with end-to-end encryption and forward secrecy.**

A modern desktop application for encrypted messaging over local networks (LAN) or VPN, built with **Rust** and **egui**. Implements **industry-standard encryption** (Signal/WhatsApp equivalent) with no central server.

[Features](#-features) • [Quick Start](#-quick-start) • [Security](#-security) • [Documentation](#-documentation) • [Contributing](CONTRIBUTING.md)

---

## 🎯 What is this?

Encrypted P2P Messenger is a **desktop application** for secure messaging built with these principles:

- **Privacy First**: No central server, no data collection, no tracking.
- **End-to-End Encryption**: Military-grade cryptography (RSA-2048 + AES-256-GCM).
- **Forward Secrecy**: Past messages stay secure even if keys are compromised (X25519 ECDH).
- **Peer-to-Peer**: Direct connections between devices on your local network.
- **Open Source**: Transparent, auditable, and free forever.

Perfect for secure team communication, private file sharing, or just chatting with friends without Big Tech surveillance.

---

## ✨ Features

### 🔒 Core Security
- **End-to-end encryption** with AES-256-GCM (authenticated encryption).
- **Forward secrecy** using X25519 ECDH key exchange.
- **Fingerprint verification** to prevent man-in-the-middle attacks.
- **Tamper-proof messages** with GCM authentication tags.
- **No key reuse**: Fresh ephemeral keys for each session.
- **Protocol Version 2**: Prevents downgrade attacks.

### 💬 Messaging
- **Text messaging** with timestamps.
- **File sharing** with drag-and-drop support (any file size).
- **Emoji picker** with 32 common emojis.
- **Typing indicators** (real-time "typing..." status).
- **Desktop notifications** (cross-platform).
- **Message history** (persistent, JSON-based).
- **Auto-save** conversation history.

### 👥 Contacts & Groups
- **Contact management** (add, remove, search).
- **Group chats** (multi-participant conversations).
- **Rename conversations** with custom titles.
- **Colorful avatars** (generated from fingerprints).

### 🎨 User Experience
- **Modern UI** (WhatsApp-like interface with dark theme).
- **Drag & drop files** directly into chat window.
- **Multiline text input** with Ctrl+Enter to send.
- **Cross-platform** (Windows, Linux, macOS).

---

## 🚀 Quick Start

### Prerequisites
- **Rust 1.70+** (install from [rustup.rs](https://rustup.rs/))
- **Network access** (same LAN or VPN)

### Build & Run

```bash
# Clone the repository
git clone <repository-url>
cd chat-p2p

# Build release version
cargo build --release
```

The application can be run in either GUI or CLI mode.

**GUI Mode (Recommended):**
```bash
cargo run --release
```

**CLI Mode:**

- **Start as Host:**
    ```bash
    .\target\release\encodeur_rsa_rust.exe --host
    ```

- **Connect as Client:**
    ```bash
    .\target\release\encodeur_rsa_rust.exe --connect <HOST_IP>
    ```

### Verify Fingerprints (CRITICAL for Security!)
1. Compare the displayed 64-character fingerprints.
2. Verify via another secure channel (phone call, in-person, etc.).
3. Click "Continue" **only if they match**.

**⚠️ Never skip fingerprint verification!** This is your protection against man-in-the-middle attacks.


---

## 🔐 Security

This application protects against:
- ✅ **Eavesdropping**: All messages encrypted end-to-end.
- ✅ **Tampering**: GCM authentication tags detect modifications.
- ✅ **Replay attacks**: Random nonces prevent message replay.
- ✅ **Key compromise**: Forward secrecy protects past sessions.
- ✅ **Downgrade attacks**: Protocol version negotiation.

### Cryptography Stack
| Component | Algorithm | Key Size | Purpose |
|-----------|-----------|----------|---------|
| Key Exchange | X25519 ECDH | 256-bit | Forward secrecy |
| Session Derivation | HKDF-SHA256 | 256-bit | Key material expansion |
| Identity | RSA-OAEP | 2048-bit | Authentication |
| Message Encryption | AES-256-GCM | 256-bit | Confidentiality + integrity |
| Fingerprints | SHA-256 | 256-bit | Key verification |

[→ Read the full security documentation](docs/developer_guide/security.md)

---

## 📚 Documentation

- **[User Guide](user_guide.md)**: Instructions on how to install and use the application.
- **[Developer Guide](dev_guide.md)**: Technical details for developers and contributors.
- **[Roadmap](ROADMAP.md)**: The development roadmap and future plans.

---

## 🤝 Contributing

We welcome contributions! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for:
- How to report bugs
- How to suggest features
- Code style guidelines
- Pull request process

---

## 📜 License

This project is licensed under the **MIT License** - see [LICENSE](LICENSE) for details.