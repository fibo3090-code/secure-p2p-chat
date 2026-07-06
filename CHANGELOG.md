# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added

- **Community lifecycle in the desktop app.** Joined communities are now
  remembered across restarts (saved to `parties.json` — address, username, and
  server name; never the password) and offered as one-click rejoin cards on the
  join screen. You can finally **leave** a community (header button with a
  confirm step), **join more than one** (a `+` tab in the community switcher),
  and recover from a lost connection or rejected join via a **Rejoin / Remove**
  banner instead of being stuck with a dead entry. Rejoining replaces the old
  entry (deduplicated by address).
- **Desktop installers in releases.** Tagged releases now also build and attach
  the Tauri desktop app's native installers (Windows MSI + NSIS, macOS DMGs for
  Intel and Apple Silicon, Linux deb/AppImage) alongside the classic egui
  binaries — previously the new desktop app could only be run from source. The
  Tauri CLI is now a devDependency (`npx tauri dev` works on a fresh clone), and
  the bundle version now tracks the workspace version instead of a stale `0.1.0`.
- **Unread badges in the desktop app.** The conversation list and the Chats rail
  now show real unread counts (messages that arrived while another view was
  open), clearing when the conversation is opened. Previously the unread badge
  was rendered but never fed. The Communities rail label is now "Communities"
  everywhere (was mixed "Parties"/"Party").
- **Community server identity pinning (TOFU).** The first join pins the server's
  fingerprint; a later join to the same address presenting a different identity
  is refused with a clear security warning instead of silently trusting the new
  key. Leaving a community clears its pin (the documented way to accept a
  legitimately redeployed server).

- **Community file sharing in the desktop app.** You can now share a file into a
  Community channel or direct message (a paperclip button in the composer) and
  download files others have shared (click a file message to save it via a native
  dialog). This wires up the client half of the Party file feature that had a
  complete server but no way to send or receive files from any client:
  `PartyManager` gained `send_file` / `send_file_dm` (optimistic append + inline
  upload, size-checked against the 4 MiB `MAX_INLINE_FILE_BYTES`) and
  `request_download` (correlates the async `FileData` response by content hash);
  the Tauri bridge added `party_send_file` / `party_send_file_dm` /
  `party_download_file` commands; and file messages now surface their content hash
  and size. Downloads remain access-checked server-side, so a member only receives
  files they can see.

### Security

- **Party input length caps.** The server now bounds member usernames (≤ 32
  characters), channel names (≤ 64 characters), and channel/DM message text (≤ 64
  KiB, matching the P2P transport cap) instead of accepting any string up to the 8
  MiB packet limit. This prevents a member from storing or broadcasting a
  multi-megabyte username, channel name, or message that every client would then
  persist and render. The desktop Communities UI mirrors the username/channel caps
  for immediate feedback; the server remains authoritative.

### Documentation

- Brought the docs back in sync with the code: the four-crate workspace (adding
  `p2pem-desktop`), the shipped **Tauri + React** desktop app (the plan docs still
  described an unshipped SolidJS rewrite), automatic session-key rotation and text
  chunking in the protocol reference and constants, mDNS LAN discovery, Party file
  download access control, the corrected pre-workspace `src/` paths in the developer
  and agent guides, and the current test count (290). Added a webview/IPC/CSP
  section to `SECURITY.md`, Party-operator and desktop surfaces to `THREAT_MODEL.md`,
  and a 2026 findings entry to `docs/AUDITS.md`.

## [1.11.1] - 2026-06-29

### Fixed

- **Release pipeline produces binaries again.** The `v1.11.0` tag shipped with a
  stale `Cargo.lock`, so every platform job in the Release workflow failed at
  `cargo build --locked` and no installers/archives were attached to the release.
  The lockfile is now kept in sync with the workspace version, restoring the
  Windows installer, macOS DMGs, and Linux tarball as release assets.

### Dependencies

