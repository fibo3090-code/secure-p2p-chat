# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a peer-to-peer encrypted messaging application with file transfer capabilities, built in Rust. It implements end-to-end encryption using RSA-2048-OAEP for key exchange and AES-256-GCM for message encryption, with a modern desktop GUI built on egui/eframe.

## Build and Test Commands

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

### Testing
```bash
# Run all tests
cargo test

# Run specific module tests
cargo test crypto::tests
cargo test framing::tests
cargo test protocol::tests

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

# Run clippy with all warnings as errors
cargo clippy --all-features -- -D warnings

# Generate and open documentation
cargo doc --open
```

### Debugging
```bash
# Run with detailed logs
RUST_LOG=debug cargo run

# Run with backtrace on panic
RUST_BACKTRACE=1 cargo run

# Run specific test with logs
RUST_LOG=debug cargo test test_name -- --nocapture
```

## Architecture Overview

### Module Structure

```
src/
├── lib.rs              # Public API, constants (PORT_DEFAULT, MAX_PACKET_SIZE, etc.)
├── main.rs             # Entry point (CLI + GUI initialization)
├── core/               # Cryptographic primitives and protocol
│   ├── crypto.rs       # RSA-2048-OAEP, AES-256-GCM, SHA-256 fingerprints
│   ├── framing.rs      # Length-prefixed TCP packets (4-byte BE header)
│   └── protocol.rs     # Message types and wire format parsing
├── network/            # Network session management
│   └── session.rs      # Host/client handshake and message loop
├── transfer/           # File transfer system
│   ├── sender.rs       # Chunked file sending (64 KiB chunks)
│   └── receiver.rs     # Streaming file reception with temp files
├── app/                # Business logic
│   ├── chat_manager.rs # Session lifecycle, message history, toasts
│   └── persistence.rs  # JSON-based history storage
├── types.rs            # Shared type definitions
└── util.rs             # Helper functions (formatting, sanitization)
```

### Critical Constants (DO NOT MODIFY - Wire Protocol Compatibility)

These values in `src/lib.rs` ensure compatibility with the protocol specification:

- `PORT_DEFAULT = 12345` - Default TCP port
- `MAX_PACKET_SIZE = 8 MiB` - Maximum packet size for DoS protection
- `FILE_CHUNK_SIZE = 64 KiB` - File transfer chunk size
- `AES_KEY_SIZE = 32` (256 bits)
- `AES_NONCE_SIZE = 12` (96 bits for GCM)
- `RSA_KEY_BITS = 2048`
- `HANDSHAKE_TIMEOUT_SECS = 15`

### Protocol Wire Format

All messages use length-prefixed framing:
1. 4-byte big-endian header containing payload length
2. Payload bytes (encrypted after handshake)

After decryption, messages use ASCII prefixes:
- `TEXT:<content>` - Text message
- `FILE_META|<filename>|<size>` - File metadata
- `FILE_CHUNK:<binary_data>` - File chunk
- `FILE_END:` - Transfer complete
- `PING` - Keep-alive (optional)

### Handshake Sequence (Symmetric Deterministic)

**Critical:** The host always generates and distributes the AES session key:

1. TCP connection established
2. Host → Client: RSA public key (PEM format)
3. Client → Host: RSA public key (PEM format)
4. Host: Generates random AES-256 key
5. Host → Client: AES key encrypted with client's RSA public key
6. Both peers: Switch to AES-GCM encrypted communication

### Cryptography Implementation

**RSA Operations** (`core/crypto.rs`):
- Key generation uses `tokio::task::spawn_blocking` to avoid blocking the GUI (200-500ms operation)
- OAEP padding with SHA-256 for chosen-ciphertext attack resistance
- PEM import/export for key persistence
- Fingerprints: SHA-256 hash of PEM bytes, displayed as 64-char hex string

**AES-GCM Operations** (`core/crypto.rs`):
- Each message gets a unique 12-byte random nonce (CSPRNG via `OsRng`)
- Wire format: `nonce(12 bytes) || ciphertext || tag(16 bytes)`
- Tag provides authentication and tamper detection
- Decryption returns `None` if authentication fails

**Security Considerations**:
- All crypto operations use `OsRng` for CSPRNG
- Failed decryption indicates potential tampering - log and reject
- Never log private keys, plaintext messages, or session keys
- Fingerprints MUST be displayed to user for manual verification

### File Transfer System

**Sender** (`transfer/sender.rs`):
- Opens file with `tokio::fs::File` for async I/O
- Reads in 64 KiB chunks to minimize memory usage
- Sequence: `FileMeta` → multiple `FileChunk` → `FileEnd`
- Progress callbacks for UI updates

**Receiver** (`transfer/receiver.rs`):
- Writes to temporary file during transfer (`tmp_<uuid>_<filename>`)
- Atomically renames to final destination on completion
- Filename sanitization removes path separators: `/, \, :, *, ?, ", <, >, |`
- Validates received size matches expected size
- Cleanup on error: removes temporary file

**Why Streaming?**
Files can be >1 GB. Loading entire file into memory causes OOM. Chunked streaming keeps memory usage constant.

### Session Management

**ChatManager** (`app/chat_manager.rs`):
- Manages multiple active sessions (HashMap by UUID)
- Each session has its own tokio channels for async communication
- Toast notification system for user feedback
- Auto-save history every 30 seconds (if implemented)

