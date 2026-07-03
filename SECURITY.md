# Security

**Last Updated:** July 3, 2026

This document describes the current security posture of the project, what protections are implemented today, what risks remain open, and how to report issues responsibly.

## Security Posture

Current overall risk assessment: **medium**.

Reasoning:

- strong modern transport/session primitives are in place
- identity and history are encrypted at rest
- protocol correctness and replay protection have been hardened
- some product-grade gaps remain, especially around discovery privacy, relay operational hardening, and dependency posture

## Implemented Protections

### In transit

- AES-256-GCM transport encryption
- X25519 ECDH for ephemeral session establishment
- HKDF-SHA256 for session key derivation
- transcript-bound AAD for encrypted identity proofs and transport packets
- replay protection using sequence validation
- transport rekeying

### Identity and storage

- RSA-2048 identity keys
- RSA-PSS identity proofs in the current runtime
- password-based identity encryption using Argon2 + ChaCha20-Poly1305
- encrypted chat history at rest
- zeroization for sensitive in-memory material where implemented

### Operational hardening

- handshake timeouts
- rate limiting
- oversized packet rejection (DoS-hardened framing: bounded length prefix, chunked reads)
- automatic session-key rotation (rekey every 100 messages)
- signed v2 invite links
- self-hosted relay-assisted transport that forwards only already-encrypted session traffic
- Party file downloads are access-checked server-side (`blob_bytes_for(member, hash)`): content-addressed blobs are deduplicated globally, so the download endpoint enforces that only channel members or DM participants can fetch a given file
- checked-in CI for format, lint, cross-platform tests, locked build verification, and tagged release packaging

### Desktop app (Tauri) attack surface

The new Tauri 2 desktop app (`p2pem-desktop`) adds a system-webview + IPC surface:

- all cryptography stays in Rust; the React webview only calls `#[tauri::command]`s and never handles key material
- `tauri.conf.json` sets a restrictive **CSP** (`default-src 'self'`; no external hosts, scripts, or fonts) and disables the global Tauri injection (`withGlobalTauri: false`)
- the desktop app uses its **own** data directory (`ProjectDirs "P2PEM"`), separate from the egui app, so the two are distinct identities rather than sharing one keystore
- the desktop crate has no automated tests; it is verified by `cargo check` + `npm run build`, so treat its UI logic as build-verified rather than test-covered

## Current Limits and Open Risks

- No STUN/TURN or peer-to-peer hole punching; WAN support currently relies on a self-hosted relay
- LAN discovery exposes metadata tradeoffs when enabled
- The runtime keeps a signature-scheme field on the wire, but currently only supports RSA-PSS identity proofs
- Invite timestamps are informational and not enforced for expiry
- `rsa` still carries an unresolved upstream timing-sidechannel advisory (`RUSTSEC-2023-0071`)
- `bincode` remains a tracked dependency migration concern

## Trust Model

The app uses TOFU.

Users are responsible for verifying fingerprints on first contact using a separate trusted channel. Without that step, first-contact MITM remains possible.

## What This App Does Not Claim

It does not currently claim:

- anonymity against traffic analysis
- managed relay infrastructure or anonymous global connectivity
- Ed25519 identity-key support in the shipped runtime
- invite revocation or expiry enforcement
- full protection against a compromised local machine

## Related Documents

- Threat assumptions and attack surfaces: [THREAT_MODEL.md](THREAT_MODEL.md)
- Protocol details: [docs/04_protocol.md](docs/04_protocol.md)
- Audit history: [docs/AUDITS.md](docs/AUDITS.md)

## Responsible Disclosure

Security issues should not be reported in public issues.

Preferred report contents:

- affected version
- reproduction steps
- impact
- affected components
- proof of concept if available
- mitigation ideas if known

Contact: `security@fibo3090-code.dev`  
Replace this with the real maintained address if the project adopts one.
