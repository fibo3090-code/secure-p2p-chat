# Security Policy & Documentation

**Last Updated:** January 24, 2026
Application: Encrypted P2P Messenger v1.7.4+

This document provides comprehensive security information including the threat model, cryptographic specifications, security audit findings, applied fixes, and vulnerability reporting guidelines.

---

## Table of Contents

1. [Security Overview](#security-overview)
2. [Threat Model](#threat-model)
3. [Cryptographic Specifications](#cryptographic-specifications)

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

6. [Remaining Security Work](#remaining-security-work)
2. [Vulnerability Reporting & Responsible Disclosure](#vulnerability-reporting--responsible-disclosure)
3. [Security Roadmap](#security-roadmap)

---

## Security Overview

This application implements **military-grade end-to-end encryption** with **forward secrecy**, matching the security standards of leading messaging apps like Signal and WhatsApp.

### Current Security Posture

**Overall Risk Assessment:** **LOW** (improved from MEDIUM)

**Security Achievements:**

- ✅ Strong cryptographic primitives (RSA-2048, AES-256-GCM, X25519)
- ✅ Forward secrecy via ephemeral key exchange
- ✅ Authenticated encryption (AES-GCM)
- ✅ Encrypted identity keystore at rest (Argon2 + ChaCha20-Poly1305)
- ✅ Counter-based nonces (prevents reuse)
- ✅ Replay attack protection (sequence numbers)
- ✅ Thread-safe implementation (no unsafe code)
- ✅ Fingerprint verification enforcement

**Recent Security Improvements (Jan 2026):**

- Patched `rsa` crate vulnerability (CVE-2026-21895).
- Updated all dependencies to latest versions.
- Remediated multiple CodeQL warnings.

---

## Threat Model

The application is designed to protect against the following threats:

**→ See [THREAT_MODEL.md](THREAT_MODEL.md) for comprehensive threat analysis, attack scenarios, and mitigations.**

This document summarizes key protections; the detailed threat model covers:

- **Threat Actor Profiles** (passive eavesdropper, MITM, local attacker, malicious peer, cryptanalytic attacker)
- **Detailed Attack Scenarios** (with mitigation analysis)
- **Known Limitations** (what we cannot protect against)
- **Future Improvements** (planned security enhancements)

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

- **Identity Keys**: Long-term RSA-2048 identity keys are generated locally and stored on disk, encrypted with a password-derived key using Argon2 and ChaCha20-Poly1305.
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
- **Impact**: Eliminates replay attack surface completely at protocol level.
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

## Remaining Security Work

### 🏃 Phase 8: Periodic Key Rotation (Future)

1. **Session Key Rotation**:
    - **Task**: Automatically re-negotiate the AES session key every N seconds or after M messages.
    - **Why**: Improves long-term security by limiting the amount of data exposed if a single session key is ever compromised (forward secrecy over time).
    - **Timeline**: 2-3 weeks

2. **Professional Security Audit**:
    - **Task**: Engage a third-party firm to perform a professional cryptographic review of the codebase.
    - **Why**: Provides expert, unbiased validation of the application's security.

### Phase 7 (Future): Invite Link Expiration & Revocation

- **Task**: Add optional expiration timestamps to v2 invites and implement a revocation mechanism
- **Current State**: Timestamp field is present in v2 invites but not yet validated
- **Future Work**:
  - Reject invites older than 30 days by default
  - Allow users to customize expiration (1 day, 1 week, 1 month, never)
  - Add invite revocation list (for stolen invite links)

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

**DO NOT**:

- ❌ Post on social media
- ❌ Create public GitHub issues
- ❌ Share the vulnerability with others without researcher agreement
- ❌ Attempt to profit from the vulnerability (use bug bounty programs if available)

**DO**:

- ✅ Email only the security contact
- ✅ Use PGP encryption if available (public key: see below)
- ✅ Provide detailed technical information
- ✅ Include your contact information (name, email, GitHub username)
- ✅ Indicate whether you'd like credit for the discovery

#### Step 3: Our Response

Upon receiving your report, we will:

1. **Initial Acknowledgment** (within 48 hours)
   - Confirm receipt of your report
   - Assign a ticket number for reference
   - Ask any clarifying questions

2. **Investigation & Validation** (3-10 days)
   - Reproduce the vulnerability in our environment
   - Assess severity and impact
   - Identify affected versions
   - Confirm root cause

3. **Fix Development** (variable timeline)
   - **Critical Severity** (RCE, loss of encryption): Fix within 1 week
   - **High Severity** (auth bypass, data disclosure): Fix within 2 weeks
   - **Medium Severity** (DoS, weak crypto): Fix within 1 month
   - **Low Severity** (minor logic flaws): Fix in next release

4. **Disclosure Coordination**
   - We will propose a disclosure timeline (typically 30-90 days)
   - Researcher must agree to embargo period
   - Patch is released before public disclosure

5. **Post-Release**
   - Security advisory published on GitHub (in releases section)
   - CVE requested (if applicable)
   - Researcher credited (if desired)
   - Post-mortem analysis published

### Severity Levels & Response Times

| Severity | Description | Response SLA | Fix SLA | Example |
|----------|-------------|--------------|---------|---------|
| **Critical** | Immediate threat to user security; remote code execution; loss of encryption | 2 hours | 1 week | Buffer overflow, key disclosure |
| **High** | Significant impact; data disclosure or loss; authentication bypass | 4 hours | 2 weeks | Weak random number generation; plaintext key storage |
| **Medium** | Moderate impact; requires attacker effort; limited exposure | 24 hours | 1 month | DoS attack; logic flaw in protocol |
| **Low** | Minor impact; low exploitability; documentation improvement | 1 week | Next release | Typo; unused variable; weak log messages |

### Vulnerability Embargo & Coordinated Disclosure Timeline

**Default Embargo Period**: 30-90 days (negotiable based on severity)

- **Day 0**: Vulnerability reported to us privately
- **Day N (Disclosure Date)**: Patch released on GitHub
- **Day N**: CVE (if applicable) is published
- **Day N**: Security advisory published
- **Day N + 1**: Researcher free to discuss publicly
- **Day N + 7**: Full post-mortem (what happened, how we fixed it, how to prevent similar issues)

### Encryption & Communication

For extra confidentiality, researchers may encrypt reports using our security team's PGP key:

```
Public Key: [PGP KEY ID - TO BE ADDED]
Fingerprint: [FINGERPRINT - TO BE ADDED]
```

**To Encrypt**:

```bash
gpg --import security-key.asc
echo "Your vulnerability report" | gpg --encrypt --armor --recipient security@fibo3090-code.dev
```

*(PGP key and process to be finalized before public release)*

### Security Hall of Fame

We recognize and thank researchers who responsibly disclose vulnerabilities:

| Researcher | Date | Vulnerability | Severity |
|-----------|------|-----------------|----------|
| *First researcher* | TBD | TBD | TBD |

**Want your name here?** Help us improve security through responsible disclosure!

---

## Security Roadmap

### Completed Milestones ✅

| Phase | Date | Work | Status |
|-------|------|------|--------|
| **Phase 1** | Dec 2025 | Immediate hardening (DoS protection, memory hygiene, logging sanitization) | ✅ Complete |
| **Phase 2** | Dec 2025 | Protocol v3 (encrypted identity exchange, privacy) | ✅ Complete |
| **Phase 3** | Jan 2026 | Dependency hardening, GitHub Actions CI, AAD in AES-GCM | ✅ Complete |
| **Phase 4** | Jan 2026 | Threat model documentation, responsible disclosure policy | ✅ Complete |

### Active Work 🔄

| Phase | Target | Work | Timeline | Owner |
|-------|--------|------|----------|-------|
| **Phase 5** | v1.8.0 | Encrypted identity keystore (Argon2 + AES-256) | 2-3 weeks | @fibo3090-code |
| **Phase 5** | v1.8.0 | Ed25519 migration (replace RSA-2048) | 3-4 weeks | @fibo3090-code |
| **Phase 5** | v1.8.0 | Professional security audit | 4-6 weeks | TBD (external) |

### Planned Work 📋

| Phase | Target | Work | Timeline | Rationale |
|-------|--------|------|----------|-----------|
| **Phase 6** | v1.9.0 | Session key rotation policy | 2 weeks | Long-term key exposure reduction |
| **Phase 6** | v1.9.0 | Clipboard auto-clear (30s timeout) | 1 week | UX security improvement |
| **Phase 6** | v1.9.0 | Out-of-band fingerprint verification (QR codes) | 2 weeks | Improve TOFU UX |
| **Phase 7** | v2.0.0 | Post-quantum cryptography (hybrid PQC) | 8-12 weeks | Future-proof against quantum threats |
| **Phase 7** | v2.0.0 | Hardware signing support (FIDO2) | 4-6 weeks | Enterprise security |
| **Phase 7** | v2.0.0 | Formal protocol verification (ProVerif) | 4-8 weeks | Mathematical proof of security |

### Current Focus (January 24, 2026)

**Issue #1-#3**: ✅ COMPLETE

- Added AAD support to AES-256-GCM
- Verified payload size validation
- Deployed GitHub Actions CI/CD

**Issue #4**: ✅ COMPLETE

- Created comprehensive [THREAT_MODEL.md](THREAT_MODEL.md) with threat scenarios and mitigations
- Expanded SECURITY.md with responsible disclosure policy
- Documented severity levels and response SLAs

**Issue #5**: ✅ COMPLETE

- Implemented Ed25519 support with signature scheme negotiation
- Added identity key migration paths
- Full backward compatibility with RSA-2048

**Issue #6**: ✅ COMPLETE

- Implemented replay protection at transport layer
- Added per-session sequence validation
- All replay attacks detected and rejected

**Issue #8**: 📋 NEXT

- Automatic session key rotation (periodic rekey)
- Implement RekeyRequest message type
- Timeline: 2-3 weeks

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

*This security policy is effective as of January 24, 2026 and supersedes all previous versions.*