- Rolled up the outstanding Dependabot updates: the grouped `rust-minor` updates,
  `rfd` 0.14 → 0.17, `rusqlite` 0.32 → 0.40, `mdns-sd` 0.11 → 0.20, `emojis`
  0.6 → 0.9, and the desktop frontend's `react`/`react-dom` 18 → 19, `vite`
  6 → 8, `@vitejs/plugin-react` 4 → 6, and `lucide-react`.

## [1.11.0] - 2026-06-29

### Added

- **File sharing in Party servers (Phase 2, slice 1).** Members can share files
  (up to 4 MiB inline) in channels and direct messages. The server stores each
  file once, content-addressed by SHA-256 and reference-counted, with the bytes on
  disk and metadata in the SQLite store; a file appears in history like a message
  and can be fetched by hash. Larger-file chunking and the Drive UI are still to
  come.

### Changed

- **TUI command polish.** `:help <command>` now opens focused command help, the
  Party channel command is canonicalized as `:party-create-channel` with the old
  `:party-channel` spelling kept as an alias, bare IPv6 connect targets no longer
  get misparsed as `host:port`, and `--tui --connect-relay` now reports a missing
  `--relay-token` instead of silently doing nothing.

## [1.10.1] - 2026-06-28

### Changed

- **Party server durability moved to embedded SQLite.** The server now mirrors its
  state (members, channels, message + DM history) to a `party.db` SQLite database
  under the operator's data dir, writing each change incrementally instead of
  rewriting a whole JSON snapshot on every message. An existing `party_state.json`
  snapshot is imported once on first start and then superseded. No configuration
  change for operators; the runtime model and behavior are unchanged.

### Fixed

- CI now builds the whole workspace: installs the WebKitGTK system dependencies the
  Tauri desktop crate needs, and applies `rustfmt` to that crate (both were missing
  after it was merged into the workspace).

## [1.10.0] - 2026-06-28

### Added

- **Tauri 2 desktop app (new `p2pem-desktop` crate).** A native desktop shell wrapping a React/Vite web UI (`desktop/src/`), driving the same `ChatManager` core as the egui/TUI front-ends through a `#[tauri::command]` bridge (`desktop/src-tauri/src/lib.rs`). Includes onboarding, conversations with message view, contacts, invite import/export, fingerprint verification with the safety-color grid, relays pane, settings, and a toast system. Run with `cd desktop && npx tauri dev`.
- **Conversation model.** Every `Chat` now carries `kind: ChatKind` (`Dm` / `Group` / `Channel`) and `transport: Transport` (`Direct` / `Relay` / `Server`), both `#[serde(default)]` for back-compat with existing history files. Front-ends use these for conversation badges.
- **Party server-routed direct messages (DMs).** Members can exchange DMs through a Party server in addition to channels, with the hub routing and persisting them; surfaced in the GUI Party window and the TUI.
- **Party channel creation and management** from the client.

### Changed

- **Workspace now has four members** — `core`, `client`, `server`, and `desktop/src-tauri`. Bare `cargo` commands still target the client.
- Consolidated the planning/spec docs into a single canonical platform spec and refreshed the architecture, protocol, README, and contributor guides to match the Phase 1 Party server and the new desktop UI direction.

## [1.9.0] - 2026-06-01

### Added

