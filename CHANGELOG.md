# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### 🔐 Security

- **Issue #5 (Pending)**: Signature scheme hardening - planned Ed25519 migration from RSA-2048
- **Issue #6 (Pending)**: Replay protection and session key rotation policy
- **Issue #7 (Pending)**: Hardened invite links and token-based sharing

### 🐛 Bug Fixes & Polish

- Windows Add/Remove Programs icon: `UninstallDisplayIcon` in `setup.iss` now points to `encodeur_rsa_icon.ico`
- Toast messaging improvements for consistent English in-app strings

### 📚 Documentation

- Merged overlapping documentation files for cleaner structure
- Added comprehensive security roadmap

---

## [1.7.2] - 2026-01-24

### 🔐 Security Enhancements

- **Issue #1 (Complete)**: Added AAD (Additional Authenticated Data) support to AES-256-GCM encryption
  - Updated `encrypt()` and `decrypt()` signatures to accept optional AAD parameter
  - Strengthens AEAD guarantees with optional context binding
  - All 15 crypto tests passing, 53 unit tests passing total
  - Backward compatible: AAD is optional when not provided

- **Issue #2 (Complete)**: Verified and strengthened payload size validation
  - Confirmed TCP framing uses chunked reading to prevent DoS
  - Added test `test_framing_recv_reject_oversized_header()` for verification

- **Issue #3 (Complete)**: Deployed GitHub Actions CI/CD
  - Created `.github/workflows/security.yml` with 6 parallel jobs
  - Added `deny.toml` for supply-chain security auditing
  - Jobs: rustfmt, clippy, cargo test, cargo-audit, build-release, cross-platform
  - Updated cargo-audit from v1 to v2 with documented vulnerability suppression

- **Issue #4 (Complete)**: Created comprehensive security documentation
  - New file: `THREAT_MODEL.md` (500+ lines)
    - 6 threat actor profiles with detailed capabilities
    - 8 attack scenarios with specific mitigations
    - Assets protection analysis across 6 attack surfaces
    - Known limitations and future improvements
    - Security recommendations for users
  - Expanded `SECURITY.md` with responsible disclosure section
    - Vulnerability reporting process with defined SLAs
    - Severity levels: critical (2h/1w), high (4h/2w), medium (24h/1mo), low (1w/next)
    - Coordinated disclosure timeline (30-90 day embargo)
    - Security hall of fame for researchers
    - Security roadmap through v2.0.0

### 📝 Documentation Updates

- Updated version number from 1.7.1 to 1.7.2 in:
  - `Cargo.toml` (package version)
  - `setup.iss` (installer configuration)
  - `SECURITY.md` (application reference)
  - `THREAT_MODEL.md` (application reference)
- Version display in app GUI automatically pulls from `Cargo.toml` via `env!("CARGO_PKG_VERSION")`

### 📊 Completed Milestones

- ✅ CRITICAL Issues #1-4: All complete (80% of critical security work)
- ✅ Code quality: 53 unit tests passing, 0 errors, 0 warnings
- ✅ CI/CD: GitHub Actions workflow deployed and functional
- ✅ Documentation: Threat model, security policy, responsible disclosure process

### 🎯 Next Steps (Planned)

- **Issue #5**: Signature scheme hardening (Ed25519 migration) - 8-10 hours estimated
- **Issue #6**: Replay protection & key rotation policy - medium priority
- **Issue #7**: Hardened invite links & token-based sharing - medium priority
- **Professional security audit**: Engage third-party firm for cryptographic review

---

## [1.7.1] - 2026-01-22

### 🔐 Security Fixes

- **Patched `rsa` Crate Vulnerability (CVE-2026-21895)**: Upgraded the `rsa` crate to version `0.9.10` to mitigate the "Marvin Attack", a timing side-channel vulnerability.
- **Remediated CodeQL Warnings**: Addressed multiple warnings about hard-coded cryptographic values in test functions by generating random keys directly.
- **Updated Dependencies**: Updated all project dependencies to their latest compatible versions to incorporate security fixes and improvements from the ecosystem.

## [1.7.0] - 2026-01-04

### 🐛 Bug Fixes

- **Removed File Size Limit**: Removed the 2GB file size limit for transfers. The application now relies on chunking to send large files, so there is no hard-coded limit. This fixes a critical bug that prevented large files from being sent.

## [1.6.0] - 2026-01-04

### 🔐 Critical Security Upgrade: Protocol v3

- **Encrypted Identity Exchange**: Implemented Protocol v3 where identity proofs are exchanged *inside* an encrypted tunnel. This prevents observers from seeing public keys or fingerprints (metadata protection).
- **DoS Protection**:
  - **Streaming Reads**: Protected against memory exhaustion attacks by validating packet headers before allocation.
  - **Handshake Timeouts**: Enforced strict timeouts for all handshake steps.
  - **Rate Limiting**: Added connection rate limiting per IP.
