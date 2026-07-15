# Audit History

This document consolidates the repository’s audit-oriented notes so that stale one-off reports do not drift away from the current implementation.

## Purpose

- preserve historical audit findings
- keep a record of important quality/security reviews
- avoid leaving stale standalone reports that contradict current code

## Consolidated Historical Notes

### Earlier repository state

Earlier audit notes praised several areas correctly:

- modular Rust structure
- strong cryptographic direction
- good test coverage for core logic
- serious attention to secure storage and replay protection

They also overstated some things that were true only temporarily or were later found to drift:

- documentation consistency
- CI/CD presence in the checked-in repo
- protocol claims around Ed25519 support
- lint/test guarantees as a permanent repo property

Those claims have now been normalized into the maintained docs instead of preserved as “frozen praise.”

### Key issues identified and since addressed

- false Ed25519 negotiation in the runtime handshake
- undeployed AAD usage in critical protocol paths
- broken remove-password flow
- destructive clear-data flow not matching its label
- inconsistent address parsing
- stale and contradictory docs
- missing checked-in CI
- duplicate low-level transfer code path
- UI-thread autosave behavior

### 2026 — desktop-integration & Party audit

Findings from auditing the Tauri desktop app and the Party server against the
shared core (all fixed unless noted; verified against current code):

- **Shared identity between egui and the Tauri app.** The desktop bridge resolved
  its data dir from egui's `ProjectDirs("com","chat-p2p","EncryptedMessenger")`, so
  both loaded the same `identity.json` and became the same peer — connecting them on
  one machine was a self-connection. Fixed: the desktop app uses `ProjectDirs
  "P2PEM"` with a `P2PEM_DATA_DIR` override.
- **Host auto-trusted every incoming peer.** An incoming chat pre-filled its
  `peer_fingerprint`, so the TOFU check trivially matched and silently trusted every
  caller. Fixed: incoming chats start `peer_fingerprint: None`; returning peers are
  recognized by fingerprint across chats/contacts. (See `SECURITY.md` /
  `protocol.md` TOFU notes; regression tests added.)
- **Desktop bridge never persisted history.** Unlock loaded history but nothing saved
  it. Fixed with autosave (poll loop + per-mutation + on-close).
- **Party `DownloadFile` had no access control** — any member could fetch any blob by
  hash. Fixed with `blob_bytes_for(member, hash)` (channel-member / DM-participant
  check); covered by server `state.rs` tests.

## Remaining Larger Gaps

These are still roadmap items rather than resolved findings:

- internet-grade connectivity
- stronger discovery privacy model
- richer diagnostics and support tooling

## How to Use This File

Add new audit summaries here only after they have been checked against the current codebase and linked back to maintained docs:

- [SECURITY.md](../SECURITY.md)
- [THREAT_MODEL.md](../THREAT_MODEL.md)
- [docs/platform_spec.md](platform_spec.md)