- **Party server (new `messenger-server` binary).** A self-hosted, multi-user server (the "Administered" trust tier) so non-technical users can join with an address + optional password + a username — no port-forwarding required. Members appear in a directory with presence, chat in channels with live broadcast, and the server stores history so people who were offline catch up on reconnect. The encrypted, authenticated transport reuses the existing Protocol v3 handshake (the server has its own TOFU-verified identity), with durable state (members/channels/history) and a stable server identity persisted across restarts. Run with `cargo run -p messenger-server`.
- **Party client UI.** Connect to and use a Party server from the GUI (a Party window with a join form, server/channel/member lists, message view, and a post box) and from the TUI (`:party-connect`, `:party-post`, `:party-status`).
- **P2P connection password + conversation lock.** Hosts can require an optional shared password (verified inside the encrypted v3 tunnel, after identity verification and before fingerprint confirmation, with a constant-time comparison) and can lock a conversation to refuse new connections. Reachable from the GUI (Host/Connect dialogs + a menu-bar lock toggle) and the TUI (`:connection-password`, `:lock`).
- **TUI overhaul**: the terminal interface is now fully usable on its own. Added a typed command language that exposes every action (connections, contacts, invites, file send, settings, identity, diagnostics), with a live autocomplete menu in command mode (Tab to complete, ↑/↓ to choose).
- TUI modal overlays: in-terminal fingerprint verification (`y`/`n` or `:verify`), password unlock / set-password on startup, contacts, settings, identity (with the same safety-color grid as the GUI), file-transfer progress, and a scrollable help/keybinding reference.
- TUI quality-of-life: auto-scrolling message view with clamping, proper cursor-aware text editing (mid-line edits, word delete, command history), a colored toast stack, unread/typing indicators, a readable status bar, and graceful encrypted-history save on a timer and on exit.

### Changed

- **Workspace restructure.** The single crate is now a Cargo workspace of three crates — `core` (`messenger-core`: crypto, protocol, identity, transport, shared types), `client` (`encodeur_rsa_rust`: the GUI/TUI app and its binary), and `server` (`messenger-server`) — so the client and server share `core`. No behavior change for the app: the client binary name and all packaging paths are unchanged. The v3 handshake was extracted into a reusable form so both the P2P sessions and the Party server use the exact same audited code.
- Test coverage expanded substantially (now 250+ workspace tests), including an end-to-end pipeline test over the real session functions, relay and Party server end-to-end tests, full protocol round-trip/symmetry coverage, and the connection-password gate.

### Fixed

- TUI could not complete a default handshake because it never surfaced fingerprint verification, and it never persisted chat history; both are fixed.
- TUI no longer registered each keystroke twice on Windows (now only acts on key-press events, not key-release).
- `ChatManager::rename_chat` truncated titles by bytes, which could panic on a multi-byte/emoji boundary; it now truncates by characters.
- Hardened HKDF session-key derivation to use explicit zero-initialized output buffers instead of misleading random prefill.
- Prevented AES-GCM nonce-counter wraparound so the transport now fails closed instead of risking nonce reuse after exhaustion.
- Rejected malformed legacy `EPHEMERAL_KEY:` payloads before allocation and added regression coverage for oversized handshake inputs.
- Fixed full data wipe and Windows uninstall cleanup so the saved identity, encrypted history, diagnostics, and password-protected local state are removed instead of surviving a reinstall.

### GitHub

- Added structured GitHub issue forms and a pull request template so bug reports, feature requests, and security triage come in with the context needed for faster review.

## [1.8.1] - 2026-04-22

### Fixed

- Restored `cargo clippy --all-targets -- -D warnings` by removing the current GUI/TUI lint regressions that were blocking CI.
- Reworked the GitHub release workflow to use idempotent `gh release create/upload --clobber` steps so rerunning a tag no longer fails on an existing release.
- Locked the macOS release builds to `Cargo.lock` and kept publishing separate Intel and Apple Silicon DMGs.
- Updated shipped version metadata and README packaging claims so repository docs match the automated release behavior.
- Added a proper onboarding tutorial and refreshed the user, developer, security, protocol, and audit docs to match the current app and CI behavior.

## [1.8.0] - 2026-04-22

### ✨ New Features: Automated Packaging & Distribution

- **Universal Build Pipeline**: Implemented a comprehensive GitHub Actions release workflow that automates builds for Windows, macOS, and Linux.
- **macOS DMG Support**: Added automated creation of standard `.dmg` disk images for both Intel (`x86_64`) and Apple Silicon (`aarch64`) Macs.
- **Smart App Bundling**: Automatically generates proper `Messenger.app` bundles on macOS with metadata (`Info.plist`) and resource support.
- **Automated Windows Installers**: Integrated Inno Setup into the CI to generate `.exe` installers for Windows on every version tag.
- **Linux Distribution**: Added automated Linux builds and tarball generation for improved cross-platform availability.
- **Automated GitHub Releases**: The system now automatically creates GitHub Releases and uploads all platform-specific artifacts when a new version tag is pushed.

