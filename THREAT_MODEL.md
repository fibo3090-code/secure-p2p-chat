# Threat Model - Secure P2P Chat Application

**Version:** 1.0  
**Last Updated:** January 24, 2026  
**Application:** Encrypted P2P Messenger v1.7.0+

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
   - **Protection**: File-based storage (encrypted in future), Argon2-protected (planned), `zeroize` on drop

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
- Identity keys stored unencrypted (password protection planned)
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
- `cargo-deny` license and CVE scanning in CI
- GitHub Actions audit checks (RUSTSEC database)
- Regular dependency updates
- Minimal dependency footprint
- Trusted cryptographic libraries (aes-gcm, x25519-dalek)

---

## Threat Actors

### 1. Passive Eavesdropper

**Capability**: Can observe all network traffic but cannot modify packets

**Threats**:
- Read message content if sent unencrypted
- Infer communication patterns from packet sizes/timing
- Correlate IP addresses to identify users

**Our Defenses**:
- ✅ All messages encrypted end-to-end (AES-256-GCM)
- ✅ Protocol v3 hides identities in plaintext protocol
- ⚠️ Cannot fully hide traffic patterns without onion routing (out of scope)

### 2. Active Network Attacker (MITM)

**Capability**: Can observe AND modify network traffic, perform protocol attacks

**Threats**:
- Inject fake messages
- Replace peer identity keys
- Downgrade cryptography
- Forge signatures
- Replay old messages

**Our Defenses**:
- ✅ GCM authentication detects tampering
- ✅ Fingerprint verification prevents key substitution
- ✅ Protocol v3 signature binding prevents MITM
- ✅ Sequence numbers prevent replay attacks
- ✅ Protocol version negotiation prevents downgrade

### 3. Local Attacker (Device Compromise)

**Capability**: Can read files, access memory, install backdoors

**Threats**:
- Steal identity private keys
- Read plaintext chat history
- Hijack active sessions
- Monitor user input (keylogging)
- Backdoor the application

**Our Defenses**:
- ⚠️ Identity keys unencrypted on disk (password protection planned)
- ✅ Chat history encrypted with ChaCha20-Poly1305
- ✅ Session keys kept in memory only (forward secrecy)
- ✅ Code is open-source (auditability)
- ❌ Cannot prevent true local compromise (OS-level threat)

### 4. Malicious Peer (Known Contact)

**Capability**: Can send arbitrary messages, observe your traffic patterns, attempt social engineering

**Threats**:
- Send malicious messages or files
- Observe conversation patterns
- Impersonate legitimate contacts (if credentials stolen)
- Denial of service (flood messages)

**Our Defenses**:
- ✅ GCM authentication prevents message forgery
- ✅ You can block/remove contacts
- ✅ Rate limiting prevents flood attacks
- ✅ Open communication—users aware of who they trust
- ⚠️ Cannot prevent social engineering (user education needed)

### 5. Attacker with Cryptanalytic Capability

**Capability**: Can perform large-scale cryptanalytic attacks, exploit weaknesses in primitives

**Threats**:
- Brute-force AES-256-GCM keys
- Collision attacks on SHA-256 fingerprints
- Weak random number generation
- Side-channel attacks on X25519

**Our Defenses**:
- ✅ AES-256-GCM: 256-bit key → 2^256 brute-force cost (infeasible)
- ✅ SHA-256: No known collisions (cryptographically secure)
- ✅ `getrandom` crate for cryptographic RNG
- ✅ X25519 constant-time operations (side-channel resistant)
- ⚠️ RSA-2048 approaching 112-bit security (planned: Ed25519 migration)

### 6. Supply Chain / Dependency Attacker

**Capability**: Can compromise an upstream dependency or crate registry

**Threats**:
- Inject malicious code into dependencies
- Create vulnerable versions
- Perform timing attacks or weak crypto

