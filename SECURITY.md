# Security

**Last Updated:** July 13, 2026

This document describes the current security posture of the project, what protections are implemented today, what risks remain open, and how to report issues responsibly.

The division of labor across security documents:

- this file: posture summary, open risks, supported versions, and disclosure process
- [THREAT_MODEL.md](THREAT_MODEL.md): assumptions, assets, attack surfaces, and per-surface controls
- [docs/protocol.md](docs/protocol.md): the cryptographic mechanics of the shipped wire protocol
- [docs/AUDITS.md](docs/AUDITS.md): audit history and resolved findings

## Supported Versions

Only the latest released version receives security fixes.

| Version | Supported |
|---|---|
| Latest release (see [CHANGELOG.md](CHANGELOG.md)) | Yes |
| Older releases | No — upgrade to the latest release |

## Security Posture

Current overall risk assessment: **medium**.

Reasoning:

- strong modern transport/session primitives are in place
- identity and history are encrypted at rest
- protocol correctness and replay protection have been hardened
- some product-grade gaps remain, especially around discovery privacy, relay operational hardening, and dependency posture

## Implemented Protections

In transit, sessions are established with X25519 ECDH and HKDF-SHA256 (providing forward secrecy), encrypted with AES-256-GCM under transcript-bound AAD, replay-protected by per-session sequence validation, and automatically rekeyed every 100 messages (or 5 minutes). Rekeying is initiated by a single deterministic side (the host) so the two peers never rotate simultaneously, and the receiver keeps the previous key for a bounded window (until the first frame decrypts under the new key) so peer frames still in flight under the old key are not lost — together these keep a rotation from desyncing the keys and dropping the session. Identity is a long-term RSA-2048 key used **only for RSA-PSS signatures** (identity proofs and signed invites) — the product performs **no RSA encryption/decryption at all** (those functions were removed from the codebase). At rest, the identity keystore is encrypted with Argon2 + ChaCha20-Poly1305 and chat history is encrypted, with zeroization applied to sensitive in-memory material where implemented. Full mechanics: [docs/protocol.md](docs/protocol.md).

Operational hardening includes handshake timeouts, rate limiting, DoS-hardened framing (bounded length prefix, chunked reads, oversized-packet rejection), signed invite links, a self-hosted relay mode that forwards only already-encrypted session traffic, and server-side access checks on Party file downloads (`blob_bytes_for`: only channel members or DM participants can fetch a blob). CI enforces formatting, lints, cross-platform tests, and locked build verification.

### Desktop app (Tauri) attack surface

The Tauri 2 desktop app (`p2pem-desktop`) adds a system-webview + IPC surface:

- all cryptography stays in Rust; the React webview only calls `#[tauri::command]`s and never handles key material
- `tauri.conf.json` sets a restrictive **CSP** (`default-src 'self'`; no external hosts, scripts, or fonts) and disables the global Tauri injection (`withGlobalTauri: false`)
- the desktop app uses its **own** data directory (`ProjectDirs "P2PEM"`), separate from the egui app, so the two are distinct identities rather than sharing one keystore
- the desktop crate has no automated tests; it is verified by `cargo check` + `npm run build`, so treat its UI logic as build-verified rather than test-covered

## Current Limits and Open Risks

- WAN support relies on optional UPnP/NAT-PMP port mapping (off by default —
  it opens a router port and embeds the external IP in invites) or a
  self-hosted relay. The relay first coordinates a **TCP hole punch** (both
  peers learn each other's relay-observed public endpoint and attempt a
  simultaneous open from the reused source port), so most sessions go direct
  and the relay only bridges when punching fails (symmetric NAT, CGNAT,
  filtered networks). The punch hello tag is derived from the rendezvous
  token and is pairing hygiene only — authentication is still the v3
  handshake + TOFU on whichever socket wins. `P2PEM_NO_HOLEPUNCH=1` forces
  the bridged path. Punching also reveals each peer's public *and* LAN
  endpoint to the other (the relay already saw both source addresses)
- Multi-address invites embed **both** the external and the LAN address when
  UPnP is enabled, so a shared invite reveals the private LAN IP alongside the
  public one — share invites only with people you intend to reach you
- LAN discovery exposes metadata tradeoffs when enabled
- The runtime keeps a signature-scheme field on the wire, but currently only supports RSA-PSS identity proofs
- Signed invites expire 30 days after issuance (the signature covers the
  timestamp); legacy v1 unsigned invites carry no timestamp and are not
  subject to expiry
- Invite revocation (before expiry) is not supported
- **No post-compromise security (no double ratchet).** Forward secrecy comes
  from ephemeral-DH session keys plus symmetric rekeying (`next = HKDF(current,
  nonce)`). Because rekeying folds in no *new* DH material, an attacker who
  captures a live session key can follow the ratchet forward from that point
  (the nonces travel on the wire) until the session ends and a fresh ephemeral
  handshake runs. A Signal-style Double Ratchet (a DH step per ratchet) would
  add self-healing after key compromise; it is a deliberate, larger protocol
  change, not yet implemented.
- **TOFU has no key transparency.** Trust is pinned on first use and a later
  key change is surfaced loudly, but there is no external auditable log
  (CONIKS/Keybase-style) to detect a malicious key swap presented consistently
  to a victim from the start. Out-of-band fingerprint comparison remains the
  backstop.
- `rsa` is still a dependency (for RSA-PSS **signatures**), so the upstream
  timing advisory `RUSTSEC-2023-0071` still appears in audits — but the attack
  it describes targets RSA **decryption**, which this product does not perform
  (the RSA encrypt/decrypt functions were removed). Migrating identity
  signatures to Ed25519 (the wire already negotiates a scheme field) would drop
  `rsa` from the security-critical path entirely.
- `bincode` remains a tracked dependency migration concern

## Trust Model

The app uses TOFU (trust on first use).

Users are responsible for verifying fingerprints on first contact using a separate trusted channel. Without that step, first-contact MITM remains possible.

## What This App Does Not Claim

It does not currently claim:

- anonymity against traffic analysis
- managed relay infrastructure or anonymous global connectivity
- Ed25519 identity-key support in the shipped runtime
- invite revocation before the 30-day expiry
- full protection against a compromised local machine

## Responsible Disclosure

Security issues must **not** be reported in public issues.

Report vulnerabilities privately through [GitHub's private vulnerability reporting](https://github.com/fibo3090-code/secure-p2p-chat/security/advisories/new) for this repository.

Preferred report contents:

- affected version
- reproduction steps
- impact
- affected components
- proof of concept if available
- mitigation ideas if known