### 🚀 Performance & Optimization

- **Aggressive Release Optimization**: Enabled Link-Time Optimization (LTO) and set codegen-units to 1 for maximum runtime performance.
- **Binary Size Reduction**: Configured automatic symbol stripping and panic-abort semantics to significantly reduce the size of distributed binaries.
- **Enhanced Caching**: Optimized CI/CD caching for Rust dependencies, reducing build times by up to 60% on subsequent runs.

### 🔐 Security Fixes

- Fixed handshake signature negotiation so the runtime now truthfully advertises and accepts only RSA-PSS identity proofs.
- Bound encrypted identity proofs and transport packets to the handshake transcript with AAD.
- Disabled password-removal for persisted identities and updated the GUI to reflect the enforced encrypted-at-rest model.
- Fixed the destructive local-data reset flow so it now deletes the encrypted history and identity files it claims to remove.
- Centralized endpoint parsing to support hostnames, IPv4, and bracketed IPv6 consistently across the app and invite handling.

### 🛠️ Engineering

- Synced repository metadata to `v1.8.0` (`Cargo.toml`, `README.md` badge, `setup.iss`, etc.).
- Moved periodic GUI autosave off the hot UI path by snapshotting state and persisting it in a background task.
- Added diagnostics bundle export and on-disk panic logs to make support and crash triage easier.
- Advanced the history format to `1.1` while keeping `1.0` load compatibility.

### ✨ TUI

- Implemented a command-driven, keyboard-first TUI shell with command palette and status line.
- Added commands: `:host`, `:connect`, `:disconnect`, `:rename`, `:help`, `:quit`.
- Improved rendering resilience for small terminal sizes.

### 🔐 Security

- **Issue #7 (Complete)**: Hardened invite links with RSA-PSS signatures
  - Implemented v2 signed invite format with RSA-PSS-SHA256 signatures
  - Each v2 invite includes payload (name, address, fingerprint, public_key, timestamp, nonce, version)
  - Signature verification prevents any tampering with invite data
  - Fingerprint swap attacks are cryptographically prevented
  - URL-safe base64 encoding (RFC 4648) for safe URL transmission
  - Backward compatible: v1 unsigned invites still parse with deprecation warning
  - 11 new comprehensive tests for v2 invite generation, verification, and tampering detection
  - Added `rsa_sign_pss()` and `rsa_verify_pss()` cryptographic functions to crypto module
  - Updated `docs/04_protocol.md` with v2 invite format specification and verification process
  - Updated `SECURITY.md` with Phase 11 (Hardened Invite Links) details

- **Issue #17 (Complete)**: Periodic session key rotation for perfect forward secrecy
  - Implemented automatic key rotation every 100 messages OR 5 minutes (whichever first)
  - Added `rekey_session_key()` function using HKDF-SHA256 for deterministic key derivation
  - Added `generate_rekey_nonce()` with cryptographically random nonce generation (OsRng)
  - New `Rekey` protocol message type (v3) with sequence validation and simultaneous-rekey resolution
  - Transparent rekeying: Rekey messages handled at protocol layer, not emitted to application
  - Bidirectional: Both peers independently detect rekey condition and derive identical new key
  - Added 5 comprehensive crypto tests (determinism, nonce independence, encryption flow)
  - Added 10 integration tests covering key rotation workflows, performance, and security
  - Performance: < 100ms per 1000 HKDF operations; negligible overhead (~0.03ms per operation)
  - All 105 tests passing (81 unit + 3 bug + 1 fuzz + 1 simulation + 4 integration + 10 key_rotation + 5 security)

### 📚 Documentation

