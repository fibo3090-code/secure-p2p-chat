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

In transit, sessions are established with X25519 ECDH and HKDF-SHA256 (providing forward secrecy), encrypted with AES-256-GCM under transcript-bound AAD, replay-protected by per-session sequence validation, and automatically rekeyed every 100 messages. Identity is a long-term RSA-2048 key used **only for RSA-PSS signatures** (identity proofs and signed invites) — never for encryption. At rest, the identity keystore is encrypted with Argon2 + ChaCha20-Poly1305 and chat history is encrypted, with zeroization applied to sensitive in-memory material where implemented. Full mechanics: [docs/protocol.md](docs/protocol.md).

Operational hardening includes handshake timeouts, rate limiting, DoS-hardened framing (bounded length prefix, chunked reads, oversized-packet rejection), signed invite links, a self-hosted relay mode that forwards only already-encrypted session traffic, and server-side access checks on Party file downloads (`blob_bytes_for`: only channel members or DM participants can fetch a blob). CI enforces formatting, lints, cross-platform tests, and locked build verification.

### Desktop app (Tauri) attack surface

The Tauri 2 desktop app (`p2pem-desktop`) adds a system-webview + IPC surface:

- all cryptography stays in Rust; the React webview only calls `#[tauri::command]`s and never handles key material
- `tauri.conf.json` sets a restrictive **CSP** (`default-src 'self'`; no external hosts, scripts, or fonts) and disables the global Tauri injection (`withGlobalTauri: false`)
- the desktop app uses its **own** data directory (`ProjectDirs "P2PEM"`), separate from the egui app, so the two are distinct identities rather than sharing one keystore
- the desktop crate has no automated tests; it is verified by `cargo check` + `npm run build`, so treat its UI logic as build-verified rather than test-covered

## Current Limits and Open Risks

- No STUN/TURN or peer-to-peer hole punching; WAN support relies on optional
  UPnP port mapping (off by default — it opens a router port and embeds the
  external IP in invites) or a self-hosted relay
- LAN discovery exposes metadata tradeoffs when enabled
- The runtime keeps a signature-scheme field on the wire, but currently only supports RSA-PSS identity proofs
- Signed invites expire 30 days after issuance (the signature covers the
  timestamp); legacy v1 unsigned invites carry no timestamp and are not
  subject to expiry
- Invite revocation (before expiry) is not supported
- `rsa` still carries an unresolved upstream timing-sidechannel advisory (`RUSTSEC-2023-0071`)
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
