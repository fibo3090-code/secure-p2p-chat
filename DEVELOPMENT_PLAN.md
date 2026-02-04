# Development Plan

This document serves as the unified development plan for the Encrypted P2P Messenger, consolidating the project roadmap, security roadmap, and actionable to-do items. It provides a single source of truth for the project's direction, priorities, and status.

---

## 🎨 Design Philosophy

- **Simplicity**: Remove technical jargon from the UI and provide smart defaults.
- **User Experience First**: Every feature should have a clear "why" and "how". Delight users with polish.
- **Integration**: Features should feel like they belong together with a consistent design language.
- **Innovation**: Leverage the P2P nature of the app for unique features.

---

## 📅 Review & Update Process

This document is updated regularly following each release and major feature completion.

1. **Weekly Review**: Assess status of "IN PROGRESS" items.
2. **Post-Release**: Increment version status and move "COMPLETE" items to the archive section.
3. **Audit Alignment**: Ensure findings from security and UI audits are prioritized in the backlog.

---

## 📊 Current Project Status (v1.7.5)

### ✅ What Works

- **Core Security**: End-to-end encryption with Forward Secrecy (X25519 ECDH) and Identity Hardening (Ed25519/RSA-PSS).
- **Messaging**: Text, file transfer (chunked), typing indicators, and emojis.
- **Trust & Discovery**: Trust-on-First-Use (TOFU) and mDNS Local Peer Discovery.
- **Persistence**: Encrypted-at-rest chat history and identity keystore (Argon2 + ChaCha20-Poly1305).
- **Hardening**: DoS protection, replay protection, and memory hygiene (`Zeroize`).

### ⚠️ Known Limitations

1. **Internet Peer Discovery**: Internet connectivity still requires manual IP exchange or port forwarding. NAT traversal (UPnP/PMP) is a goal for v2.0.
2. **Platform Distribution**: Packaging is currently optimized for Windows; macOS and Linux lack automated pipelines.

---

## 🚀 Roadmap by Version

### v1.8: The Hardening & Polish Release

Focus on tightening security protocols and refining the core UI/UX based on audit findings.

### v1.9: The Quality & Community Release

Focus on testing infrastructure, community documentation, and internal code quality.

### v2.0: The Ecosystem Release

Expand capabilities with NAT traversal, command palettes, and preparations for mobile.

---

## 🎯 Detailed Action Items

### 🔥 CRITICAL PRIORITY

| Title | Description | Status | Owner | Due Date | Related |
|-------|-------------|--------|-------|----------|---------|
| **Professional Security Audit** | Engage independent cryptographic firm to review handshake, key derivation, and protocol. | PLANNED | [Unassigned] | TBD | N/A |
| **Post-Quantum Cryptography** | Implement hybrid classical/PQC handshake (Kyber/Dilithium). | PLANNED | [Unassigned] | v2.0.0 | N/A |

### 🏃 HIGH PRIORITY

| Title | Description | Status | Owner | Due Date | Related |
|-------|-------------|--------|-------|----------|---------|
| **Handshake Sequence Diagram** | Create a sequence diagram for the protocol in `docs/04_protocol.md`. | PLANNED | [Unassigned] | TBD | TODO.md |
| **Re-keying Mechanism Diagram** | Create a diagram for the re-keying mechanism in `docs/04_protocol.md`. | PLANNED | [Unassigned] | TBD | TODO.md |
| **Out-of-Band Verification** | implement QR code flow for easier fingerprint comparison. | PLANNED | [Unassigned] | TBD | ROADMAP.md |
| **Hardware Signing Support** | Support for FIDO2/YubiKey for identity keys. | PLANNED | [Unassigned] | v2.0.0+ | ROADMAP.md |
| **NAT Traversal** | UPnP/PMP support for internet-based discovery. | PLANNED | [Unassigned] | v2.0.0 | ROADMAP.md |
| **Onion Routing (Tor)** | Optional routing for IP address anonymity. | PLANNED | [Unassigned] | v2.0.0+ | ROADMAP.md |

