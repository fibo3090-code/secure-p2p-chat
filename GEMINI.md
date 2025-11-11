# GEMINI.md

## Project Overview

This is a secure, peer-to-peer messaging application for desktop, built with Rust and the `egui` graphical user interface library. It provides end-to-end encryption, forward secrecy, and file sharing capabilities, all without relying on a central server.

**Key Technologies:**

*   **Language:** Rust
*   **GUI:** `egui`
*   **Async Runtime:** `tokio`
*   **Cryptography:** `rsa`, `aes-gcm`, `x25519-dalek`, `hkdf`
*   **Serialization:** `serde`, `serde_json`, `bincode`

---

# 🏗️ Architecture & Code Organization

**Version**: 1.3.0-dev
**Last Updated**: 2025-11-02

This document explains how the codebase is organized and how all components work together.

---

## 📁 Directory Structure

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

---

## 🎯 Layer Architecture

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

---

## 📦 Module Responsibilities

### `src/main.rs` - GUI Application
- **Role**: User interface and event handling.
- **Key Structures**: `App` (main application state).

### `src/app/chat_manager.rs` - Business Logic
- **Role**: Core state management, session management, and message routing.
- **Key Structure**: `ChatManager`.

### `src/identity/mod.rs` - Identity System
- **Role**: Persistent user identity, RSA key management.
- **Key Structure**: `Identity`.

### `src/core/crypto.rs` - Cryptography
- **Role**: All cryptographic operations (RSA, AES-GCM, X25519).

### `src/core/protocol.rs` - Wire Protocol
- **Role**: Message serialization and deserialization.
- **Key Enum**: `ProtocolMessage`.

### `src/network/session.rs` - Network Sessions
- **Role**: TCP connection management and handshake logic.

### `src/transfer/` - File Transfer
- **Role**: Chunked file sending and receiving.

### `src/types.rs` - Data Structures
- **Role**: Shared types used throughout the application (`Chat`, `Message`, `Contact`).

---

## 🔧 Common Modifications

### Adding a New Message Type
1.  Update `ProtocolMessage` enum in `src/core/protocol.rs`.
2.  Handle the new message in the `session.rs` receive loop.
3.  Add UI elements in `src/main.rs` if needed.

### Adding a New UI Dialog
1.  Add state fields to the `App` struct in `src/main.rs`.
2.  Add logic to show/hide the dialog.
3.  Render the dialog in the `App::update()` method.

### Changing Crypto
1.  Update functions in `src/core/crypto.rs`.
2.  Update the handshake logic in `src/network/session.rs`.
3.  Add corresponding tests.
4.  Update `docs/developer_guide/security.md`.

# Building and Testing

This guide provides instructions on how to build, run, and test the application.

## Development Setup