- Updated `docs/04_protocol.md` with Section 4.5: Session Key Rotation (Rekeying)
  - Documented rekeying schedule (100 messages or 5 minutes)
  - Documented rekeying process for initiator and receiver
  - Specified `Rekey` message format: tag(1) + seq(8) + nonce_len(4) + nonce(N)
  - Explained security properties: forward secrecy, deterministic derivation, replay protection
  - Included performance metrics (~0.03ms per operation, negligible overhead)

- Updated `docs/04_protocol.md` with Section 4.6: Invite Links (v1 and v2 formats)
  - Documented v1 legacy format (deprecated)
  - Documented v2 signed format with RSA-PSS signature
  - Added signature verification process (5-step algorithm)
  - Added implementation notes for URL-safe base64, nonce uniqueness, timestamp handling

- Updated `SECURITY.md` with Phase 11: Hardened Invite Links with RSA-PSS Signatures
  - Security problem statement and impact analysis
  - Complete fix description with files and test coverage
  - Security guarantees: authenticity, integrity, uniqueness, non-repudiation

### 🔧 Code Quality

- **Hardcoded Cryptographic Values Remediation**: Replaced all hardcoded test keys and nonces
  - Replaced hardcoded test keys/nonces with cryptographically secure random values (`OsRng`)
  - Added `AES_GCM_TAG_SIZE` constant (16 bytes) to `src/lib.rs` for consistency
  - Replaced magic numbers (12, 16) with named constants (`AES_NONCE_SIZE`, `AES_GCM_TAG_SIZE`)
  - Added key uniqueness validation in key independence tests
  - Files modified: `src/lib.rs`, `src/core/crypto.rs`, `src/network/session.rs`, `tests/key_rotation_tests.rs`
  - Impact: Tests now use cryptographically secure random values instead of predictable patterns
  - All 81 unit tests passing

---

## [1.7.5] - 2026-02-04

### 🐛 Bug Fixes

- **Modal Dialogs**: Refactored the dialog system to use an `ActiveDialog` enum, preventing multiple dialogs from opening simultaneously and ensuring proper state reset on closure.
- **Build Warnings**: Fixed unused variable warnings in `dialogs.rs`.

### 📚 Documentation

- **Compatibility Audit**: Merged findings into `README.md` (System Requirements).
- **Security Audit**: Merged findings into `SECURITY.md` (Audit History).
- **Roadmap Update**: Added planned features for Pure Rust mDNS and QR Scanning.

---

## [1.7.4] - 2026-02-01

### 🛠️ Developer Tooling

- **Fixed CodeQL Local Script**: Fixed `run-codeql-local.ps1` to handle virtual drive mounting correctly.
  - Removed redundant `Copy-Item` command that caused "Cannot overwrite file with itself" error.
  - The script now correctly identifies that the virtual drive is mapped to the original workspace.

---

## [1.7.3] - 2026-01-24

### 🔐 Security Enhancements

- **Issue #5 (Complete)**: Signature scheme hardening - Added Ed25519 support with negotiation
  - Implemented Ed25519 identity key generation and signing
  - Added `SignatureScheme` negotiation during handshake
  - Backward compatible with RSA-2048; new identities default to Ed25519
  - 12 new crypto tests for Ed25519 functionality

- **Issue #6 (Complete)**: Replay protection and transport-layer validation
  - Implemented sequence number tracking in transport layer (per-session)
  - All incoming messages validated against last received sequence
  - Out-of-order, duplicate, and old messages rejected before emission
  - 8 new replay detection tests covering all attack vectors

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
[1.11.1]: #
[1.11.0]: #
[1.10.1]: #
[1.10.0]: #
[1.9.0]: #
[1.7.3]: #
[1.7.2]: #
[1.7.1]: #
[1.7.0]: #
[1.6.0]: #
[1.5.0]: #
[1.4.0]: #
[1.3.1]: #
[1.3.0]: #
[1.2.0]: #
[1.1.0]: #
[1.0.2]: #
[1.0.0]: #
[0.9.0]: #
