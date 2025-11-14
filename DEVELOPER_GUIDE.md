# Developer Guide

This guide provides all the technical information you need to understand, build, and contribute to the Encrypted P2P Messenger.

## 1. Project Overview

This is a secure, peer-to-peer messaging application for desktop, built with Rust and the `egui` graphical user interface library. It provides end-to-end encryption, forward secrecy, and file sharing capabilities, all without relying on a central server.

**Key Technologies:**

*   **Language:** Rust
*   **GUI:** `egui`
*   **Async Runtime:** `tokio`
*   **Cryptography:** `rsa`, `aes-gcm`, `x25519-dalek`, `hkdf`
*   **Serialization:** `serde`, `serde_json`, `bincode`

## 2. Architecture

### 2.1. Directory Structure

```
chat-p2p/
├── src/
│   ├── main.rs (GUI application)
│   ├── lib.rs (Module exports)
│   ├── types.rs (Data structures)
│   ├── util.rs (Helpers)
│   │
│   ├── app/ (Business Logic Layer)
│   │   ├── chat_manager.rs (Core state management)
│   │   └── persistence.rs (JSON save/load)
│   │
│   ├── core/ (Cryptography Layer)
│   │   ├── crypto.rs (RSA, AES-GCM, X25519)
│   │   └── protocol.rs (Message types)
│   │
│   ├── network/ (Network Layer)
│   │   └── session.rs (TCP sessions, handshake)
│   │
│   ├── transfer/ (File Transfer Layer)
│   │   └── file_transfer.rs (Chunked files)
│   │
│   └── identity/ (Identity Layer)
│       └── mod.rs (Persistent RSA keys)
│
├── docs/
├── Cargo.toml
├── README.md
└── SECURITY.md
```

### 2.2. Layer Architecture

