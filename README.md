# 🔒 Encrypted P2P Messenger

[![Version](https://img.shields.io/badge/version-1.3.0-blue)](#)
[![License](https://img.shields.io/badge/license-MIT-orange)](#-license)
[![Security](https://img.shields.io/badge/security-audited-success)](#)
[![Rust](https://img.shields.io/badge/rust-1.70+-orange)](https://www.rust-lang.org/)

> **Secure, private, peer-to-peer messaging with end-to-end encryption and forward secrecy.**

A modern desktop application for encrypted messaging over local networks (LAN) or VPN, built with **Rust** and **egui**. It implements industry-standard encryption and has no central server.

[Quick Start](#-quick-start) • [Documentation](#-documentation) • [Contributing](CONTRIBUTING.md)

---

## 🎯 What is this?

Encrypted P2P Messenger is a **desktop application** for secure messaging built with these principles:

- **Privacy First**: No central server, no data collection, no tracking.
- **End-to-End Encryption**: Military-grade cryptography.
- **Forward Secrecy**: Past messages stay secure even if keys are compromised.
- **Peer-to-Peer**: Direct connections between devices on your local network.
- **Open Source**: Transparent, auditable, and free forever.

---

## ✨ Features

- **Secure messaging** with AES-256-GCM and X25519 forward secrecy
- **Peer discovery ready** (manual connect today; mDNS planned)
- **File transfer** with chunking and progress
- **Typing indicators** and desktop notifications
- **Emoji picker** and drag & drop files
- **Invite links + QR codes** to onboard contacts quickly
- **Local persistence** of history and identity (no server)

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

# Run the application
cargo run --release
```

### Windows

- Recommended shell: PowerShell or Windows Terminal
- If SmartScreen warns about an unknown app when running packaged binaries, choose “More info” → “Run anyway” (if you trust the source)
- Packaging script (optional): `./build-and-package.ps1` produces a distributable build

### Verify Fingerprints (CRITICAL for Security!)
When you connect to another user, you must verify their fingerprint to prevent man-in-the-middle attacks. Compare the 64-character fingerprint shown in the application with the other user's fingerprint through a separate, secure channel (like a phone call).

Tips:
- Always verify at first contact and when a peer’s device changes.
- Prefer voice or in-person verification over chat.
- If fingerprints don’t match, disconnect and investigate.

---

## 📚 Documentation
All project documentation is located in the root of the repository, not in a separate 'docs/' folder.

- **[DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md)**: The primary technical guide covering architecture, protocols, and build instructions.
- **[ROADMAP.md](ROADMAP.md)**: Outlines the development roadmap and future plans.
- **[SECURITY.md](SECURITY.md)**: Details the project's security policy and threat model.
- **[CHANGELOG.md](CHANGELOG.md)**: Release notes, fixes, and improvements.
- **[CONTRIBUTING.md](CONTRIBUTING.md)**: How to open issues and send PRs.

---

## 📸 Screenshots

Screenshots of the chat list, message view, fingerprint dialog, and file transfer will be added here. If you have design feedback or want to contribute visuals, see [CONTRIBUTING.md](CONTRIBUTING.md).

---

## 🤝 Contributing

We welcome contributions! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for details on how to report bugs, suggest features, and submit pull requests.

---

## 📜 License

This project is licensed under the **MIT License** - see [LICENSE.md](LICENSE.md) for details.


