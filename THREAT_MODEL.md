# Threat Model - Secure P2P Chat Application

**Version:** 1.0
**Last Updated:** January 24, 2026
**Application:** Encrypted P2P Messenger v1.7.4+

---

## Table of Contents

1. [Executive Summary](#executive-summary)
2. [Assets to Protect](#assets-to-protect)
3. [Attack Surface Analysis](#attack-surface-analysis)
4. [Threat Actors](#threat-actors)
5. [Threat Scenarios](#threat-scenarios)
6. [Mitigations & Protections](#mitigations--protections)
7. [Known Limitations](#known-limitations)
8. [Future Improvements](#future-improvements)

---

## Executive Summary

> [!NOTE]
> **tl;dr**: The Encrypted P2P Messenger is designed to protect communication against network eavesdropping, man-in-the-middle (MITM) attacks, and replay attacks. It assumes a compromised network but a secure local device. Identities are verified out-of-band to establish a root of trust.

This threat model documents the security architecture of the Encrypted P2P Messenger, a Rust-based point-to-point chat application using military-grade cryptography. The application prioritizes **end-to-end encryption**, **forward secrecy**, and **metadata privacy** to protect users' communications from unauthorized access.

**Key Design Principles:**

- All messages encrypted in transit (AES-256-GCM)
- All messages encrypted at rest (ChaCha20-Poly1305)
- Ephemeral session keys with forward secrecy (X25519 ECDH)
- Identity verification via fingerprint (TOFU model)
- No central server or trust authority
- Open-source code for transparency and auditability

---

## Assets to Protect

### Primary Assets

1. **Message Content**
   - **Value**: User communications contain sensitive information
   - **Threat**: Unauthorized access by eavesdroppers, compromised peers, or network attackers
   - **Protection**: AES-256-GCM encryption in transit, ChaCha20-Poly1305 at rest

2. **User Identity & Authentication**
   - **Value**: RSA-2048 public key fingerprints establish trust relationships
   - **Threat**: MITM attacks, identity spoofing, key substitution
   - **Protection**: Fingerprint verification, TOFU model, protocol-level signature binding

3. **Session Keys**
   - **Value**: Ephemeral AES-256-GCM session keys encrypt all message traffic
   - **Threat**: Long-term compromise of session keys via key theft or weak derivation
   - **Protection**: X25519 ECDH + HKDF-SHA256, keys kept in memory only, forward secrecy

4. **Chat History**
   - **Value**: Persistent record of past conversations
   - **Threat**: Disk-based compromise, unencrypted storage, accidental exposure
   - **Protection**: ChaCha20-Poly1305 encryption, password-protected keystore (planned)

5. **User Identity Keys**
   - **Value**: RSA-2048 long-term identity keys used to sign sessions
   - **Threat**: Theft from disk, side-channel extraction, weak entropy
   - **Protection**: Encrypted at rest with a password-derived key (Argon2 + ChaCha20-Poly1305), `zeroize` on drop

6. **Metadata (Connection Patterns)**
   - **Value**: Information about *who* talks to *whom* and *when*
   - **Threat**: Traffic analysis, IP-based location tracking, communication pattern inference
   - **Protection**: Protocol v3 ECDH-first design hides identities in plaintext, TCP encryption hides packet timing

---

## Attack Surface Analysis

### Entry Points & Threat Vectors

#### 1. Network Layer (TCP)

**Threats:**

- Passive eavesdropping on cleartext protocol negotiation
- Active MITM attacks during handshake
- Denial of service (slowloris, connection flooding)
- Packet replay attacks
- Traffic analysis (packet timing, sizes)

**Controls:**

- Protocol version negotiation (fallback prevention)
- ECDH-first encrypted tunnel (identity privacy)
- Sequence number-based replay protection
- Rate limiting & handshake timeouts
- Counter-based nonces (prevent reuse)

#### 2. Cryptographic Layer (Encryption/Signing)

**Threats:**

- Weak key derivation function
- Nonce reuse (catastrophic GCM failure)
- Lack of authenticated encryption (tampering not detected)
- Weak identity signature binding
- Downgrade to weaker crypto (RSA vs. modern alternatives)

**Controls:**

- HKDF-SHA256 for key derivation (industry standard)
- Atomic counters + session IDs prevent nonce reuse
- AES-GCM + ChaCha20-Poly1305 provide authenticated encryption
- Protocol v3 signature binding ephemeral keys to identity
- Planned: Ed25519 migration (more robust than RSA-2048)

#### 3. Identity & Trust Management

**Threats:**

- Key substitution / MITM without fingerprint verification
- Compromise of long-term identity keys
- Fingerprint spoofing or collision (SHA-256 >> 64-bit display)
- Weak fingerprint display (user confusion)

**Controls:**

- Fingerprint verification enforced (TOFU)
- 64-bit fingerprint display (risk vs. usability trade-off)
- RSA public key stored locally (no central CA)
- User-driven acceptance model (users decide trust)
- Planned: Formal certificate pinning for known contacts

#### 4. Local Filesystem

**Threats:**

- Unencrypted identity key on disk
- Unencrypted chat history
- Temporary files with plaintext data
- Residual memory after application exit

**Controls:**

- Chat history encrypted with ChaCha20-Poly1305
- Identity keys encrypted at rest with a password-derived key (Argon2 + ChaCha20-Poly1305)
- `zeroize` crate used to securely wipe secrets from memory
- No temporary plaintext files created
- Planned: Argon2 + AES-256 keystore for identity keys

#### 5. User Interface & UX

**Threats:**

- Password disclosure on screen during entry
- Cached passwords in memory
- Weak password validation
- Accidental message leaks (clipboard, screenshot history)
- Social engineering (user tricks sender)

**Controls:**

- Blocking password gate (cannot use app without unlock)
- Password-protected identity keys (mandatory)
- No weak password acceptance
- TBD: Clipboard auto-clear after paste
- TBD: Screenshot detection / warning

#### 6. Dependency Chain

**Threats:**

- Malicious or vulnerable transitive dependencies
- Supply chain attacks (compromised package registries)
- Outdated crates with known CVEs
- Unmaintained dependencies

**Controls:**

- `cargo-deny` audits all dependencies
- GitHub Actions audit checks (RUSTSEC database)
- Regular dependency updates
- Minimal dependency footprint
- Trusted cryptographic libraries (aes-gcm, x25519-dalek)

---

## Threat Actors

### 1. Passive Eavesdropper

**Defenses**:

- ✅ All messages encrypted end-to-end (AES-256-GCM)
- ✅ Protocol v3 hides identities in plaintext protocol
- ⚠️ Cannot fully hide traffic patterns without onion routing

### 2. Active Network Attacker (MITM)

**Defenses**:

- ✅ GCM authentication detects tampering
- ✅ Fingerprint verification prevents key substitution
- ✅ Protocol v3 signature binding prevents MITM
- ✅ Sequence numbers prevent replay attacks
- ✅ Protocol version negotiation prevents downgrade

### 3. Local Attacker (Device Compromise)

**Defenses**:

- ✅ Identity keys encrypted with password-derived key
- ✅ Chat history encrypted with ChaCha20-Poly1305
- ✅ Session keys kept in memory only (forward secrecy)
- ✅ Code is open-source (auditability)
- ❌ Cannot prevent true local compromise (OS-level threat)

### 4. Malicious Peer (Known Contact)

**Defenses**:

- ✅ GCM authentication prevents message forgery
- ✅ You can block/remove contacts
- ✅ Rate limiting prevents flood attacks
- ⚠️ Cannot prevent social engineering

---

## Threat Scenarios

### Scenario 1: Passive ISP Eavesdropping

- ✅ **MITIGATED**: All messages encrypted with AES-256-GCM.

### Scenario 2: MITM Attack During First Contact

- ✅ **MITIGATED**: Fingerprint verification and signature binding.

### Scenario 3: Session Key Compromise

- ✅ **MITIGATED**: Forward secrecy via X25519 ECDH.

### Scenario 4: Replay Attack

- ✅ **MITIGATED**: Sequence numbers in protocol.

### Scenario 5: Chat History Theft from Disk

- ✅ **MITIGATED**: Chat history encrypted with ChaCha20-Poly1305.

### Scenario 6: Identity Key Theft

- ✅ **MITIGATED**: Identity key is encrypted at rest with Argon2 + ChaCha20-Poly1305.

---

## Mitigations & Protections

### Cryptographic Controls

| Control | Mechanism | Strength |
|---------|-----------|----------|
| **Message Encryption** | AES-256-GCM | 256-bit symmetric |
| **Key Derivation** | HKDF-SHA256 | KDF-approved |
| **Key Exchange** | X25519 ECDH | 256-bit curve |
| **Identity Signature** | RSA-2048 / Ed25519 | Modern defaults |
| **Chat History** | ChaCha20-Poly1305 | 256-bit symmetric |

---

## Known Limitations

1. **Compromised Operating System**: Security is void if OS is backdoored.
2. **Weak User Passwords**: Subject to offline brute-force if file is stolen.
3. **Traffic Pattern Analysis**: Communication timing and size may leak metadata.
4. **Quantum Computers**: Current asymmetric primitives are vulnerable to Shor's algorithm (PQC planned).

---

## Future Improvements

1. **Formal Security Audit**: Engage independent cryptographic firm.
2. **Hardware Signing Support**: Integration with FIDO2/security keys.
3. **Onion Routing (Tor)**: Hide IP addresses from peers.
4. **Post-Quantum Cryptography (PQC)**: Hybrid classical/PQC handshake.

---

## Document History

| Version | Date | Changes |
|---------|------|---------|
| 1.0 | Jan 24, 2026 | Initial threat model; comprehensive scenarios |
