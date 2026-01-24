# Developer Guide

This guide provides all the technical information you need to understand, build, and contribute to the Encrypted P2P Messenger.

## 1. Project Overview

This is a secure, peer-to-peer messaging application for desktop, built with Rust and the `egui` graphical user interface library. It provides end-to-end encryption, forward secrecy, and file sharing capabilities, all without relying on a central server.

**Key Technologies:**

* **Language:** Rust
* **GUI:** `egui`
* **Async Runtime:** `tokio`
* **Cryptography:** `rsa`, `aes-gcm`, `x25519-dalek`, `hkdf`
* **Serialization:** `serde`, `serde_json`, `bincode`

## 2. Architecture

### 2.1. Directory Structure

```text
chat-p2p/
├── src/
│   ├── main.rs (GUI application entrypoint)
│   ├── lib.rs (Module exports and constants)
│   ├── types.rs (Core data structures)
│   ├── util.rs (Utility functions)
│   │
│   ├── app/ (Business Logic Layer)
│   │   ├── mod.rs
│   │   ├── chat_manager.rs (Core state management)
│   │   └── persistence.rs (JSON history save/load)
│   │
│   ├── core/ (Cryptography & Protocol)
│   │   ├── mod.rs
│   │   ├── crypto.rs (RSA, AES-GCM, X25519)
│   │   ├── framing.rs (Length-prefixed send/recv)
│   │   └── protocol.rs (Message types and serialization)
│   │
│   ├── gui/ (GUI Layer)
│   │   ├── mod.rs
│   │   ├── app_ui.rs (Main application state)
│   │   ├── chat_view.rs (Message display)
│   │   ├── dialogs.rs (All pop-up dialogs)
│   │   ├── help_view.rs (Help and about windows)
│   │   ├── sidebar.rs (Chat list)
│   │   ├── styling.rs (Theme and visuals)
│   │   └── widgets.rs (Custom UI components)
│   │
│   ├── network/ (Network Layer)
│   │   ├── mod.rs
│   │   ├── discovery.rs (mDNS Peer Discovery)
│   │   └── session.rs (TCP sessions and handshake)
│   │
│   ├── transfer/ (File Transfer Layer)
│   │   ├── mod.rs
│   │   ├── receiver.rs (Receiving files)
│   │   └── sender.rs (Sending files)
│   │
│   └── identity/ (Identity Layer)
│       └── mod.rs (Persistent user identity and RSA keys)
│
├── Cargo.toml
├── README.md
└── SECURITY.md
```

### 2.2. Layer Architecture

```text
┌─────────────────────────────────────┐
│   GUI Layer (egui/eframe)           │  ← User interaction
└──────────────┬──────────────────────┘
               │ Arc<Mutex<ChatManager>>
┌──────────────▼──────────────────────┐
│   Business Logic Layer              │  ← State management
└──────────────┬──────────────────────┘
               │ tokio channels
    ┌──────────┼──────────┬──────────┐
    │          │          │          │
┌───▼───┐  ┌──▼────┐  ┌──▼────┐  ┌──▼──────┐
│Network│  │Crypto │  │Transfer│ │Identity │
│Session│  │       │  │        │ │         │
│(TCP)  │  │RSA/AES│  │Files   │ │RSA Keys │
└───────┘  └───────┘  └────────┘ └─────────┘
```

### 2.3. Module Responsibilities

* **`src/main.rs` - GUI Application**: User interface and event handling.
* **`src/app/chat_manager.rs` - Business Logic**: Core state management, session management, and message routing.
* **`src/identity/mod.rs` - Identity System**: Persistent user identity, RSA key management.
* **`src/core/crypto.rs` - Cryptography**: All cryptographic operations (RSA, AES-GCM, X25519).
* **`src/core/protocol.rs` - Wire Protocol**: Message serialization and deserialization.
* **`src/network/session.rs` - Network Sessions**: TCP connection management and handshake logic.
* **`src/network/discovery.rs` - Peer Discovery**: Handles mDNS/Bonjour broadcasting and discovery of local peers.
* **`src/transfer/` - File Transfer**: Chunked file sending and receiving.
* **`src/types.rs` - Data Structures**: Shared types used throughout the application (`Chat`, `Message`, `Contact`).

#### Notable runtime events

* `SessionEvent::NewConnection(chat_id, peer_meta)`: emitted on the host when a client connects and presents a chat identifier. Used by `ChatManager` to create/sync chats across peers.

