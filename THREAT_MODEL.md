# Threat Model

This document describes the assumptions, assets, attack surfaces, and limitations used when reasoning about the security of Encrypted P2P Messenger.

## Assumptions

- The network can be observed and actively manipulated.
- The local operating system is not trusted if fully compromised.
- Users can verify fingerprints out of band when prompted.
- Peers may be malicious even if previously known.

## Assets

- message contents
- long-term identity keys
- session keys
- chat history
- contact trust state
- connection metadata

## Main Attack Surfaces

### Network handshake and transport

Risks:

- MITM during first contact
- replay and reordering
- downgrade or negotiation confusion
- denial of service

Controls:

- X25519 + HKDF session establishment
- transcript-bound authenticated encryption
- RSA-PSS identity proofs
- replay checks
- handshake timeouts and rate limiting

### Local storage

Risks:

- theft of identity or history files
- plaintext persistence
- dangerous path usage

Controls:

- encrypted identity keystore
- encrypted chat history
- path sanitization on loaded config
- atomic encrypted-history writes

### Discovery and metadata

Risks:

- LAN observers learning hostname, address, fingerprint, or presence
- traffic analysis
- relay operators learning connection metadata such as timing and endpoint usage

Controls:

- discovery disabled by default
- encrypted identity exchange after session establishment
- self-hosted relay forwards already-encrypted session traffic

Residual limitation:

- enabling mDNS still exposes local-network metadata tradeoffs
- relay use improves reachability, not metadata privacy

### Invite sharing and trust bootstrap

Risks:

- users importing forged or tampered invite data
- users trusting first contact without fingerprint verification

Controls:

- signed invite links in current UI flows
- invalid invite addresses are sanitized during import
- explicit fingerprint verification workflow

### Party server (Administered tier)

Risks:

- the server operator can read message/file contents (the Administered tier stores **plaintext** by design, to enable offline buffering, search, and simple groups)
- a member fetching files they should not see
- a malicious server impersonating a known server

Controls:

- the trust tier is a **server property**, shown to users; the operator wears an explicit "this operator can read messages" badge
- the client↔server channel uses the same v3 handshake, and the server has its own **TOFU-verified** identity/fingerprint (stable across restarts)
- file downloads are access-checked server-side (`blob_bytes_for(member, hash)`): only channel members or DM participants can fetch a blob, despite global content-addressed dedup

Residual limitation:

- the Administered tier is not end-to-end encrypted against the operator; the planned E2EE tier (per-channel group keys, ciphertext-only storage) is future work

### Desktop app webview / IPC

Risks:

- a system-webview + IPC boundary is a larger surface than a pure-Rust GUI
- webview content-injection or an over-permissive command surface

Controls:

- crypto and key material stay in Rust; the React UI only invokes commands
- a restrictive CSP (`default-src 'self'`, no external hosts) and `withGlobalTauri: false` in `tauri.conf.json`

## Threat Actors

### Passive observer

Goal:

- read messages or correlate peers

Mitigation:

- transport encryption and encrypted identity exchange

### Active network attacker

Goal:

- tamper with handshake or replay traffic

Mitigation:

- fingerprint verification
- transcript-bound authenticated encryption
- replay protection

### Local attacker

Goal:

- steal keys or history from disk or memory

Mitigation:

- encrypted at-rest storage
- zeroization in sensitive paths

Limitation:

- a compromised host OS defeats the trust boundary

### Malicious peer

Goal:

- abuse trust, flood, replay, or socially engineer

Mitigation:

- trust verification
- rate limiting
- session validation

Limitation:

- the app cannot prevent social engineering

### Party server operator

Goal:

- read or retain message/file contents on a server they run

Mitigation:

- honest trust-tier labeling (Administered = operator-readable) so users consent knowingly
- server-identity TOFU; per-download access checks

Limitation:

- in the Administered tier the operator can read stored plaintext; only the future E2EE tier removes this

## Known Security Limits

- TOFU requires user action to be meaningful
- no anonymity network or metadata-hiding transport
- no invite revocation or expiry enforcement
- no hardware-backed identity support
- no post-quantum protection

## Planning References

- Current posture and disclosure: [SECURITY.md](SECURITY.md)
- Future roadmap: [docs/05_platform_spec.md](docs/05_platform_spec.md)
- Audit history: [docs/AUDITS.md](docs/AUDITS.md)
