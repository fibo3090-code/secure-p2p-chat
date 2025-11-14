# 🔒 Encrypted P2P Messenger

[![Version](https://img.shields.io/badge/version-1.3.0-blue)](https://github.com/yourusername/chat-p2p)
[![License](https://img.shields.io/badge/license-MIT-orange)](LICENSE)
[![Security](https://img.shields.io/badge/security-audited-success)](DEVELOPER_GUIDE.md#5-security)
[![Rust](https://img.shields.io/badge/rust-1.70+-orange)](https://www.rust-lang.org/)

> **Secure, private, peer-to-peer messaging with end-to-end encryption and forward secrecy.**

A modern desktop application for encrypted messaging over local networks (LAN) or VPN, built with **Rust** and **egui**. It implements industry-standard encryption with no central server.

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

### Verify Fingerprints (CRITICAL for Security!)
When you connect to another user, you must verify their fingerprint to prevent man-in-the-middle attacks. Compare the 64-character fingerprint shown in the application with the other user's fingerprint through a separate, secure channel (like a phone call).

---

## 📚 Documentation

- **[DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md)**: Technical details for developers and contributors, including architecture, protocol specifications, and full feature list.
- **[ROADMAP.md](ROADMAP.md)**: The development roadmap and future plans.

---

## 🤝 Contributing

We welcome contributions! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for details on how to report bugs, suggest features, and submit pull requests.

---

## 📜 License

This project is licensed under the **MIT License** - see [LICENSE](LICENSE) for details.