**Our Defenses**:
- ✅ `cargo-deny` audits all dependencies
- ✅ CI automatically checks for known CVEs
- ✅ Regular dependency updates
- ✅ Open-source allows code review
- ⚠️ Cannot fully prevent (ecosystem-wide risk)

---

## Threat Scenarios

### Scenario 1: Passive ISP Eavesdropping

**Attacker Goal**: Read user messages over ISP link

**Attack Flow**:
1. Attacker captures all TCP traffic between two peers
2. Attempts to decrypt messages

**Outcome**:
- ✅ **MITIGATED**: All messages encrypted with AES-256-GCM
- Messages remain confidential even if network is compromised

---

### Scenario 2: MITM Attack During First Contact

**Attacker Goal**: Intercept communication and impersonate peer

**Attack Flow**:
1. Attacker positions themselves on network path between Alice and Bob
2. Attacker creates two sessions: Alice↔Attacker, Attacker↔Bob
3. Attacker forwards messages while reading plaintext

**Outcome**:
- ✅ **MITIGATED** (mostly): Fingerprint verification prevents this
- When Bob's identity arrives, Alice compares fingerprint
- If Alice verifies fingerprint with Bob out-of-band (phone, email), attack fails
- ⚠️ **RISK**: If Alice doesn't verify fingerprint, MITM succeeds
- **Mitigation**: User education, stronger UX for fingerprint checking

---

### Scenario 3: Session Key Compromise (Without Forward Secrecy)

**Attacker Goal**: Decrypt past messages by compromising session key

**Attack Flow**:
1. Attacker steals session key from Alice's memory (malware)
2. Attacker uses key to decrypt captured past messages

**Outcome**:
- ✅ **MITIGATED**: Forward secrecy via X25519 ECDH
- Old session keys are NOT recoverable from compromised long-term keys
- Only future sessions from compromise point are at risk

---

### Scenario 4: Replay Attack

**Attacker Goal**: Repeat a previously sent message to mislead recipient

**Attack Flow**:
1. Attacker captures an old encrypted message
2. Attacker replays it to the peer at a later date

**Outcome**:
- ✅ **MITIGATED**: Sequence numbers in protocol
- Replayed message has old sequence number
- Recipient detects duplicate/out-of-order and rejects

---

### Scenario 5: Chat History Theft from Disk

**Attacker Goal**: Read historical chat messages from compromised disk

**Attack Flow**:
1. Attacker gains physical or filesystem access to Alice's computer
2. Attacker copies the chat history database

**Outcome**:
- ✅ **MITIGATED** (partially): Chat history encrypted with ChaCha20-Poly1305
- Attacker has encrypted blobs without decryption key
- ⚠️ **RISK**: Key stored on disk in plaintext (planned: password protection)
- **Mitigation**: Use password-protected keystore (Argon2 + AES-256)

---

### Scenario 6: Identity Key Theft

**Attacker Goal**: Impersonate Alice by stealing her long-term RSA key

**Attack Flow**:
1. Attacker gains filesystem access and copies `identity/alice_privkey.pem`
2. Attacker uses key to sign sessions and impersonate Alice

**Outcome**:
- ✅ **MITIGATED** (planned): Password-protected key storage
- Currently: Identity key on disk in plaintext (HIGH RISK)
- Future: Argon2 + AES-256 encryption with password
- **Current Workaround**: Store identity file on encrypted partition

---

### Scenario 7: Denial of Service (Connection Flood)

**Attacker Goal**: Crash or disable the application via network overload

**Attack Flow**:
1. Attacker sends thousands of connection requests to target
2. Application runs out of resources (memory, file descriptors)

**Outcome**:
- ✅ **MITIGATED**: Rate limiting per IP address
- Excessive connections from single IP rejected
- Global connection limit prevents resource exhaustion

---

### Scenario 8: Cryptanalytic Attack on AES-256-GCM

**Attacker Goal**: Decrypt messages by breaking AES-256-GCM