```
┌─────────────────────────────────────┐
│   GUI Layer (egui/eframe)          │  ← User interaction
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

*   **`src/main.rs` - GUI Application**: User interface and event handling.
*   **`src/app/chat_manager.rs` - Business Logic**: Core state management, session management, and message routing.
*   **`src/identity/mod.rs` - Identity System**: Persistent user identity, RSA key management.
*   **`src/core/crypto.rs` - Cryptography**: All cryptographic operations (RSA, AES-GCM, X25519).
*   **`src/core/protocol.rs` - Wire Protocol**: Message serialization and deserialization.
*   **`src/network/session.rs` - Network Sessions**: TCP connection management and handshake logic.
*   **`src/transfer/` - File Transfer**: Chunked file sending and receiving.
*   **`src/types.rs` - Data Structures**: Shared types used throughout the application (`Chat`, `Message`, `Contact`).

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

*   **RSA**: 2048 bits, OAEP with SHA-256 (RSA-OAEP-SHA256)
*   **AES**: AES-256-GCM
*   **Nonce AES**: 12 bytes, generated randomly for each message, prefixed to the ciphertext.
*   **Fingerprint**: sha256_hex(pem_bytes) in lowercase hex.
*   **Transport Format (Encrypted)**: `nonce(12) || ciphertext || tag(16)`

### 3.3. Network Protocol

#### TCP Framing (length-prefixed)

1.  Calculate `payload: Vec<u8>`.
2.  Check `payload.len() <= MAX_PACKET_SIZE`.
3.  Send header: 4 bytes big-endian = `payload.len() as u32`.
4.  Send `payload`.

#### Handshake (Protocol v2)

1.  **Version Negotiation**: Both peers exchange and verify the protocol version.
2.  **RSA Public Key Exchange**: For identity and fingerprint verification.
3.  **X25519 Ephemeral Key Exchange**: For forward secrecy.
4.  **ECDH Computation**: A shared secret is computed.
5.  **HKDF-SHA256 Key Derivation**: The final AES session key is derived.
6.  **Encrypted Communication**: All further communication is encrypted with the session key.
7.  **Chat ID Exchange**: The client sends the `chat_id` to the host to synchronize the chat session.

### 3.4. Message Format

```rust
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ProtocolMessage {
    Text {
        text: String,
        timestamp: u64
    },
    FileMeta {
        filename: String,
        size: u64
    },
    FileChunk {
        chunk: Vec<u8>,
        seq: u64
    },
    FileEnd,
    Ping,
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

1.  **Install Rust**: Make sure you have the latest stable version of Rust installed from [rustup.rs](https://rustup.rs/).
2.  **Clone the repository**: `git clone <repository-url>`
3.  **Build the project**: `cargo build`

### 4.2. Build and Run Commands

*   **Build**: `cargo build`
*   **Build (Release)**: `cargo build --release`
*   **Run (GUI)**: `cargo run --release`
*   **Run (CLI Host)**: `cargo run --release -- --host --port 12345`
*   **Run (CLI Client)**: `cargo run --release -- --connect 192.168.1.10:12345`

### 4.3. Testing

*   **Run all tests**: `cargo test`
*   **Run specific test**: `cargo test test_aes_roundtrip -- --exact`
*   **Code Formatting**: `cargo fmt`
*   **Linter**: `cargo clippy`

#### Invite Link Parsing Tests

The `chat_manager.rs` file contains a test module with several tests for parsing invite links. These tests cover various scenarios, including:
*   Parsing a link with a placeholder address.
*   Parsing a link with a valid address.
*   Parsing a link with an invalid address (no port).
*   Parsing a link with a bad port.

You can run these tests with `cargo test`.

## 5. Recent Changes & Bug Fixes

### 5.1. Version 1.3.0 - Chat Creation & Network Synchronization Fix

*   **Issue**: When creating a new chat from the contacts list, the chat was created locally but not propagated to the peer instance. Attempting to send messages resulted in a "Message sent locally but all recipients offline" error.
*   **Root Cause**: The chat creation flow was entirely local. The application created a `Chat` object locally but never initiated a network connection with the peer.
*   **Solution**:
    1.  **Enhanced Network Protocol**: Added `SessionEvent::NewConnection` to notify the application layer of new connections, including the `chat_id`.
    2.  **Modified Session Handshake**: The client now sends its `chat_id` to the host after the RSA public key exchange.
    3.  **Updated Connection Flow**: The `connect_to_host()` function now accepts an optional `existing_chat_id` to synchronize the chat ID between peers.
    4.  **Improved UI Flow**: When "Open chat" is clicked, a local `Chat` is created immediately for UI responsiveness, and the network connection is established in the background. The "Share My Link" tab now uses `app.identity.generate_invite_link(None)` to generate the invite link.

### 5.2. History Not Persisting After Installation

*   **Issue**: Conversations and contacts disappeared when the app was closed and reopened, but only when installed via an installer.
*   **Root Cause**: Using a relative file path (`Downloads/history.json`) for history storage, which caused issues with permissions and inconsistent paths.
*   **Solution**: Used the `directories` crate to store the history file in the platform-specific user data directory (`%APPDATA%` on Windows, `~/.local/share` on Linux).

### 5.3. Invite Link Same for Everyone

*   **Issue**: The generated invite link was a placeholder and the same for all users.
*   **Root Cause**: No persistent identity system to generate unique user information.
*   **Solution**: Implemented a persistent `Identity` module (`src/identity/mod.rs`) that generates and saves a unique RSA key pair and fingerprint for each user. The invite link is now generated from this unique identity.

### 5.4. Group Chat Messages Disappearing

*   **Issue**: Messages sent to group chats would not appear in the history if all participants were offline.
*   **Root Cause**: The message was only added to the history *after* a successful send attempt. If no one was online, the message was never saved.
*   **Solution**: The message is now added to the history *before* attempting to send it. A warning is displayed to the user if some or all of the recipients are offline.
