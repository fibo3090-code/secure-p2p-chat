# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Development Commands

```bash
# Build
cargo build                    # Debug build
cargo build --release          # Release build (LTO enabled, opt-level=3)

# Run
cargo run --release            # GUI (default)
cargo run --release -- --tui   # Terminal UI (ratatui)
cargo run --release -- --host --port 9000        # Host on port 9000
cargo run --release -- --connect 127.0.0.1:12345 # Connect to peer

# Quality & Testing
cargo test                     # Run all tests
cargo test <test_name> -- --exact  # Run specific test
cargo fmt                      # Format code
cargo clippy                   # Lint and check warnings

# Logging
RUST_LOG="info,encodeur_rsa_rust=debug" cargo run

# Windows packaging
./build-and-package.ps1        # PowerShell script for distributable builds
```

## Architecture Overview

### Layered Design

```
┌─────────────────────────────────────┐
│   GUI Layer (egui/eframe)           │  ← User interaction, App holds Arc<Mutex<ChatManager>>
│   OR TUI Layer (ratatui)            │
└──────────────┬──────────────────────┘
               │ Arc<Mutex<ChatManager>>
┌──────────────▼──────────────────────┐
│   Business Logic (app/)             │  ← ChatManager - central coordinator
│   └── chat_manager.rs               │     (chats, contacts, sessions, file transfers, toasts)
│   └── persistence.rs                │     (JSON save/load)
└──────────────┬──────────────────────┘
               │ tokio::sync::mpsc channels
    ┌──────────┼──────────┬──────────┐
    │          │          │          │
┌───▼───┐  ┌──▼────┐  ┌──▼────┐  ┌──▼──────┐
│Network│  │Crypto │  │Transfer│ │Identity │
│(TCP)  │  │(RSA/AES)│  │(Files) │ │(Keys)   │
└───────┘  └───────┘  └────────┘ └─────────┘
```

### Key Patterns

**ChatManager**: The single source of truth for all application state. GUI layers hold `Arc<Mutex<ChatManager>>`. All state changes (persisting, session mapping, chat operations) should go through ChatManager methods.

**Async Runtime**: `tokio` is used throughout. Background tasks are spawned with `tokio::spawn`. Inter-component communication uses `tokio::sync::mpsc` channels—many are intentionally `unbounded_channel()` for low-latency messaging.

**Session Events**: Network layer emits `SessionEvent` enum. GUI polls `ChatManager::poll_session_events()` for updates. Events include: `Listening`, `Connected`, `NewConnection`, `ShowFingerprintVerification`, `MessageReceived`, `Disconnected`, `Error`, `Warning`.

**Protocol v3 Handshake (ECDH-First)**:
1. Version exchange (plaintext, u32)
2. X25519 ephemeral key exchange (plaintext) - provides forward secrecy
3. Session key derivation via HKDF-SHA256 from shared secret
4. Encrypted tunnel established with AES-256-GCM
5. Identity exchange inside tunnel (`IdentityProof` with signature binding ephemeral key to identity)
6. Fingerprint verification (TOFU - Trust On First Use)

**Message Framing**: Binary-tagged ASCII format `[type: u8][payload...]`. CRITICAL: `to_plain_bytes()` and `from_plain_bytes()` in `src/core/protocol.rs` must remain symmetric—changing one side without the other breaks protocol compatibility.

## Security-Sensitive Areas

⚠️ **The following files require extra scrutiny**:
- `src/network/session.rs` - Protocol v3 handshake, message loop, replay protection
- `src/core/crypto.rs` - Key derivation, AEAD operations, signing
- `src/identity/mod.rs` - Private key storage (encrypted with Argon2 + ChaCha20-Poly1305)

**Critical Security Constraints**:
- Never change only one side of `to_plain_bytes`/`from_plain_bytes`—they must stay symmetric
- Session keys bind the full handshake transcript via HKDF salt/info—modify key derivation only if you understand the implications
- Replay protection uses per-session sequence numbers—file transfer packets share this namespace (`last_recv_seq` in `Session`)
- Identity storage always uses `zeroize` behavior—plaintext private keys must never be persisted
- The app blocks all UI functionality until password unlock/set-password completes

## File Transfer System

File transfers use chunked transmission (`FILE_CHUNK_SIZE = 64 KiB`). Progress is tracked via `FileTransferState`. Transfer packets share the per-chat monotonic sequence namespace with standard messages, so replay protection covers both message types.

**Key Files**:
- `src/transfer/sender.rs` - File sending logic
- `src/transfer/receiver.rs` - File receiving logic
- `src/core/protocol.rs` - `FILE_CHUNK` message encoding/decoding

## TOFU (Trust-On-First-Use) Flow

Fingerprint verification is handled in `ChatManager::handle_session_event`. When a new peer connects with an unknown fingerprint:
1. `SessionEvent::ShowFingerprintVerification` is emitted
2. GUI displays verification dialog with 64-character fingerprint or colored grid
3. User must verify via separate secure channel (phone, in-person)
4. `ChatManager::confirm_fingerprint()` persists trust state on accept

## Common Entry Points

| Task | Start Here |
|------|------------|
| Feature work | `src/app/chat_manager.rs` |
| Protocol changes | `src/network/session.rs`, `src/core/crypto.rs`, `src/core/protocol.rs` |
| UI changes | `src/gui/` (egui) or `src/tui/` (ratatui) |
| Identity/persistence | `src/identity/mod.rs`, `src/app/persistence.rs` |

## Important Constants

Defined in `src/lib.rs`:
- `PORT_DEFAULT: u16 = 12345`
- `MAX_PACKET_SIZE: usize = 8 MiB`
- `FILE_CHUNK_SIZE: usize = 64 KiB`
- `AES_KEY_SIZE: usize = 32`
- `AES_NONCE_SIZE: usize = 12`
- `HANDSHAKE_TIMEOUT_SECS: u64 = 15`
- `MAX_FILE_SIZE: u64 = 10 GiB`

## Testing

- Unit tests live in source files (e.g., `src/network/session.rs` contains handshake tests)
- Integration tests go in `tests/` directory
- Async tests use `#[tokio::test]`
- Handshake tests must verify derived keys match on both sides
- Protocol changes require new serialization/deserialization tests