## 3. Protocol Specification

### 3.1. Constants

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

### 3.2. Cryptography

* **RSA**: 2048 bits, OAEP with SHA-256 (RSA-OAEP-SHA256)
* **AES**: AES-256-GCM
* **Nonce AES**: 12 bytes, counter-based for guaranteed uniqueness. Format: `session_id(4 bytes) || counter(8 bytes)`. Prefixed to the ciphertext.
* **Fingerprint**: sha256_hex(pem_bytes) in lowercase hex.
* **Transport Format (Encrypted)**: `nonce(12) || ciphertext || tag(16)`

### 3.3. Network Protocol

#### TCP Framing (length-prefixed)

1. Calculate `payload: Vec<u8>`.
2. Check `payload.len() <= MAX_PACKET_SIZE`.
3. Send header: 4 bytes big-endian = `payload.len() as u32`.
4. Send `payload`.

#### Handshake (Protocol v3)

1. **Version Exchange**: Peers exchange `u32` (Big Endian) protocol version. Must be >= 3.
2. **Ephemeral Key Exchange (Plaintext)**:
   * Peers exchange 32-byte X25519 ephemeral public keys.
   * These keys are unique to the session.
3. **Session Key Derivation**:
   * `SharedSecret = ECDH(MyEphemeralPriv, PeerEphemeralPub)`
   * `SessionKey = HKDF-SHA256(SharedSecret)`
   * An encrypted tunnel (AES-256-GCM) is established immediately.
4. **Encrypted Identity Exchange**:
   * Peers exchange `IdentityProof` messages inside the encrypted tunnel.
   * `IdentityProof` contains:
     * `public_key_pem`: RSA Identity Key.
     * `signature`: `RSA_Sign(SHA256("IDENTITY_PROOF" || MyEphemeralPub))`
   * The signature binds the ephemeral key to the long-term identity, preventing MITM.
5. **Fingerprint Verification**:
   * The received RSA key fingerprint is checked against trusted contacts.
   * If unknown, the user is prompted to verify and trust the new identity (TOFU).

### 3.4. Message Format

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

### 3.5. Invite Links

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

## 4. Building and Testing

### 4.1. Development Setup

1. **Install Rust**: Make sure you have the latest stable version of Rust installed from [rustup.rs](https://rustup.rs/).
2. **Clone the repository**: `git clone <repository-url>`
3. **Build the project**: `cargo build`

### 4.2. Build and Run Commands

* **Build**: `cargo build`
* **Build (Release)**: `cargo build --release`
* **Run (GUI)**: `cargo run --release`

> **Note**: Standalone CLI operation with `--host` and `--connect` is not implemented. Please use the GUI to host or connect.

### 4.3. Testing

* **Run all tests**: `cargo test`
* **Run specific test**: `cargo test test_aes_roundtrip -- --exact`
* **Code Formatting**: `cargo fmt`
* **Linter**: `cargo clippy`

#### Invite Link Parsing Tests

The `chat_manager.rs` file contains a test module with several tests for parsing invite links. These tests cover various scenarios, including:

* Parsing a link with a placeholder address.
* Parsing a link with a valid address.
* Parsing a link with an invalid address (no port).
* Parsing a link with a bad port.

You can run these tests with `cargo test`.

### 4.4. Logging & Diagnostics

* Logging uses `tracing` with `tracing-subscriber`.
* Set `RUST_LOG="info,encodeur_rsa_rust=debug"` to increase verbosity.
* The GUI integrates logs via `egui_tracing` for in-app viewing.

### 4.5. Build Profiles

* `dev`: faster builds, debug assertions on.
* `release`: optimized with `lto = true` and `codegen-units = 1` (see `Cargo.toml`).

### 4.6. Developer Workflow

1. **Create a feature branch**: `git checkout -b feature/new-feature`
2. **Make changes**: Implement your feature or bug fix.
3. **Run tests**: `cargo test`
4. **Format code**: `cargo fmt`
5. **Lint code**: `cargo clippy`
6. **Commit changes**: `git commit -m "Brief description of changes"`
7. **Push changes**: `git push origin feature/new-feature`
8. **Create a pull request**: Open a pull request on GitHub to merge your feature branch into `main`.

### 4.7. Checklist: After a Bug Fix or New Feature

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

## 5. Recent Changes

For a detailed history of changes, new features, and bug fixes, please refer to the [CHANGELOG.md](CHANGELOG.md) file.