**Attack Flow**:
1. Attacker attempts cryptanalytic attack (brute force, key derivation weakness, nonce reuse)

**Outcome**:
- ✅ **MITIGATED**: 
  - 256-bit key space → 2^256 brute-force (infeasible, ~10^77 operations)
  - HKDF-SHA256 is cryptographically sound
  - Atomic counter + session ID prevent nonce reuse
- **Security Level**: 256-bit symmetric encryption ≈ 128-bit asymmetric equivalence

---

## Mitigations & Protections

### Cryptographic Controls

| Control | Mechanism | Strength |
|---------|-----------|----------|
| **Message Encryption** | AES-256-GCM | 256-bit symmetric (128-bit equiv.) |
| **Session Key Derivation** | HKDF-SHA256 | KDF-approved, cryptographically sound |
| **Key Exchange** | X25519 ECDH | 256-bit, elliptic curve DH (128-bit equiv.) |
| **Identity Signature** | RSA-2048 + SHA256 | 112-bit security (planned: Ed25519) |
| **Fingerprint** | SHA-256 over PEM | Collision-resistant, 64-bit display |
| **Chat History Encryption** | ChaCha20-Poly1305 | 256-bit symmetric (128-bit equiv.) |

### Protocol Controls

| Control | Mechanism | Protects Against |
|---------|-----------|------------------|
| **Sequence Numbers** | Per-session counter | Replay attacks |
| **Nonce Uniqueness** | Atomic counter + session ID | GCM nonce reuse |
| **Handshake Signature** | Peer signs ephemeral key | MITM key substitution |
| **GCM Authentication Tags** | Per-message MAC | Tampering detection |
| **Protocol v3 Design** | ECDH-first encrypted tunnel | Identity metadata leakage |
| **Fingerprint Verification** | TOFU model | Key substitution |

### Operational Controls

| Control | Mechanism | Protects Against |
|---------|-----------|------------------|
| **Rate Limiting** | Per-IP connection limit | DoS attacks |
| **Handshake Timeouts** | 30s timeout per step | Slowloris attacks |
| **Chunked Reading** | Stream-based I/O | Memory exhaustion DoS |
| **Dependency Auditing** | `cargo-deny` + CI checks | Supply chain attacks |
| **Code Review** | Open-source model | Logic vulnerabilities |
| **Memory Sanitization** | `zeroize` on drop | Key exposure in dumps |

### User Controls

| Control | Mechanism | Protects Against |
|---------|-----------|------------------|
| **Password Gate** | Blocking auth screen | Unattended device abuse |
| **Fingerprint Display** | 64-bit hex | User manual verification |
| **Contact Management** | Block/remove features | Unwanted communication |
| **Session Visibility** | Show active peers | Unknown session hijacking |

---

## Known Limitations

### Cannot Protect Against

1. **Compromised Operating System**
   - If the OS is infected with malware, all security is void
   - An attacker with kernel-level access can read memory, intercept keystrokes, etc.
   - **Mitigation**: Use trusted, hardened OS; disable kernel exploits

2. **Weak User Passwords**
   - If user chooses a weak password, it can be brute-forced
   - **Mitigation**: Enforce minimum password length, educate users

3. **Social Engineering**
   - An attacker can trick a user into accepting a malicious fingerprint
   - **Mitigation**: User education, out-of-band fingerprint verification

4. **Traffic Pattern Analysis**
   - While message content is encrypted, the pattern of communication (who talks to whom, when) may leak information
   - **Mitigation**: Onion routing (Tor integration), dummy messages (not implemented)

5. **Malicious Peer**
   - A trusted contact can still attempt social engineering or send harmful content
   - **Mitigation**: User awareness, ability to block/report contacts

6. **Quantum Computers**
   - Future large-scale quantum computers could break current public-key cryptography (RSA, ECDH)
   - **Mitigation**: Planned hybrid PQC (post-quantum cryptography) support in future versions