1.  **Install Rust**: Make sure you have the latest stable version of Rust installed. You can get it from [rustup.rs](https://rustup.rs/).
2.  **Clone the repository**: `git clone <repository-url>`
3.  **Build the project**: `cargo build`

## Build and Run Commands

### Development

```bash
# Build the project
cargo build

# Build optimized release version
cargo build --release

# Run in GUI mode (default)
cargo run --release

# Run in CLI host mode
cargo run --release -- --host --port 12345

# Run in CLI client mode
cargo run --release -- --connect 192.168.1.10:12345
```

### Debugging

```bash
# Run with detailed logs
RUST_LOG=debug cargo run

# Run with backtrace on panic
RUST_BACKTRACE=1 cargo run
```

## Testing

All code changes should be accompanied by tests.

```bash
# Run all tests
cargo test

# Run specific module tests
cargo test crypto::tests

# Run tests with output
cargo test -- --nocapture

# Run specific test
cargo test test_aes_roundtrip -- --exact
```

### Code Quality

```bash
# Format code
cargo fmt

# Run clippy linter
cargo clippy

# Generate and open documentation
cargo doc --open
```

# Spécification du Protocole

Ce document contient toutes les informations nécessaires pour qu'un développeur puisse recoder l'application de zéro avec compatibilité parfaite au niveau réseau et comportement identique.

## 1. Constantes & Contrats Critiques

Ces valeurs sont non-négociables pour assurer la compatibilité :

```rust
const PORT_DEFAULT: u16 = 12345;
const MAX_PACKET_SIZE: usize = 8 * 1024 * 1024;  // 8 MiB
const FILE_CHUNK_SIZE: usize = 64 * 1024;         // 64 KiB
const AES_KEY_SIZE: usize = 32;                   // 256 bits
const AES_NONCE_SIZE: usize = 12;                 // 96 bits (GCM standard)
const RSA_KEY_BITS: usize = 2048;
const HANDSHAKE_TIMEOUT_SECS: u64 = 15;
```

### Préfixes du Protocole (ASCII exacts)

-   Message texte : `TEXT:` + utf8_string
-   Métadonnée fichier : `FILE_META|<filename>|<size>`
-   Chunk fichier : `FILE_CHUNK:` + raw bytes (binaire)
-   Fin fichier : `FILE_END:`
-   Ping (optionnel) : `PING`

### Cryptographie

-   **RSA**: 2048 bits, OAEP avec SHA-256 (RSA-OAEP-SHA256)
-   **AES**: AES-256-GCM
-   **Nonce AES**: 12 bytes, généré aléatoirement pour chaque message, préfixé au ciphertext
-   **Fingerprint**: sha256_hex(pem_bytes) en lowercase hex
-   **Format de transport chiffré**: nonce(12) || ciphertext || tag(16)

## 2. Protocole Réseau Détaillé

### Framing TCP (length-prefixed)

Pour tout envoi :

1.  Calculer `payload: Vec<u8>`.
2.  Vérifier `payload.len() <= MAX_PACKET_SIZE`.
3.  Envoyer header : 4 bytes big-endian = `payload.len() as u32`.
4.  Envoyer `payload`.

### Handshake (Séquence Déterministe)

1.  **Connexion TCP**.
2.  **Host → Client**: Clé publique RSA (PEM).
3.  **Client → Host**: Clé publique RSA (PEM).
4.  **Host**: Génère une clé de session AES-256.
5.  **Host → Client**: Clé AES chiffrée avec la clé publique du client.
6.  Les deux pairs utilisent maintenant la clé AES pour la communication.

### Format des Messages (après déchiffrement)

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

# Security Policy and Implementation

**Current Version**: 1.3.0-dev
**Protocol Version**: 2

## Security Overview

Encrypted P2P Messenger implements **military-grade end-to-end encryption** with **forward secrecy**, matching the security standards of Signal and WhatsApp.

### Threat Model

This application protects against:
- ✅ **Eavesdropping**: All messages encrypted end-to-end.
- ✅ **Tampering**: GCM authentication tags detect modifications.
- ✅ **Replay Attacks**: Random nonces prevent message replay.
- ✅ **Key Compromise**: Forward secrecy protects past sessions.
- ✅ **Downgrade Attacks**: Protocol version negotiation.

### Assumptions

This security model assumes:
- Users **verify fingerprints** on first connection.
- The network is **trusted** (LAN or secure VPN).
- The operating system is **not compromised**.

## Cryptographic Specifications

### Encryption Primitives

- **Message Encryption**: AES-256-GCM
- **Key Exchange**: X25519 ECDH
- **Identity**: RSA-2048-OAEP
- **Fingerprinting**: SHA-256

### Forward Secrecy Implementation

Forward secrecy is achieved using the X25519 Elliptic Curve Diffie-Hellman (ECDH) key exchange.

- **Ephemeral Keys**: For each session, new X25519 keys are generated and then discarded after use.
- **Key Derivation**: The shared secret from ECDH is used to derive a unique 32-byte AES-256 session key via HKDF-SHA256.
- **Identity vs. Encryption**: Long-term RSA keys are used only for identity verification (fingerprints), not for session encryption. This ensures that a compromise of the long-term keys does not compromise past session keys.

### Handshake Sequence (Protocol v2)

1.  **Version Negotiation**: Both peers exchange and verify the protocol version.
2.  **RSA Public Key Exchange**: For identity and fingerprint verification.
3.  **X25519 Ephemeral Key Exchange**: For forward secrecy.
4.  **ECDH Computation**: A shared secret is computed.
5.  **HKDF-SHA256 Key Derivation**: The final AES session key is derived.
6.  **Encrypted Communication**: All further communication is encrypted with the session key.

## Reporting Security Issues

If you discover a security vulnerability, please **DO NOT** open a public GitHub issue. Instead, email `security@example.com` (replace with a real address) with a detailed description of the vulnerability.

### Contribution Guidelines

Refer to `CONTRIBUTING.md` for details on reporting bugs, suggesting features, and the pull request process.

## Documentation

- **[User Guide](user_guide.md)**: Instructions on how to install and use the application.
- **[Developer Guide](dev_guide.md)**: Technical details for developers and contributors.
- **[Roadmap](ROADMAP.md)**: The development roadmap and future plans.