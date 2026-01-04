# Security Policy & Documentation

**Last Updated:** January 4, 2026  
Application: Encrypted P2P Messenger v1.6.0  

This document provides comprehensive security information including the threat model, cryptographic specifications, security audit findings, applied fixes, and vulnerability reporting guidelines.

---

## Table of Contents

1. [Security Overview](#security-overview)
2. [Threat Model](#threat-model)
3. [Cryptographic Specifications](#cryptographic-specifications)
4. [Security Audit Report (December 18, 2025)](#security-audit-report-december-18-2025)
5. [Applied Security Fixes](#applied-security-fixes)
6. [Remaining Security Work](#remaining-security-work)
7. [Reporting Security Issues](#reporting-security-issues)

---

## Security Overview

This application implements **military-grade end-to-end encryption** with **forward secrecy**, matching the security standards of leading messaging apps like Signal and WhatsApp.

### Current Security Posture

**Overall Risk Assessment:** **MEDIUM** (improved from CRITICAL)

**Security Achievements:**

- ✅ Strong cryptographic primitives (RSA-2048, AES-256-GCM, X25519)
- ✅ Forward secrecy via ephemeral key exchange
- ✅ Authenticated encryption (AES-GCM)
- ✅ Encrypted chat history at rest (ChaCha20-Poly1305)
- ✅ Counter-based nonces (prevents reuse)
- ✅ Replay attack protection (sequence numbers)
- ✅ Thread-safe implementation (no unsafe code)
- ✅ Fingerprint verification enforcement

**Recent Security Improvements (Dec 2025):**

- Fixed race conditions in static mutable variables
- Implemented encrypted storage for chat history
- Added sequence numbers for replay attack protection
- Replaced random nonces with deterministic counters
- Removed fingerprint auto-accept vulnerability

---

## Threat Model

The application is designed to protect against the following threats:

### Protected Against

- **Eavesdropping**: All messages are encrypted end-to-end with AES-256-GCM, making them unreadable to anyone who intercepts the traffic.
- **Tampering**: GCM authentication tags detect any modification of messages in transit.
- **Replay Attacks**: Sequence numbers prevent attackers from replaying old messages.
- **Key Compromise**: Forward secrecy via X25519 ECDH ensures that past sessions remain secure even if long-term identity keys are compromised.
- **Downgrade Attacks**: Protocol version negotiation prevents forcing use of weaker protocols.
- **Data at Rest Compromise**: Chat history is encrypted with ChaCha20-Poly1305.
- **Nonce Reuse**: Counter-based nonces guarantee uniqueness.

### Assumptions

This security model makes the following assumptions:

- Users **verify fingerprints** on the first connection to prevent man-in-the-middle attacks.
- The operating system is **not compromised**.
- The application is used on a **trusted network** (e.g., a home LAN or a secure VPN).

### Key Handling & Persistence

- **Identity Keys**: Long-term RSA-2048 identity keys are generated locally and stored on disk. Future versions will support an encrypted keystore (Argon2 + AES-256).
- **Session Keys**: Ephemeral AES-256-GCM session keys are derived via X25519 ECDH + HKDF and are kept in memory only for the session lifetime.
- **Fingerprints**: The RSA public key fingerprint is SHA-256 over the PEM bytes in lowercase hex.

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

## Applied Security Fixes

### Phase 1: Immediate Hardening (Dec 2025)

#### 1. Denial of Service (DoS) Protection

- **Rate Limiting**: Implemented a global rate limiter to reject excessive connections from a single IP.
- **Handshake Timeouts**: All handshake steps are wrapped in strict timeouts to prevent Slowloris attacks.
- **Chunked Reads**: Replaced large buffer pre-allocation with chunked steraming reads to prevent memory exhaustion attacks.
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

---

## Remaining Security Work

### 🏃 Phase 3: Long-term Cryptographic Hygiene (Future)

1. **Session Key Rotation**:
    - **Task**: Automatically re-negotiate the AES session key periodically.
    - **Why**: Improves long-term security by limiting the amount of data exposed if a single session key is ever compromised.

2. **Professional Security Audit**:
    - **Task**: Engage a third-party firm to perform a professional cryptographic review of the codebase.
    - **Why**: Provides expert, unbiased validation of the application's security.

---

## Reporting Security Issues

### Responsible Disclosure

If you discover a security vulnerability, please **DO NOT** open a public GitHub issue. Instead:

1. **Email:** Report to `[YOUR_SECURITY_EMAIL_ADDRESS_HERE]` (replace with actual address)
2. **Include:**
   - Clear description of the issue and potential impact
   - Steps to reproduce with proof-of-concept if available
   - Affected versions/commits and environment details
   - Any mitigation ideas you may have

3. **Response Time:** We will acknowledge within 48 hours and provide updates on fix timeline

4. **Disclosure:** We follow coordinated disclosure - please allow us time to fix before public disclosure

### Security Hall of Fame

We recognize security researchers who responsibly disclose vulnerabilities:

- *Your name could be here!*

### Bug Bounty

Currently, we do not offer a formal bug bounty program. However, we deeply appreciate security contributions and will acknowledge your work.
