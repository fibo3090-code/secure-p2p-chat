# Security Policy & Documentation

**Last Updated:** February 23, 2026
Application: Encrypted P2P Messenger v1.7.7+

This document provides comprehensive security information including the threat model, cryptographic specifications, security audit findings, applied fixes, and vulnerability reporting guidelines.

> [!IMPORTANT]
> **tl;dr (Security Overview)**: This application implements industry-standard end-to-end encryption (AES-256-GCM) with perfect forward secrecy (X25519 ECDH). It follows a decentralized, serverless architecture where security is enforced through peer-to-peer verification (TOFU). Core identity keys and chat history are encrypted at rest.

---

## Table of Contents

1. [Security Overview](#security-overview)
2. [Threat Model](#threat-model)
3. [Cryptographic Specifications](#cryptographic-specifications)
4. [Security Audit History](#security-audit-history)
5. [Applied Security Fixes](#applied-security-fixes)
6. [Known Open Risks (Audit Feb 23, 2026)](#known-open-risks-audit-feb-23-2026)
7. [Security Roadmap](#security-roadmap)
8. [Vulnerability Reporting & Responsible Disclosure](#vulnerability-reporting--responsible-disclosure)

---

## Security Overview

This application implements **strong end-to-end encryption with known limitations** and forward secrecy; see [Known Open Risks](#known-open-risks-audit-feb-23-2026) for the items that motivated this audit.

### Current Security Posture

**Overall Risk Assessment:** **MEDIUM** (see Known Open Risks below for the highest-priority items and their resolutions)

**Security Achievements:**

- ✅ Strong cryptographic primitives (RSA-2048, AES-256-GCM, X25519)
- ✅ Forward secrecy via ephemeral key exchange
- ✅ Authenticated encryption (AES-GCM)
- ✅ Encrypted identity keystore at rest (Argon2 + ChaCha20-Poly1305)
- ✅ Counter-based nonces (prevents reuse)
- ✅ Replay attack protection (sequence numbers)
- ✅ Thread-safe architecture with narrowly scoped platform `unsafe` blocks (Windows console attach/alloc)
- ✅ Fingerprint verification enforcement

**Recent Security Improvements (Jan-Feb 2026):**

- Patched Dependabot findings for `bytes`, `time`, and `lru`.
- Remediated CodeQL findings for hard-coded crypto values and workflow permissions.
- Added signed v2 invite verification and transport-level replay protection.

---

## Known Open Risks (Audit Feb 23, 2026)

The highest-priority issues identified during this audit were addressed as part of the fixes documented here; we will re-open the corresponding GitHub issues if regressions emerge.

- [Issue #21](https://github.com/fibo3090-code/secure-p2p-chat/issues/21) (HIGH): File transfer sequence numbers now share the per-chat monotonic namespace that the transport-level replay protection tracks.
- [Issue #22](https://github.com/fibo3090-code/secure-p2p-chat/issues/22) (HIGH): GUI share flow now emits only signed v2 invite links.
- [Issue #23](https://github.com/fibo3090-code/secure-p2p-chat/issues/23) (MEDIUM): Session event routing now consistently uses the mapped chat identifier.
- [Issue #24](https://github.com/fibo3090-code/secure-p2p-chat/issues/24) (MEDIUM): CI now runs `cargo deny check advisories`, matching the dependency policy defined in `deny.toml`.
- [Issue #25](https://github.com/fibo3090-code/secure-p2p-chat/issues/25) (LOW): Security documentation now matches the implementation and references these items.

Dependency risk status:

- `RUSTSEC-2023-0071` (`rsa`) remains open with no upstream fixed release currently available.
- Unmaintained crate warnings remain for `bincode` and transitive `paste`; both are tracked and require migration planning.

---

## Threat Model

The application is designed to protect against the following threats:

**→ See [THREAT_MODEL.md](THREAT_MODEL.md) for comprehensive threat analysis, attack scenarios, and mitigations.**

This document summarizes key protections; the detailed threat model covers:

- **Threat Actor Profiles** (passive eavesdropper, MITM, local attacker, malicious peer, cryptanalytic attacker)
- **Detailed Attack Scenarios** (with mitigation analysis)
- **Known Limitations** (what we cannot protect against)
- **Future Improvements** (planned security enhancements)

---

## Cryptographic Specifications

### Encryption Primitives

- **Message Encryption**: AES-256-GCM
- **Key Exchange**: X25519 ECDH
- **Identity**: RSA-2048-OAEP
- **Fingerprinting**: SHA-256

### Forward Secrecy

Forward secrecy is a critical feature of this application, ensuring that a compromise of long-term keys does not compromise past session keys. This is achieved as follows:

1. **Ephemeral Keys**: For each new session, a new X25519 key pair is generated. These keys are used only once and are discarded at the end of the session.
2. **Key Derivation**: The shared secret derived from the ECDH key exchange is used as input to a Key Derivation Function (HKDF-SHA256) to generate a unique 32-byte AES-256 session key.
3. **Identity vs. Encryption**: Long-term RSA keys are used only for identity verification (fingerprints) and are not used for session encryption.

### Handshake Sequence (Protocol v3)

The handshake process has been upgraded to **Protocol v3** to eliminate metadata leakage. The new ECDH-first flow ensures that identities are only exchanged *inside* an encrypted tunnel:

1. **Version Negotiation**: Peers exchange protocol version (u32, plaintext) to ensure compatibility.
2. **X25519 Ephemeral Key Exchange (Plaintext)**:
   - Peers generate and exchange ephemeral X25519 public keys immediately.
   - **Forward Secrecy**: These keys are used ONLY for this session and are never reused.
3. **Session Key Derivation**:
   - A shared secret is computed via ECDH.
   - A 32-byte AES-256 session key is derived using HKDF-SHA256.
   - An encrypted tunnel is established using AES-256-GCM.
4. **Encrypted Identity Exchange**:
   - Peers exchange `IdentityProof` messages *inside* the encrypted tunnel.
   - `IdentityProof` contains the RSA Public Key and a **Signature** of the session's Ephemeral Key.
   - The signature binds the long-term identity to the ephemeral session, preventing Man-in-the-Middle (MITM) attacks.
5. **Fingerprint Verification**:
   - The received RSA public key's fingerprint is verified against known contacts.
   - If unknown, the user is prompted to verify and trust the new identity (TOFU).

---

## Security Audit History

### Internal Audit (December 18, 2025)

**Status: PASSED (Excellent)**

A comprehensive security audit was conducted on December 18, 2025.

**Summary of Findings:**

- **Identity Storage**: ✅ Secure (encrypted with Argon2id + ChaCha20Poly1305).
- **Nonce Management**: ✅ Secure (atomic counters prevent reuse).
- **Replay Protection**: ✅ Secure (transporter-layer sequence numbers).
- **DoS Protection**: ✅ Secure (rate limiting and handshake timeouts).
- **Protocol Security**: ✅ Secure (Protocol v3 implements forward secrecy and prevents MITM).

**Conclusion**: No exploitable vulnerabilities were found in the core cryptographic design or implementation.

---

## Applied Security Fixes

### Phase 1: Immediate Hardening (Dec 2025)

#### 1. Denial of Service (DoS) Protection

- **Rate Limiting**: Implemented a global rate limiter to reject excessive connections from a single IP.
- **Handshake Timeouts**: All handshake steps are wrapped in strict timeouts to prevent Slowloris attacks.
- **Chunked Reads**: Replaced large buffer pre-allocation with chunked streaming reads to prevent memory exhaustion attacks.
- **Files**: `src/network/session.rs`, `src/core/framing.rs`

#### 2. Memory Hygiene

- **Zeroize Integration**: Critical secrets (private keys, session keys) are wrapped in `Zeroizing<T>` to ensure they are securely wiped from memory when dropped.
- **Files**: `src/identity/mod.rs`, `src/network/session.rs`

#### 3. Logging Sanitization

- **Redacted Logs**: Sensitive content (message text, file chunks) is now redacted in debug logs using a custom `Debug` implementation.
- **Truncation**: Raw plaintext logs during parsing failures are truncated to prevent log flooding.
- **Files**: `src/core/protocol.rs`, `src/network/session.rs`

### Phase 2: Protocol v3 (Privacy & Authentication)

#### 4. Encrypted Identity Exchange (Protocol v3)

- **Problem**: Previous protocol exchanged RSA public keys in plaintext, allowing passive observers to link communicating peers.
- **Fix**: Implemented ECDH-first handshake. Identities are now exchanged only after an encrypted tunnel is established.
- **Impact**: full metadata privacy for communicating parties.
- **Files**: `src/core/protocol.rs`, `src/network/session.rs`

#### 5. Code Quality & Robustness

- **Error Handling**: Systematically replaced over 100 dangerous `unwrap()` calls with proper error propagation (`Result/Option`) in critical paths.
- **Files**: `src/network/session.rs`, `src/transfer/*`, `src/identity/mod.rs`

### Phase 3: Dependency and Codebase Hardening (Jan 2026)

#### 6. Dependency Vulnerability Patching

- **rsa Crate Panic (CVE-2026-21895)**: Upgraded the `rsa` crate to version `0.9.10` to patch a vulnerability where creating a private key from components with a prime equal to 1 could cause a panic.
- **Current Limitation**: `RUSTSEC-2023-0071` (Marvin timing sidechannel in `rsa`) still has no fixed upstream version; risk is mitigated operationally but not eliminated.
- **General Dependency Update**: Updated all dependencies to their latest compatible versions to incorporate security fixes and improvements from the ecosystem.
- **Files**: `Cargo.toml`, `Cargo.lock`

#### 7. CodeQL Warning Remediation

- **Hard-coded Cryptographic Values**: Addressed multiple CodeQL warnings about hard-coded cryptographic values in test functions by generating random keys directly instead of initializing with zeros first.
- **Files**: `src/core/crypto.rs`, `src/transfer/sender.rs`

### Phase 4: Password Gate (Jan 2026)

#### 8. Blocking Unlock / Set-Password Screen

- **Problem**: The app allowed full use (chats, connections, sending) even when the identity was password-protected and not yet unlocked. Users could dismiss or ignore the password dialog.
- **Fix**: The entire main UI (menus, sidebar, chats, status) is blocked until the user either unlocks with their password or sets a password for a new/legacy identity. A full-screen auth screen is shown; it cannot be closed or bypassed. Auto-host and auto-connect do not run until after unlock/set-password.
- **Impact**: Ensures the private key is never used for network operations without the user first proving knowledge of the password.
- **Files**: `src/gui/app_ui.rs`, `src/gui/dialogs.rs`

### Phase 5: Signature Hardening & Transport Protection (Jan 24, 2026)

#### 9. Ed25519 Support with Signature Scheme Negotiation

- **Problem**: RSA-2048 is resource-intensive and slower than modern alternatives like Ed25519.
- **Fix**: Implemented Ed25519 identity key generation and signing with backward-compatible negotiation.
  - `SignatureScheme` enum added to protocol (RSA = 0, Ed25519 = 1)
  - Handshake negotiates signature scheme during `IdentityProof` exchange
  - New identities default to Ed25519; existing RSA identities continue to work
  - Full dual-mode support: peers can sign with different schemes simultaneously
- **Impact**: Improved performance and crypto hygiene; cleaner identity infrastructure for future migrations.
- **Files**: `src/core/crypto.rs`, `src/core/protocol.rs`, `src/network/session.rs`
- **Tests**: 12 new Ed25519 tests in `src/core/crypto.rs`; 20+ protocol tests updated

#### 10. Replay Protection at Transport Layer

- **Problem**: Attacker could capture and replay old valid messages, potentially causing double-sends or state corruption.
- **Fix**: Implemented per-session sequence number validation in transport layer.
  - Each `Session` tracks `last_recv_seq` (last received sequence number)
  - All incoming messages validated before emission to ChatManager
  - Out-of-order messages rejected with detailed error logging
  - Duplicate and old messages dropped (defensive against replay attacks)
- **Impact**: Eliminates replay attack surface for standard messages and now extends the same guarantee to file-transfer traffic by keeping those packets in the per-chat monotonic sequence namespace (see Issue #21).
- **Files**: `src/network/session.rs`, `src/types.rs`
- **Tests**: 8 new replay detection tests covering:
  - Duplicate message rejection
  - Out-of-order message rejection
  - Old message rejection
  - Valid sequence accepted

#### 11. Hardened Invite Links with RSA-PSS Signatures (Issue #7, Jan 24, 2026)

- **Problem**: V1 invite links (unsigned base64-encoded JSON) are vulnerable to tampering attacks. An attacker could intercept an invite link and modify the fingerprint, address, or public key without detection.
- **Fix**: Implemented v2 signed invite format with RSA-PSS-SHA256 signatures.
  - New function `Identity::generate_signed_invite_link()` creates v2 invites with:
    - Deterministic JSON serialization of payload (version, timestamp, nonce, identity info)
    - RSA-PSS-SHA256 signature over the payload
    - URL-safe base64 encoding (RFC 4648) without padding
    - Ephemeral nonce (random bytes) for uniqueness per invite
    - Timestamp field (not validated; invites do not expire)
  - Updated `ChatManager::parse_invite_link()` to support both v1 and v2:
    - Automatically detects v2 format by `/v2/` URL prefix
    - Verifies RSA-PSS signature before accepting v2 invites
    - Falls back to v1 parsing with deprecation warning for backward compatibility
    - Rejects any v2 invite with tampered signature or invalid public key
  - Added `rsa_sign_pss()` and `rsa_verify_pss()` helper functions to crypto module
- **Impact**: Prevents invite link tampering attacks. Users can safely share invites via untrusted channels without risk of fingerprint substitution or address modification.
- **Files**: `src/identity/mod.rs`, `src/app/chat_manager.rs`, `src/core/crypto.rs`, `docs/04_protocol.md`
- **Tests**: 11 new comprehensive tests covering:
  - V2 signed invite generation
  - V2 signature verification success
  - V2 signature tampering detection and rejection
  - Fingerprint/address swap attack prevention
  - V1 backward compatibility with warning logs
  - Timestamp and nonce uniqueness validation
  - URL-safe base64 encoding verification
- **Security Guarantees**:
  - **Authenticity**: Only the identity holder can create valid invites (signature proves origin)
  - **Integrity**: Any bit-level modification of the invite is detected and rejected
  - **Uniqueness**: Each invite is unique (random nonce prevents exact replay at transport layer)
  - **Non-Repudiation**: The sender cannot deny creating the invite (RSA signature)
  - **Non-Expiring**: Invites do not expire because the timestamp field is not validated during verification

---

## Security Roadmap

For the detailed security roadmap, including planned features like Key Rotation, Hardware Signing, and Post-Quantum Cryptography, please refer to the main **[DEVELOPMENT_PLAN.md](DEVELOPMENT_PLAN.md)**.

---

## Vulnerability Reporting & Responsible Disclosure

We take security seriously and welcome responsible vulnerability reports from security researchers, developers, and users.

### Policy Overview

- **Coordinated Disclosure**: We follow responsible disclosure practices and ask researchers to keep reports confidential until a fix is released
- **No Public Issues**: Security vulnerabilities should NOT be reported via GitHub public issues
- **Timely Response**: We aim to acknowledge all reports within 48 hours
- **Transparent Process**: We will keep you informed throughout the remediation process
- **Credit**: Researchers who help us improve security will be recognized (with permission)

### How to Report a Vulnerability

#### Step 1: Prepare Your Report

Gather information about the vulnerability:

- **Clear Description**: What is the vulnerability? (e.g., buffer overflow, weak crypto, logic flaw)
- **Impact Assessment**: What could an attacker achieve? (e.g., code execution, data disclosure, DoS)
- **Affected Versions**: Which versions of the application are vulnerable?
- **Affected Components**: Which modules or files are involved?
- **Reproducibility**: Can you provide steps to reproduce or a proof-of-concept?
- **Environment**: OS, Rust version, dependency versions where applicable
- **Mitigation Ideas**: Any suggestions for fixes (optional but helpful)

#### Step 2: Submit Responsibly

**Email**: <security@fibo3090-code.dev> *(replace with actual contact)*

---

## Related Documentation

- **[THREAT_MODEL.md](THREAT_MODEL.md)** - Comprehensive threat analysis, attack scenarios, mitigations, known limitations
- **[CONTRIBUTING.md](CONTRIBUTING.md)** - Guidelines for security-focused contributions
- **[docs/04_protocol.md](docs/04_protocol.md)** - Protocol v3 technical specification

---

## References & Standards

- **Encryption**: [NIST SP 800-38D (AES-GCM)](https://nvlpubs.nist.gov/nistpubs/Legacy/SP/nistspecialpublication800-38d.pdf)
- **Key Derivation**: [RFC 5869 (HKDF)](https://tools.ietf.org/html/rfc5869)
- **Key Exchange**: [RFC 7748 (Elliptic Curves for Security)](https://tools.ietf.org/html/rfc7748)
- **Digital Signatures**: [RFC 3447 (RSA Cryptography)](https://tools.ietf.org/html/rfc3447)
- **Responsible Disclosure**: [ISO/IEC 29147:2018](https://www.iso.org/standard/72319.html)

---

## Acknowledgments

We thank the following for contributions to our security posture:

- Security researchers who have helped identify and responsibly disclose vulnerabilities
- The Rust cryptography community for excellent libraries (aes-gcm, x25519-dalek, etc.)
- Contributors who have reviewed code, tested, and provided feedback

---

## Questions?

For security-related inquiries:

- **Vulnerability Reports**: See [Vulnerability Reporting & Responsible Disclosure](#vulnerability-reporting--responsible-disclosure)
- **General Questions**: Open an issue on GitHub (non-sensitive questions only)
- **Direct Contact**: [TBD - add contact info]

*This security policy is effective as of February 23, 2026 and supersedes all previous versions.*
