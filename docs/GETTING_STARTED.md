# Getting Started with Encrypted P2P Messenger

[![Version](https://img.shields.io/badge/version-1.7.1-blue)](../CHANGELOG.md)
[![License](https://img.shields.io/badge/license-MIT-orange)](../LICENSE.md)
[![Security](https://img.shields.io/badge/security-audited-success)](../SECURITY.md)
[![Rust](https://img.shields.io/badge/rust-1.70+-orange)](https://www.rust-lang.org/)

> **Secure, private, peer-to-peer messaging with end-to-end encryption and forward secrecy.**

This guide combines the **introduction** and **setup** for the Encrypted P2P Messenger. It explains what the app is, its features, and how to build, run, and use it securely.

---

## 1. What Is This?

Encrypted P2P Messenger is a **desktop application** for secure messaging built with these principles:

- **Privacy First**: No central server, no data collection, no tracking. Your conversations are your own.
- **End-to-End Encryption**: RSA for identity, AES-256-GCM for messages. Messages are encrypted from send until receive.
- **Forward Secrecy**: X25519 ECDH ensures past messages stay secure even if long-term keys are compromised.
- **Peer-to-Peer**: Direct connections on your LAN or VPN—no central server as a single point of failure.
- **Open Source**: The codebase is open for inspection, audit, and contribution.

---

## 2. Features

- **Secure Messaging**: State-of-the-art encryption for all conversations.
- **Encrypted Identity Exchange (Protocol v3)**: Identity is exchanged inside an encrypted tunnel so metadata is hidden from observers.
- **Password-Protected Identity**: Your private key is encrypted on disk (Argon2 + ChaCha20-Poly1305). You must unlock with your password before using the app.
- **Local Peer Discovery (mDNS)**: Optional automatic discovery of peers on your LAN.
- **File Transfer**: Chunked, with progress; supports large files up to the configured maximum.
- **Typing Indicators**, **Desktop Notifications**, **Emoji Picker**, **Drag & Drop** files.
- **Invite Links & QR Codes**: Add contacts via `chat-p2p://invite/...` or a QR code.
- **Local Persistence**: Chat history and identity stored locally; you control your data.
- **Auto-Host & Auto-Rehost**: Optional auto-listen on startup and auto re-listen after a connection.

---

## 3. Prerequisites

- **Rust 1.70+**: [rustup.rs](https://rustup.rs/)
- **Network**: Same LAN or VPN to reach other users.

---

## 4. Build & Run

1. **Clone the repository**

   ```bash
   git clone <repository-url>
   cd chat-p2p
   ```

2. **Build (release recommended)**

   ```bash
   cargo build --release
   ```

3. **Run**

   ```bash
   cargo run --release
   ```

   > **Note**: The CLI is for launching the GUI. Standalone CLI operation with `--host` and `--connect` is not implemented. Please use the GUI to host or connect.

   On first run (or when your identity is encrypted), you will see a **blocking unlock/set-password screen**. You must enter your password to unlock, or set a password for a new identity, before the main UI (chats, connections, etc.) is available. This cannot be bypassed.

### Platform-Specific

#### Windows

- Use **PowerShell** or **Windows Terminal**.
- **SmartScreen**: For packaged binaries, you may need “More info” → “Run anyway” if you trust the source.
- **Packaging**: Use `./build-and-package.ps1` to produce a distributable build and installer. The installer places the app icon in Add/Remove Programs (Settings → Apps) via `encodeur_rsa_icon.ico`.
- **PATH**: If `cargo` is not found, run `$env:Path += ';$HOME\.cargo\bin'` in PowerShell.

---

## 5. Verifying Fingerprints (Critical for Security)

To protect against **Man-in-the-Middle (MITM)** attacks, you must **verify the fingerprint** of each contact on first connection.

- A **fingerprint** is a 64-character hex string that uniquely identifies a public key.
- **When**: On first connection and whenever a contact’s device or key may have changed.
- **How**: Compare the fingerprint in the app with the one your peer provides over a **separate, secure channel** (e.g. phone call, in-person).
- **Do not** verify over the same unencrypted channel you are about to protect.
- **If they do not match**: Disconnect and investigate. Do not proceed.

---

## 6. Where to Go Next

- **Architecture**: [docs/03_architecture.md](03_architecture.md)
- **Protocol**: [docs/04_protocol.md](04_protocol.md)
- **Development**: [DEVELOPER_GUIDE.md](../DEVELOPER_GUIDE.md)
- **Security**: [SECURITY.md](../SECURITY.md)
- **Contributing**: [CONTRIBUTING.md](../CONTRIBUTING.md)
