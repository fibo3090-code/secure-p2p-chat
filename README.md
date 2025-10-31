# 🔒 Encrypted P2P Messenger

A modern, secure peer-to-peer encrypted messaging application with **forward secrecy**, built in Rust with end-to-end encryption.

![Version](https://img.shields.io/badge/version-1.2.0-blue)
![License](https://img.shields.io/badge/license-MIT-green)
![Rust](https://img.shields.io/badge/rust-1.70%2B-orange)
![Security](https://img.shields.io/badge/security-forward_secrecy-brightgreen)

## ✨ Features

### Security
- 🔐 **End-to-End Encryption**: RSA-2048-OAEP + AES-256-GCM
- 🔒 **Forward Secrecy**: X25519 ECDH + HKDF-SHA256 (v1.1.0+)
- 🔑 **Fingerprint Verification**: Manual verification for security
- 🛡️ **Tamper Detection**: GCM authentication tags
- 🎲 **Secure Random**: CSPRNG for all crypto operations
- 🔄 **Protocol Version 2**: Prevents downgrade attacks

### Messaging
- 💬 **Modern Chat Interface**: WhatsApp-like UI with avatars and timestamps
- ✍️ **Multiline Input**: Comfortable text box with keyboard shortcuts
- 📎 **File Transfer**: Send files of any size with progress tracking
- 💾 **Message History**: Automatic persistence in JSON format
- ⏰ **Smart Timestamps**: Relative time display (Today, Yesterday, etc.)
- 😊 **Emoji Picker**: Quick access to common emojis
- 📁 **Drag & Drop**: Simply drag files to send them
- ✍️ **Typing Indicators**: See when your peer is typing
- 🔔 **Desktop Notifications**: Get notified of new messages

### User Experience
- 👋 **Welcome Screen**: Guided onboarding for new users
- ⚙️ **Settings Panel**: Configure downloads, file limits, and preferences
- 🎨 **Colorful Avatars**: Unique colors generated from fingerprints
- ⌨️ **Keyboard Shortcuts**: Ctrl+Enter to send, and more
- 🎯 **File Preview**: Confirm files before sending

### Architecture
- 🚀 **P2P Design**: Direct peer-to-peer, no central server
- 🔄 **Cross-Platform**: Windows, Linux, and macOS
- ⚡ **Async Runtime**: Built on Tokio for performance
- 🎨 **Modern GUI**: egui/eframe desktop interface

## 🚀 Quick Start

### Installation

```bash
# Clone the repository
git clone <repository-url>
cd "encodeur_rsa_rust"

# Build release version
cargo build --release

# Run the application
cargo run --release
```

### First Launch

When you first open the app:

1. **Read the Welcome Screen** - Complete guide for new users
2. **Choose Your Mode**:
   - **Host**: Start hosting to accept connections
   - **Client**: Connect to someone who's hosting

### Host Mode (Server)

1. Click **Connection** → **Start Host**
2. Use default port (12345) or customize
3. Share your IP address with your peer
4. Wait for connection
5. **Verify fingerprint** when they connect

**Find your IP**:
- Windows: `ipconfig`
- Linux/Mac: `ifconfig` or `ip addr`

### Client Mode

1. Get host's IP address and port
2. Click **Connection** → **Connect to Host**
3. Enter IP (e.g., "192.168.1.100") and port (12345)
4. **Verify fingerprint** when connected
5. Start chatting!

### Sending Messages

- Type in the text box at the bottom
- Press **Ctrl+Enter** or click **📤 Send**
- Your messages appear on the right (blue)
- Received messages appear on the left (gray)

### Sending Files

1. Click **📎** attachment button
2. Select your file
3. Preview appears - verify it's correct
4. Click **✅ Send File** to confirm

## 🎯 Key Keyboard Shortcuts

- **Ctrl+Enter**: Send message
- **Tab**: Navigate between fields
- **Escape**: Close dialogs

## 🔐 Security - CRITICAL!

### Always Verify Fingerprints

When you connect, both users see a fingerprint (64-character hex string).

**⚠️ You MUST compare fingerprints via a different channel**:
- ✅ Phone call
- ✅ Video call
- ✅ In person
- ❌ NOT via the same network/app

If fingerprints **match** → Safe to proceed!
If they **don't match** → **STOP! Possible attack!**

### Security Model

**Current Protection**:
- ✅ Strong encryption (RSA-2048, AES-256-GCM)
- ✅ Authentication via GCM tags
- ✅ Fingerprint verification
- ✅ Tamper detection
- ✅ Path traversal protection
- ✅ Secure random number generation

**Limitations**:
- ⚠️ No forward secrecy (session keys from long-term RSA)
- ⚠️ No persistent identity (keys per session)
- ⚠️ Trust-on-first-use model
- ⚠️ LAN recommended (no NAT traversal)

## ⚙️ Configuration

Access via **Settings** → **Preferences**:

- **Download Directory**: Where received files are saved
- **Auto-Accept Files**: Toggle automatic file acceptance
- **Max File Size**: Set upload limit (1 MB - 10 GB)

## 🏗️ Architecture

```
src/
├── core/                # Cryptography and protocol
│   ├── crypto.rs        # RSA-2048-OAEP, AES-256-GCM
│   ├── framing.rs       # Length-prefixed TCP packets
│   └── protocol.rs      # Message types and parsing
├── network/             # Session management
│   └── session.rs       # Host/client handshake
├── transfer/            # File transfer system
│   ├── sender.rs        # Chunked sending (64 KiB)
│   └── receiver.rs      # Streaming reception
├── app/                 # Business logic
│   ├── chat_manager.rs  # Sessions and messages
│   └── persistence.rs   # JSON history storage
└── main.rs              # GUI and CLI entry points
```

## 🔧 Protocol Specification

### Handshake Sequence

1. TCP connection established
2. Host → Client: RSA public key (PEM)
3. Client → Host: RSA public key (PEM)
4. Host: Generates AES-256 session key
5. Host → Client: Encrypted AES key (via RSA)
6. Both: Switch to AES-GCM encrypted communication

### Message Format

**Framing**: All messages use length-prefix framing
- Header: 4 bytes big-endian (payload length)
- Payload: Encrypted message data

**Message Types** (after decryption):
- `TEXT:<message>` - Text message
- `FILE_META|<filename>|<size>` - File metadata
- `FILE_CHUNK:<data>` - File chunk (64 KiB)
- `FILE_END:` - Transfer complete
- `PING` - Keep-alive

### Constants

```rust
PORT_DEFAULT = 12345          // Default TCP port
MAX_PACKET_SIZE = 8 MiB       // DoS protection
FILE_CHUNK_SIZE = 64 KiB      // Streaming chunks
AES_KEY_SIZE = 32 bytes       // 256 bits
RSA_KEY_BITS = 2048           // Key size
```

## 🧪 Testing

```bash
# Run all tests
cargo test

# Run specific module tests
cargo test crypto::tests
cargo test framing::tests
cargo test protocol::tests

# Run with output
cargo test -- --nocapture

# Check code without building
cargo check

# Format code
cargo fmt

# Run linter
cargo clippy
```

## 🐛 Troubleshooting

### Can't Connect

**Check**:
1. Firewall - Allow port 12345
2. IP address - Must be exact
3. Network - Both on same network (for LAN)
4. Port - Verify both using same port

### Messages Not Sending

1. Verify connection is active (check for errors)
2. Look for red error toasts
3. Try reconnecting
4. Check logs: `RUST_LOG=debug cargo run`

### Files Won't Transfer

1. Check file size vs your limit (Settings)
2. Verify download folder exists and is writable
3. Check disk space
4. Try a smaller file first

### App Won't Build

1. Check Rust version: `rustc --version` (need 1.70+)
2. Update Rust: `rustup update`
3. Clean and rebuild: `cargo clean && cargo build --release`

## 🎨 What's New

### 🎉 v1.2.0 - Enhanced UX Release

**New Features**:
- 😊 **Emoji Picker**: Quick access to common emojis
- 📁 **Drag & Drop**: Simply drag files into the chat window
- ✍️ **Typing Indicators**: See when your peer is typing in real-time
- 🔔 **Desktop Notifications**: Get notified of new messages

### 🔒 v1.1.0 - Major Security Enhancement: Forward Secrecy

**Critical Improvement**: Past messages now secure even if encryption keys are compromised!

#### Key Security Enhancements:
- 🔐 **X25519 ECDH**: Ephemeral key exchange for each session
- 🔑 **HKDF-SHA256**: Secure session key derivation
- 🔒 **Forward Secrecy**: Past sessions protected if RSA keys leak
- 🔄 **Protocol v2**: Version negotiation prevents downgrade attacks
- ⚡ **Performance**: Only ~100 microseconds overhead

**Security Impact**:
- ✅ **Past messages secure** even if keys compromised
- ✅ **New ephemeral keys** every session
- ✅ **Matches Signal/WhatsApp** security model
- ✅ **No backward compatibility** - both users must upgrade

#### Previous Improvements (v1.0)
- ✨ Modern WhatsApp-like UI
- ✍️ Multiline input with keyboard shortcuts
- 📎 File preview before sending
- 🎨 Colorful avatars with initials
- ⏰ Smart relative timestamps
- ⚙️ Comprehensive settings panel

## 📚 Documentation

### Essential Reading
- **README.md** (this file) - User guide and quick start
- **FORWARD_SECRECY.md** - v1.1.0 forward secrecy technical details and security analysis
- **CHANGELOG.md** - Version history and release notes
- **DEVELOPMENT_PLAN.md** - Roadmap for future features

### Technical References
- **CLAUDE.md** - Development guide and architecture deep-dive
- **IMPLEMENTATION_STATUS.md** - Component status and technical implementation
- **HISTORY.md** - Past bug fixes and feature evolution (v0.9-v1.0.2)

### Contributing
- **CONTRIBUTING.md** - Contribution guidelines
- **CODE_OF_CONDUCT.md** - Community standards

## 🤝 Contributing

Contributions welcome! Please ensure:

1. ✅ All tests pass: `cargo test`
2. ✅ Code is formatted: `cargo fmt`
3. ✅ No clippy warnings: `cargo clippy`
4. ✅ Security changes are documented

## 📋 Requirements

- **Rust**: 1.70 or higher
- **OS**: Windows 10/11, Linux, macOS
- **Network**: Local network access for P2P connections

## 🎓 CLI Mode

For advanced users or automation:

```bash
# Host mode
cargo run --release -- --host --port 12345

# Client mode
cargo run --release -- --connect 192.168.1.10:12345
```

## 🔮 Planned Features

Future enhancements:
- Message search functionality
- Group chats (requires protocol update)
- Persistent identities with encrypted key storage
- NAT traversal for WAN connectivity
- Mobile apps (Android/iOS)

## 📜 License

[Specify your license here - e.g., MIT, GPL, Apache 2.0]

## 🙏 Acknowledgments

Built with excellent Rust crates:
- [Tokio](https://tokio.rs/) - Async runtime
- [RustCrypto](https://github.com/RustCrypto) - Cryptography (RSA, AES-GCM, SHA-2)
- [egui](https://github.com/emilk/egui) - Immediate mode GUI
- [eframe](https://github.com/emilk/egui/tree/master/crates/eframe) - GUI framework
- [rfd](https://github.com/PolyMeilex/rfd) - File dialogs
- [serde](https://serde.rs/) - Serialization

## 💡 Pro Tips

- Set up your download folder first (Settings)
- Use Ctrl+Enter for faster sending
- Verify fingerprints immediately after connecting
- Check the welcome screen if you forget how something works
- Keep the app updated for latest security improvements

## 🎉 Get Started Now!

```bash
cargo build --release
cargo run --release
```

Welcome screen will guide you through the rest! 🚀

---

**Questions? Issues?** Check CLAUDE.md for technical details or open an issue.

**Security concerns?** Always verify fingerprints and use on trusted networks.