- **Improved Robustness**: Removed over 100 `unwrap()` calls from critical network paths to prevent crashes on malformed data.
- **Memory Hygiene**: Implemented `Zeroize` for sensitive keys to ensure they are wiped from memory.

### 📝 Documentation

- Updated `SECURITY.md`, `ROADMAP.md`, and `docs/04_protocol.md` to reflect the new security posture.

## [1.5.0] - 2025-12-20

### 🔐 Security & Hardening

- **Remediated Hard-coded Keys**: Replaced hard-coded cryptographic keys in all test suites (`crypto.rs`, `sender.rs`) with securely generated random keys. This resolves five critical CodeQL warnings.
- **Hardened Cipher Initialization**: Refactored `AesCipher::new` to return a `Result`, preventing the application from crashing on an invalid AES key length. All call sites in production and test code were updated to handle the new return type.

### ✅ Tests & Verification

- **Corrected Handshake Test**: Rewrote the `test_full_handshake` integration test to accurately test the production code's modern, forward-secret ECDH handshake, resolving a major discrepancy between the test suite and the actual implementation.
- **Fixed Security Test**: Corrected a bug in `test_file_meta_parsing_robustness` where the test was using a malformed payload, allowing it to properly validate input sanitization logic.

### 🔧 Code Quality & Documentation

- **Linter Clean-up**: Fixed all (8) `clippy` warnings across the codebase, improving code style and idiomaticity.
- **Documentation Sync**: Updated `DEVELOPER_GUIDE.md` to be consistent with the production codebase. Corrected the AES nonce generation description and updated the `ProtocolMessage` enum definition to include all variants.

## [1.4.0] - 2025-12-19

### ✨ New Features & Enhancements

- **QR Code Connection**: Generated invite links can now be displayed as QR codes for easy scanning and contact addition.
  - Files: `src/gui/dialogs.rs`, `src/gui/app_ui.rs`
  - Impact: Simplifies contact onboarding and sharing of invite links.

### 🔐 Security Fixes (December 18, 2025)

- **\[HIGH\] Version Downgrade Protection**: Implemented signed version announcements during handshake.
  - Peers now exchange digitally signed protocol versions, verified with RSA public keys.
  - Files: `src/network/session.rs`
  - Impact: Prevents attackers from forcing communication over older, less secure protocol versions.
- **\[HIGH\] Replay Attack Protection**: Fully implemented session sequence validation
  - Added `seq: u64` field to all `ProtocolMessage` variants
  - Per-chat `send_seq` and `recv_seq` tracking in `Chat` struct
  - All outgoing messages increment `send_seq` before transmission
  - All incoming messages validate `seq > recv_seq` before processing
  - Invalid/duplicate sequence numbers are logged and discarded
  - Covers all message types: Text, FileMeta, FileChunk, FileEnd, Ping, TypingStart, TypingStop
- **\[CRITICAL\] Encrypted Chat History at Rest**: Implemented ChaCha20-Poly1305 encryption for chat history storage
  - Added `save_encrypted()` and `load_encrypted()` methods to `HistoryFile`
  - Random nonce generation per save operation
  - Authenticated encryption prevents tampering
  - Restrictive file permissions (0600 on Unix)
- **\[HIGH\] Counter-Based Nonces**: Replaced random nonces with deterministic counters
  - Guaranteed nonce uniqueness for AES-GCM
  - Structure: `session_id (4 bytes) || counter (8 bytes)`
  - Eliminates birthday paradox collision risk

### 🔧 Compilation Fixes (December 18, 2025)

- **Rust 2021 Compatibility**: Refactored let chains to nested if-let statements
  - Fixed ~20 instances across `src/app/chat_manager.rs` and `src/gui/*.rs`
  - Removed deprecated ChaCha20-Poly1305 API usage (`Nonce::from_slice` → `Nonce::from`)
  - Fixed RSA-PSS signing by using `RandomizedSigner::sign_with_rng`
  - Project now compiles successfully on Rust 2021 edition

### 📊 Security Posture

- **Overall Risk:** Improved from CRITICAL → MEDIUM
- **Vulnerabilities Fixed:** 8 out of 14 (57%)
- **Critical Issues:** 2/2 fixed (100%)
- **High Priority:** 5/5 fixed (100%)

## [1.3.1] - 2025-11-16

### 🔧 Improvements

- Auto-rehost now shows a success toast: "Host relancé" after a listener is restarted.
- Added a minimal guard to prevent multiple concurrent listeners on the same port, avoiding duplicate hosts during auto-rehost.

### ✅ Tests

- Added unit test to validate placeholder-host detection used by the listener guard.

## [1.3.0] - 2025-11-12

### 🐛 Bug Fixes

