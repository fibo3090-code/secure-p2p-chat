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

## Remaining Larger Gaps

These are still roadmap items rather than resolved findings:

- internet-grade connectivity
- stronger discovery privacy model
- richer diagnostics and support tooling

## How to Use This File

Add new audit summaries here only after they have been checked against the current codebase and linked back to maintained docs:

- [SECURITY.md](../SECURITY.md)
- [THREAT_MODEL.md](../THREAT_MODEL.md)
- [docs/05_platform_spec.md](05_platform_spec.md)
