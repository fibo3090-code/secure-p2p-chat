# Documentation Summary & Index

This document provides a comprehensive index and summary of all project documentation for the Encrypted P2P Messenger.

## 📖 Documentation Structure

### Root Directory (Quick Reference)

- **[SECURITY.md](../SECURITY.md)** - Comprehensive security policy, audit findings, and applied fixes
- **[CHANGELOG.md](../CHANGELOG.md)** - Release notes, fixes, and improvements
- **[CONTRIBUTING.md](../CONTRIBUTING.md)** - How to contribute to the project
- **[ROADMAP.md](../ROADMAP.md)** - Development roadmap and future plans
- **[DEVELOPER_GUIDE.md](../DEVELOPER_GUIDE.md)** - Technical guide for developers
- **[DESIGN_NOTES.md](../DESIGN_NOTES.md)** - Comprehensive design & UI/UX guide
- **[README.md](../README.md)** - Project overview and quick start

### Detailed Documentation (docs/ folder)

- **[01_introduction.md](01_introduction.md)** - Project overview and goals
- **[02_getting_started.md](02_getting_started.md)** - Installation and setup guide
- **[03_architecture.md](03_architecture.md)** - System architecture and components
- **[04_protocol.md](04_protocol.md)** - Protocol specification and message formats

---

## Project Overview

The project is a **secure, peer-to-peer desktop messaging application** built with Rust and the `egui` library. Its primary goal is to provide a private and secure communication channel without relying on a central server.

## Core Features

- **End-to-End Encryption**: Utilizes a combination of RSA for identity and AES-256-GCM for message encryption.
- **Forward Secrecy**: Implemented using the X25519 Elliptic Curve Diffie-Hellman (ECDH) key exchange to protect past conversations even if long-term keys are compromised.
- **Peer-to-Peer (P2P) Architecture**: Direct communication between users on a local network (LAN) or VPN, eliminating the need for a central server.
- **File Transfer**: Supports sending and receiving files with chunking.
- **User-Friendly Interface**: A modern GUI with features like typing indicators, emoji support, and desktop notifications.
- **Local Persistence**: Chat history and user identity are stored locally on the user's device.

## Key Technologies

- **Programming Language**: Rust
- **GUI Framework**: `egui`
- **Asynchronous Runtime**: `tokio`
- **Cryptography Libraries**: `rsa`, `aes-gcm`, `x25519-dalek`, `hkdf`
- **Serialization**: `serde`, `serde_json`, `bincode`

## Architecture

The application follows a layered architecture:

1. **GUI Layer**: Handles user interaction (built with `egui`).
2. **Business Logic Layer**: Manages the application's state, including chats and sessions.
3. **Core Layers**:
    - **Network**: Manages TCP connections and the communication protocol.
    - **Crypto**: Implements all cryptographic operations.
    - **Transfer**: Handles file transfers.
    - **Identity**: Manages the user's persistent RSA keys.

## Protocol

The application uses a custom TCP-based protocol (version 2) with the following characteristics:

- **Length-Prefixed Framing**: Each message is prefixed with its length.
- **Secure Handshake**: A multi-step handshake process establishes a secure session, including:
  - Version negotiation to prevent downgrade attacks.
  - Exchange of RSA public keys for identity verification.
  - Exchange of ephemeral X25519 keys to ensure forward secrecy.
  - Derivation of a session-specific AES key using HKDF.

## Contribution and Development

The project has clear guidelines for contributions, including:

- **Conventional Commits** for commit messages.
- A requirement for `cargo fmt` and `cargo clippy` to be run before submitting pull requests.
- A well-defined branching strategy and release process.

## Development Roadmap

The project has an ambitious roadmap for future development:

- **v1.4.0** (Current): Usability improvements and security fixes
- **v2.0**: The Professional Release
  - Automatic peer discovery (mDNS)
  - NAT traversal for internet connectivity
  - Message search and moderation tools
- **v3.0**: The Next Generation
  - Post-quantum cryptography
  - Mobile applications
  - Voice/video calls

## Security

Security is a primary focus of the project, with:

- A detailed **threat model** that considers eavesdropping, tampering, and key compromise.
- A strong emphasis on **fingerprint verification** to prevent man-in-the-middle attacks.
- A responsible **vulnerability disclosure policy**.
- **Current Security Status**: MEDIUM risk (improved from CRITICAL)
  - 7 out of 14 vulnerabilities fixed (50%)
  - All critical issues resolved (2/2)
  - Most high-priority issues resolved (4/5)

## Recent Updates (December 2024)

### Documentation Consolidation

- Merged security documentation into comprehensive SECURITY.md
- Consolidated design documentation (DESIGN_NOTES.md + ui_ux_principles.md)
- Organized detailed security reports in docs/ folder
- Updated all cross-references and links
- Removed redundant and temporary files

### Security Improvements

- Encrypted chat history at rest (ChaCha20-Poly1305)
- Replay attack protection (sequence numbers)
- Counter-based nonces for AES-GCM
- Thread-safe implementation (no unsafe code)
- Fingerprint verification enforcement

### Compilation Fixes

- Rust 2021 compatibility (let chains → nested if-let)
- Deprecated API fixes (ChaCha20-Poly1305)
- Unreachable code removal
- Project now compiles successfully

---

## Quick Navigation

**For Users:**

- Start here: [README.md](../README.md)
- Getting started: [02_getting_started.md](02_getting_started.md)
- Security info: [SECURITY.md](../SECURITY.md)

**For Contributors:**

- Contributing guide: [CONTRIBUTING.md](../CONTRIBUTING.md)
- Developer guide: [DEVELOPER_GUIDE.md](../DEVELOPER_GUIDE.md)
- Architecture: [03_architecture.md](03_architecture.md)

**For Designers:**

- Design guide: [DESIGN_NOTES.md](../DESIGN_NOTES.md)
- Roadmap: [ROADMAP.md](../ROADMAP.md)

**For Security Researchers:**

- Security policy: [SECURITY.md](../SECURITY.md)
- Audit report: [SECURITY.md#security-audit-report-december-18-2025](../SECURITY.md#security-audit-report-december-18-2025)
- Applied fixes: [SECURITY.md#applied-security-fixes](../SECURITY.md#applied-security-fixes)