- **Fixed Chat Creation Synchronization Issue**: When creating a new chat from the contacts list, the chat was created locally but not propagated to the peer instance, causing "all recipients offline" errors when sending messages.
  - Added `SessionEvent::NewConnection` to properly notify the receiving peer about new incoming connections
  - Enhanced handshake to exchange chat IDs between client and host
  - Modified UI flow to create local chat immediately for responsiveness, then connect in background
  - Updated `connect_to_host()` and `connect_to_contact()` to accept optional `existing_chat_id` parameter

### 🔧 Technical Changes

- Modified `src/network/session.rs`: Client now sends chat_id to host during handshake (step 7)
- Enhanced `src/app/chat_manager.rs`: Added handler for `SessionEvent::NewConnection` to create chats on incoming connections
- Improved `src/gui/dialogs.rs`: "Open chat" button now creates chat locally, then connects asynchronously
- Updated `src/types.rs`: Added `NewConnection` variant to `SessionEvent` enum

### ✅ Improvements

- Chats now sync immediately across both peer instances
- Messages are reliably routed to correct sessions
- Better user experience with instant UI feedback during chat creation
- Backward compatible with existing connection methods

## [1.2.0] - 2025-10-31

### ✨ New Features & Enhancements

- **🎨 Emoji Picker**: Quick access to 32 common emojis with a dedicated picker button.
- **📁 Drag & Drop File Transfer**: Drag files directly into the chat window to send them.
- **🔔 Desktop Notifications**: Get notified when new messages arrive (configurable).
- **✍️ Typing Indicators**: See when your peer is typing in real-time.
- **💾 Auto-Save**: Conversations automatically saved every 30 seconds.
- **🗑️ Delete Chat**: Right-click or button to delete individual conversations.
- **⌨️ Keyboard Shortcuts**: `Ctrl+Enter` to send, `Escape` to clear input.
- **🔌 Connection Status**: Visual indicators for connected/disconnected state.

### 🎨 UI/UX Improvements

- Improved chat header with dynamic status display.
- Better visual feedback for typing state.
- Enhanced Settings panel with new toggles for notifications and typing indicators.
- Clickable chat rows for better usability.
- Delete confirmation dialog to prevent accidental deletion.
- Toast notifications for all errors.

### 🔧 Technical Changes

- Added `notify-rust` for desktop notifications.
- Added `emojis` for emoji support.
- Extended protocol with `TypingStart` and `TypingStop` message types.
- Updated `Config` struct with `enable_notifications` and `enable_typing_indicators` fields.

## [1.1.0] - 2025-10-31

### 🔐 Major Security Enhancement: Forward Secrecy

- **Critical Security Improvement**: Implemented forward secrecy using X25519 Elliptic Curve Diffie-Hellman (ECDH).
- **Past messages are now secure** even if long-term RSA keys are compromised.
- **Ephemeral keys** are generated for each session and discarded after use.
- **HKDF-SHA256** is used for key derivation from the shared secret.
- **Protocol version 2** is introduced with version negotiation to prevent downgrade attacks.

### 🔧 Technical Changes

- Added `x25519-dalek` and `hkdf` dependencies.
- Extended the protocol with `Version` and `EphemeralKey` messages.
- Updated the handshake sequence to include ephemeral key exchange.

## [1.0.2] - 2025-10-23

### 🐛 Critical Bug Fix: Messages Not Being Received

- **Issue**: Messages were sent successfully but never appeared in the receiver's chat.
- **Root Cause**: Session events were being logged but never processed by the `ChatManager`.
- **Fix**: Implemented a proper event polling and processing system in the UI update loop.

### ✨ Improvements

- **Enhanced Logging**: Added comprehensive trace/debug logging throughout the network layer.
- **Event Processing**: All session events (`Listening`, `Connected`, `MessageReceived`, etc.) are now properly handled.

## [1.0.0] - 2025-10-23

### 🎉 Major Release - Complete UI/UX Overhaul

This release transformed the application from a functional prototype into a polished, user-friendly messaging app.

### ✨ Added Features

- **Welcome Screen**: Onboarding guide for new users.
- **Settings Panel**: Configure download folder, file size limits, etc.
- **Enhanced Chat Experience**: Multiline text input, colorful avatars, smart timestamps, and visual feedback.
- **User Experience**: Smart send button, hover tooltips, and consistent layout.

### 🔧 Improvements

- Consolidated and improved documentation.
- Fixed various borrow checker issues and warnings.

## [0.9.0] - Previous Version

- Basic chat functionality.
- End-to-end encryption (RSA + AES-GCM).
- File transfer support.
- Simple GUI interface.
- Message history persistence.

[Unreleased]: #
[1.5.0]: #
[1.4.0]: #
[1.3.1]: #
[1.3.0]: #
[1.2.0]: #
[1.1.0]: #
[1.0.2]: #
[1.0.0]: #
[0.9.0]: #