**Session Events**:
- `Listening`, `Connected`, `FingerprintReceived`, `Ready`, `Disconnected`
- Events flow from network tasks to ChatManager via unbounded channels
- GUI polls ChatManager state (try_lock to avoid blocking)

### GUI Architecture

**Thread Model**:
- Main thread runs egui event loop
- Network/crypto operations spawn tokio tasks
- ChatManager wrapped in `Arc<Mutex<>>` for shared access
- Use `try_lock()` in GUI update loop to avoid blocking

**Key UI Components**:
- Top menu bar: Connection options
- Left sidebar: Chat list with selection
- Central panel: Messages with left/right alignment (from_me boolean)
- Toast overlay: Notifications with auto-expiry (4 seconds)
- Dialogs: Host/Connect with port configuration

**Message Display**:
- Timestamps formatted as HH:MM if today, "Yesterday HH:MM" if yesterday, full date otherwise
- File sizes formatted with KB/MB/GB units
- Fingerprints shown as first 8 + "..." + last 8 chars, with copy button

## Common Development Tasks

### Adding a New Message Type

1. Add variant to `ProtocolMessage` enum in `core/protocol.rs`
2. Implement serialization in `to_plain_bytes()` with ASCII prefix
3. Implement deserialization in `from_plain_bytes()`
4. Add test cases for roundtrip
5. Update handler in `session.rs` message loop
6. Update `MessageContent` enum in `types.rs` if needed for GUI

### Adding Crypto Features

**IMPORTANT**: Crypto changes affect wire compatibility. Coordinate with specification.

- RSA operations: Use `spawn_blocking` for key generation
- New symmetric cipher: Ensure authentication (AEAD modes only)
- Random generation: Always use `OsRng`, verify success
- Add tests for: encryption/decryption, tampering detection, key serialization

### Testing File Transfer

Create test file:
```bash
# Create 100 MB test file
dd if=/dev/urandom of=test_100mb.bin bs=1M count=100
```

Test scenarios:
- Small files (<1 MB)
- Large files (>100 MB)
- File with special characters in name
- Transfer interruption (kill connection mid-transfer)
- Disk full scenario
- Path traversal attempts (`../../../etc/passwd`)

### Debugging Network Issues

Enable detailed logging:
```bash
RUST_LOG=encodeur_rsa_rust=trace,tokio=debug cargo run
```

Check handshake:
- Verify both peers exchange public keys successfully
- Confirm AES key decryption succeeds (check key length = 32)
- Look for "decryption failed" messages (indicates tampering or key mismatch)

Common issues:
- Firewall blocking port 12345
- Fingerprint mismatch (indicates MITM or version mismatch)
- Packet size exceeded (check MAX_PACKET_SIZE)
- RSA key generation timeout (increase timeout or use cached keys)

## Known Limitations and Future Work

**No Forward Secrecy**: Session keys derived from long-term RSA keys
- Future: Implement X25519 ECDH ephemeral key exchange

**No Persistent Identity**: Keys generated per session
- Future: Encrypt private keys with Argon2-derived passphrase

**LAN Only**: No NAT traversal or relay servers
- Future: STUN/TURN for WAN connectivity

**Trust-on-First-Use (TOFU)**: No certificate authority
- Future: Add digital signatures with Ed25519 for authentication

## Testing Strategy

**Unit Tests**: Each module (`crypto`, `framing`, `protocol`) has inline tests
- Test both success and failure cases
- Test boundary conditions (empty, max size, invalid)
- Use `tokio::io::duplex` for mocking TcpStream

**Integration Tests**: Located in `tests/` directory
- Full handshake flow (host + client)
- File transfer end-to-end
- Error recovery scenarios

**Manual Testing Checklist**:
- [ ] Host/client connection establishment
- [ ] Fingerprint display and verification
- [ ] Text message send/receive
- [ ] File transfer (small and large)
- [ ] Connection interruption handling
- [ ] Multiple simultaneous chats
- [ ] History persistence (restart app)
- [ ] Toast notifications appearance
- [ ] Copy fingerprint to clipboard

## Project-Specific Conventions

- Async functions: Use `tokio::spawn` for independent tasks
- Error handling: `anyhow::Result` for application errors, `thiserror` for library errors
- Logging: `tracing::info` for normal ops, `tracing::warn` for security events, `tracing::error` for failures
- File paths: Always use absolute paths, validate before use
- Timestamps: Store as UTC (`chrono::Utc`), display as local time
- UUIDs: Use v4 for all generated IDs (sessions, messages, chats)

## Security Checklist for Changes

When modifying crypto/network code:
- [ ] Validates all input sizes (prevent DoS)
- [ ] Uses CSPRNG for all random generation
- [ ] Logs security events (failed auth, tampering)
- [ ] Never logs sensitive data (keys, plaintexts)
- [ ] Sanitizes file paths (prevent directory traversal)
- [ ] Uses authenticated encryption (AEAD modes)
- [ ] Handles errors securely (no info leakage)
- [ ] Tests tampering detection (flip bits, verify rejection)

## Useful References

- **Specification**: See `PROTOCOL_SPEC.md` (French) for complete protocol details
- **Implementation Status**: See `IMPLEMENTATION_STATUS.md` for current progress
- **README**: User-facing documentation for installation and usage
- **RustCrypto**: https://github.com/RustCrypto - Cryptography primitives used
- **egui**: https://github.com/emilk/egui - Immediate mode GUI framework
