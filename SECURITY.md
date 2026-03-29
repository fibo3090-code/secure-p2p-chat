# Security

**Last Updated:** March 29, 2026

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
- oversized packet rejection
- signed v2 invite links
- self-hosted relay-assisted transport that forwards only already-encrypted session traffic
- checked-in CI for format, lint, test, advisory audit, and Windows build validation

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