### 🛠️ MEDIUM PRIORITY (Backlog)

| Title | Description | Status | Owner | Due Date | Related |
|-------|-------------|--------|-------|----------|---------|
| **Pure Rust mDNS** | Replace `mdns-sd` to remove external dependency on Bonjour/Avahi. | PLANNED | [Unassigned] | TBD | ROADMAP.md |
| **QR Code Scanning** | Implementation of camera-based scanning for invite codes on Windows. | PLANNED | [Unassigned] | TBD | ROADMAP.md |
| **File Transfer UI Polish** | Add progress bars and cancellation controls in chat view. | PLANNED | [Unassigned] | TBD | ROADMAP.md |
| **Settings Tab Refactor** | Organize settings into tabs (General, Appearance, Security). | PLANNED | [Unassigned] | TBD | ROADMAP.md |
| **Quick Switcher (Ctrl+K)** | Floating search bar for instant chat/contact switching. | PLANNED | [Unassigned] | TBD | ROADMAP.md |
| **Glossary** | Create `docs/GLOSSARY.md` for technical/cryptographic terms. | PLANNED | [Unassigned] | TBD | TODO.md |
| **File Transfer Diagram** | Illustrate chunking/transfer process in `DEVELOPER_GUIDE.md`. | PLANNED | [Unassigned] | TBD | TODO.md |
| **Version Sync Hook** | Script/hook to sync version across all docs on release. | PLANNED | [Unassigned] | TBD | TODO.md |
| **README Streamline** | Consolidate README and GETTING_STARTED. | PLANNED | [Unassigned] | TBD | TODO.md |
| **Unit/Integration/Fuzzing** | Expand test coverage to 85%+, add fuzzing for protocol. | PLANNED | [Unassigned] | v1.9.0 | #8 |
| **Community Guidelines** | CONTRIBUTING.md, CODE_OF_CONDUCT.md, Signed Releases. | PLANNED | [Unassigned] | v1.9.0 | #9 |
| **Typed Errors & Logging** | Replace string errors with `thiserror`, add structured logging. | PLANNED | [Unassigned] | v1.9.0+ | #10 |

### 🎨 LOW PRIORITY (Polish)

| Title | Description | Status | Owner | Due Date | Related |
|-------|-------------|--------|-------|----------|---------|
| **Clipboard Protection** | Auto-clear clipboard after 30s of copying sensitive data. | PLANNED | [Unassigned] | TBD | ROADMAP.md |
| **Invite Link Expiration** | Revocation list and time-based expiration for v2 invites. | PLANNED | [Unassigned] | TBD | ROADMAP.md |
| **tl;dr for Security Docs** | Add executive summaries to THREAT_MODEL and SECURITY.md. | PLANNED | [Unassigned] | TBD | TODO.md |
| **WCAG AA Audit** | Review UI components for color contrast compliance. | PLANNED | [Unassigned] | TBD | DESIGN_NOTES.md |
| **ARIA Tags** | Implement accessibility tags for screen readers. | PLANNED | [Unassigned] | TBD | DESIGN_NOTES.md |

---

## ✅ Completed Milestones (v1.7.5 and below)

- **Protocol v3**: Encrypted identity exchange, DoS protection, memory hygiene.
- **Trust on First Use (TOFU)**: Automatic fingerprint saving and blocking warnings.
- **Local Peer Discovery**: mDNS integration.
- **Identity Hardening**: Ed25519 support and RSA-PSS signatures for invites.
- **Replay Protection**: Sequence number validation.
- **Session Key Rotation**: Automatic re-keying every 100 messages/5 mins.
- **Encrypted History**: Chat history encrypted at rest with ChaCha20-Poly1305.
- **AAD in AES-GCM**: Additional Authenticated Data support.
- **Security Docs**: Detailed `THREAT_MODEL.md` and expanded `SECURITY.md`.
- **UI Refresh**: Modal dialog refactor, typing indicators, emojis, and toasts.