7. **Side-Channel Attacks**
   - Timing, power, or cache attacks on cryptographic operations
   - **Mitigation**: Use constant-time libraries (x25519-dalek); avoid unsafe code

8. **Unencrypted Identity Keys (Current)**
   - Long-term RSA identity keys are stored in plaintext on disk
   - **Current Impact**: HIGH RISK for compromised systems
   - **Mitigation (Planned)**: Encrypt with Argon2 + AES-256

---

## Future Improvements

### High Priority (Next Release)

1. **Encrypted Identity Keystore**
   - Use Argon2 KDF + AES-256 to encrypt private keys at rest
   - Prevent key theft from compromised filesystem
   - **Estimated Impact**: Eliminates identity key theft threat

2. **Signature Algorithm Migration**
   - Replace RSA-2048 with Ed25519 (smaller, faster, more robust)
   - Plan dual-mode handshake for backward compatibility
   - **Estimated Impact**: Improves cryptographic agility, reduces key size

3. **Formal Security Audit**
   - Engage independent cryptographic audit firm
   - Review handshake, key derivation, protocol implementation
   - **Estimated Impact**: Professional validation of design and implementation

### Medium Priority

4. **Session Key Rotation**
   - Periodically re-negotiate session keys during long sessions
   - Reduce data exposure if a single key is compromised
   - **Impact**: Defense-in-depth against session key compromise

5. **Clipboard Protection**
   - Auto-clear clipboard after 30 seconds of paste
   - Prevent accidental message leaks
   - **Impact**: User operational security

6. **Out-of-Band Fingerprint Verification**
   - Built-in QR code exchange for fingerprints
   - One-click verification via phone
   - **Impact**: Improve user experience of TOFU verification

### Lower Priority (Future)

7. **Onion Routing Integration (Tor)**
   - Route connections through Tor for additional privacy
   - Hide IP addresses from peers
   - **Impact**: Defend against traffic analysis and IP-based tracking

8. **Post-Quantum Cryptography (PQC)**
   - Hybrid classical/PQC handshake (CRYSTALS-Kyber, CRYSTALS-Dilithium)
   - Prepare for future quantum threats
   - **Impact**: Long-term cryptographic security

9. **Formal Protocol Verification**
   - Use ProVerif or similar tool to formally verify Protocol v3
   - Mathematically prove security properties
   - **Impact**: High confidence in protocol design

10. **Hardware Signing Support**
    - Integration with hardware wallets / security keys (FIDO2)
    - Isolate identity key to secure enclave
    - **Impact**: Enterprise-grade security

---

## Security Recommendations for Users

### Best Practices

1. **Verify Fingerprints Out-of-Band**
   - When connecting to a new contact, verify fingerprint via phone, video, or in-person
   - Do NOT rely solely on fingerprint shown in app

2. **Use Strong, Unique Passwords**
   - Minimum 12 characters, mix of upper/lower/digits/special
   - Different password for each application

3. **Keep Software Updated**
   - Update the application and OS regularly
   - Security patches are frequently released

4. **Use Trusted Networks**
   - Avoid public WiFi for sensitive communications
   - Use a VPN or home network when possible

5. **Physical Security**
   - Lock your computer when away
   - Encrypt your disk (BitLocker, FileVault, LUKS)
   - Don't leave devices unattended in untrusted locations

6. **Monitor Fingerprints**
   - Periodically re-check contact fingerprints in the app
   - Alert if a fingerprint suddenly changes (possible MITM)

---

## Document History

| Version | Date | Changes |
|---------|------|---------|
| 1.0 | Jan 24, 2026 | Initial threat model; comprehensive threat scenarios and mitigations |

---

## Feedback & Contact

Questions about this threat model or security issues?

- **Security Report**: See SECURITY.md for responsible disclosure guidelines
- **General Feedback**: Open an issue on GitHub
- **Comments**: Contact the development team

---

*This threat model is a living document and will be updated as the application evolves and new threats emerge.*
