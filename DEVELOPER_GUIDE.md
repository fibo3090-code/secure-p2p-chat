# Developer Guide

This comprehensive guide provides all the necessary information to understand, build, run, and contribute to the Encrypted P2P Messenger. It's intended for both new contributors and advanced users.

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

- **Rust 1.86+**: [rustup.rs](https://rustup.rs/)
- **Network**: Same LAN or VPN to reach other users.

---

## 4. Building and Running the Application

This section guides you through setting up your development environment, building the application from source, and running it.

### 4.1. Development Setup

1. **Install Rust**: Make sure you have the latest stable version of Rust installed from [rustup.rs](https://rustup.rs/).
2. **Clone the repository**: `git clone <repository-url>`
3. **Navigate to project directory**: `cd chat-p2p`
4. **Build the project**: `cargo build`

### 4.2. Build & Run Commands

- **Build**: `cargo build`
- **Build (Release, recommended for use)**: `cargo build --release`
- **Run (GUI)**: `cargo run --release`
- **Run (TUI)**: `cargo run --release -- --tui`
- **Run (TUI + host on port 9000)**: `cargo run --release -- --tui --host --port 9000`
- **Run (TUI + connect)**: `cargo run --release -- --tui --connect 127.0.0.1:12345`

> **Note**: GUI remains the default mode. Use `--tui` to launch the terminal interface.
>
> On first run (or when your identity is encrypted), you will see a **blocking unlock/set-password screen**. You must enter your password to unlock, or set a password for a new identity, before the main UI (chats, connections, etc.) is available. This cannot be bypassed.

### 4.3. Platform-Specific Notes

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

## 6. Code Quality Standards

This project adheres to high quality standards.

- **Formatting**: Checked via `cargo fmt`.
- **Linting**: Checked via `cargo clippy` (must be clean).
- **Security**: checked via `cargo-audit` and `cargo-deny`.
- **Testing**:
  - **Unit Tests**: Required for all crypto and core logic.
  - **Integration Tests**: Required for network flows.
  - **UI Tests**: TUI rendering and interaction regression tests (with `ratatui::TestBackend` and key-event integration tests).
- **Documentation**: All public APIs must be documented.

---

## 7. Architecture Overview

This is a secure, peer-to-peer messaging application for desktop, built with Rust and the `egui` graphical user interface library. It provides end-to-end encryption, forward secrecy, and file sharing capabilities, all without relying on a central server. For a detailed architectural deep-dive, refer to the [Architecture Document](docs/03_architecture.md).

### Key Technologies:

- **Language:** Rust
- **GUI:** `egui`
- **Async Runtime:** `tokio`
- **Cryptography:** `rsa`, `aes-gcm`, `x25519-dalek`, `hkdf`
- **Serialization:** `serde`, `serde_json`, `bincode`

### High-Level Architecture

The application is designed with a clear separation of concerns, following a layered architecture. This makes the codebase easier to understand, maintain, and test.

```text
┌─────────────────────────────────────┐
│   GUI Layer (egui/eframe)           │  ← Handles all user interaction and rendering.
└──────────────┬──────────────────────┘
               │ Shares state via Arc<Mutex<ChatManager>>
┌──────────────▼──────────────────────┐
│   Business Logic Layer (app)        │  ← Manages the application's core state and logic.
└──────────────┬──────────────────────┘
               │ Communicates with other layers via tokio channels
    ┌──────────┼──────────┬──────────┐
    │          │          │          │
┌───▼───┐  ┌──▼────┐  ┌──▼────┐  ┌──▼──────┐
│Network│  │Crypto │  │Transfer│ │Identity │  ← Core functionality layers.
│(TCP)  │  │(RSA/AES)│  │(Files) │ │(RSA Keys) │
└───────┘  └───────┘  └────────┘ └─────────┘
```
For a detailed breakdown of the directory structure and module responsibilities, refer to the [Architecture Document](docs/03_architecture.md).

---

## 8. Protocol Specification

### 8.1. Constants

These values are critical for compatibility:

```rust
const PORT_DEFAULT: u16 = 12345;
const MAX_PACKET_SIZE: usize = 8 * 1024 * 1024;  // 8 MiB
const FILE_CHUNK_SIZE: usize = 64 * 1024;         // 64 KiB
const AES_KEY_SIZE: usize = 32;                   // 256 bits
const AES_NONCE_SIZE: usize = 12;                 // 96 bits (GCM standard)
const RSA_KEY_BITS: usize = 2048;
const HANDSHAKE_TIMEOUT_SECS: u64 = 15;
```

### 8.2. Cryptography

- **RSA**: 2048 bits, OAEP with SHA-256 (RSA-OAEP-SHA256)
- **AES**: AES-256-GCM
- **Nonce AES**: 12 bytes, counter-based for guaranteed uniqueness. Format: `session_id(4 bytes) || counter(8 bytes)`. Prefixed to the ciphertext.
- **Fingerprint**: sha256_hex(pem_bytes) in lowercase hex.
- **Transport Format (Encrypted)**: `nonce(12) || ciphertext || tag(16)`

### 8.3. Network Protocol

#### TCP Framing (length-prefixed)

1. Calculate `payload: Vec<u8>`.
2. Check `payload.len() <= MAX_PACKET_SIZE`.
3. Send header: 4 bytes big-endian = `payload.len() as u32`.
4. Send `payload`.

#### Handshake (Protocol v3)

1. **Version Exchange**: Peers exchange `u32` (Big Endian) protocol version. Must be >= 3.
2. **Ephemeral Key Exchange (Plaintext)**:
   - Peers exchange 32-byte X25519 ephemeral public keys.
   - These keys are unique to the session.
3. **Session Key Derivation**:
   - `SharedSecret = ECDH(MyEphemeralPriv, PeerEphemeralPub)`
   - `SessionKey = HKDF-SHA256(SharedSecret)`
   - An encrypted tunnel (AES-256-GCM) is established immediately.
4. **Encrypted Identity Exchange**:
   - Peers exchange `IdentityProof` messages inside the encrypted tunnel.
   - `IdentityProof` contains:
     - `public_key_pem`: RSA Identity Key.
     - `signature`: `RSA_Sign(SHA256("IDENTITY_PROOF" || MyEphemeralPub))`
   - The signature binds the ephemeral key to the long-term identity, preventing MITM.
5. **Fingerprint Verification**:
   - The received RSA key fingerprint is checked against trusted contacts.
   - If unknown, the user is prompted to verify and trust the new identity (TOFU).

### 8.4. Message Format

```rust
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum ProtocolMessage {
    /// Protocol version announcement (first message)
    Version { version: u8 },

    /// Ephemeral X25519 public key for forward secrecy
    EphemeralKey { public_key: Vec<u8> },

    /// Text message (with sequence number for replay protection)
    Text { text: String, timestamp: u64, seq: u64 },

    /// File metadata (sent before chunks)
    FileMeta { filename: String, size: u64, seq: u64 },

    /// File data chunk
    FileChunk { chunk: Vec<u8>, seq: u64 },

    /// File transfer complete
    FileEnd { seq: u64 },

    /// Keep-alive ping
    Ping { seq: u64 },

    /// Typing indicator - user started typing
    TypingStart { seq: u64 },

    /// Typing indicator - user stopped typing
    TypingStop { seq: u64 },
}
```

### 8.5. Invite Links

Invite links are a convenient way to share contact information. They are base64-encoded JSON objects with the following structure:

```json
{
  "name": "Alice",
  "address": "192.168.1.10:12345", // Optional
  "fingerprint": "a1b2c3d4e5f6...",
  "public_key": "-----BEGIN PUBLIC KEY-----\n..."
}
```

The `address` field is optional. If it is not included, the recipient will need to manually enter the host and port when connecting.

---

## 9. Testing

- **Run all tests**: `cargo test`
- **Run specific test**: `cargo test test_aes_roundtrip -- --exact`
- **Code Formatting**: `cargo fmt`
- **Linter**: `cargo clippy`

### Invite Link Parsing Tests

The `chat_manager.rs` file contains a test module with several tests for parsing invite links. These tests cover various scenarios, including:

- Parsing a link with a placeholder address.
- Parsing a link with a valid address.
- Parsing a link with an invalid address (no port).
- Parsing a link with a bad port.

You can run these tests with `cargo test`.

---

## 10. Logging & Diagnostics

- Logging uses `tracing` with `tracing-subscriber`.
- Set `RUST_LOG="info,encodeur_rsa_rust=debug"` to increase verbosity.
- The GUI integrates logs via `egui_tracing` for in-app viewing.

---

## 11. Build Profiles

- `dev`: faster builds, debug assertions on.
- `release`: optimized with `lto = true` and `codegen-units = 1` (see `Cargo.toml`).

---

## 12. Developer Workflow

1. **Create a feature branch**: `git checkout -b feature/new-feature`
2. **Make changes**: Implement your feature or bug fix.
3. **Run tests**: `cargo test`
4. **Format code**: `cargo fmt`
5. **Lint code**: `cargo clippy`
6. **Commit changes**: `git commit -m "Brief description of changes"`
7. **Push changes**: `git push origin feature/new-feature`
8. **Create a pull request**: Open a pull request on GitHub to merge your feature branch into `main`.

---

## 13. Checklist: After a Bug Fix or New Feature

Before merging or releasing, ensure:

| Step | Action |
|------|--------|
| **Tests** | `cargo test`, `cargo clippy`, `cargo fmt` |
| **CHANGELOG** | Add entry under `[Unreleased]` in `CHANGELOG.md` |
| **Docs** | Update `README`, `DEVELOPER_GUIDE`, `docs/*`, or `DESIGN_NOTES` if behavior, API, protocol, or UI changes |
| **SECURITY** | If security-relevant: update `SECURITY.md` and add a security note in `CHANGELOG` |
| **Version** | On release: bump `Cargo.toml` and move `[Unreleased]` into the new version |
| **Packaging** | If installer/binary/icon changes: update `setup.iss` and `build-and-package.ps1` |

See [CONTRIBUTING.md](CONTRIBUTING.md#checklist-after-a-bug-fix-or-new-feature) for the full checklist.

---

## 14. Recent Changes

For a detailed history of changes, new features, and bug fixes, please refer to the [CHANGELOG.md](CHANGELOG.md) file.

---

## 15. Where to Go Next

- **Development Plan**: [DEVELOPMENT_PLAN.md](DEVELOPMENT_PLAN.md)
- **Architecture**: [docs/03_architecture.md](docs/03_architecture.md)
- **Protocol**: [docs/04_protocol.md](docs/04_protocol.md)
- **Security**: [SECURITY.md](SECURITY.md)
- **Contributing**: [CONTRIBUTING.md](CONTRIBUTING.md)
- **Design Notes**: [DESIGN_NOTES.md](DESIGN_NOTES.md)
