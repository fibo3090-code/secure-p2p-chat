# Developer Guide

This document is the contributor-facing guide for building, testing, changing, and releasing the project. It intentionally avoids duplicating full architecture and protocol detail; those live in the dedicated docs under [docs/README.md](docs/README.md).

## Documentation Structure

- Product overview: [README.md](README.md)
- Guided onboarding: [docs/TUTORIAL.md](docs/TUTORIAL.md)
- User workflows: [docs/USER_GUIDE.md](docs/USER_GUIDE.md)
- Architecture: [docs/03_architecture.md](docs/03_architecture.md)
- Protocol: [docs/04_protocol.md](docs/04_protocol.md)
- Security posture: [SECURITY.md](SECURITY.md)
- Threat assumptions: [THREAT_MODEL.md](THREAT_MODEL.md)
- Plan, roadmap & backlog: [docs/05_platform_spec.md](docs/05_platform_spec.md)
- Audit history: [docs/AUDITS.md](docs/AUDITS.md)

## Toolchain

- Rust: `1.86+`
- Edition: `2021`
- Main entry points:
  - GUI/TUI launcher: `src/main.rs`
  - Library root: `src/lib.rs`
  - App coordinator: `src/app/chat_manager.rs`

## Build and Run

### Build

```bash
cargo build
cargo build --release
```

### Run GUI

```bash
cargo run --release
```

### Run TUI

```bash
cargo run --release -- --tui
```

### Useful Launch Variants

```bash
cargo run --release -- --tui --host --port 9000
cargo run --release -- --tui --connect 127.0.0.1:12345
```

## Quality Gates

Run these before opening a PR:

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

The repository also includes CI in `.github/workflows/ci.yml` for:

- formatting
- clippy
- tests on Ubuntu, Windows, and macOS
- locked Linux build verification

The tag-based release workflow in `.github/workflows/release.yml` builds and publishes:

- Windows installer
- Linux tarball
- macOS Intel DMG
- macOS Apple Silicon DMG

## Code Map

### Core areas

- `src/app/`
  - `chat_manager.rs`: application state, routing, chat/contact/session operations
  - `persistence.rs`: encrypted history load/save, migration compatibility
- `src/core/`
  - `crypto.rs`: AEAD, RSA, X25519, HKDF, fingerprints
  - `protocol.rs`: protocol message encoding/decoding
  - `framing.rs`: length-prefixed packet framing
- `src/network/`
  - `session.rs`: handshake, encrypted transport, replay protection, rekeying
  - `discovery.rs`: optional mDNS registration and discovery
- `src/identity/`
  - `mod.rs`: identity creation, password-based encryption, invite generation
- `src/gui/`
  - egui application, dialogs, help, styling
- `src/tui/`
  - ratatui application and command-mode workflow
- `src/transfer/`
  - file receive path and transfer file abstractions

### Important runtime invariants

- `ChatManager` is the application source of truth.
- Identity keys must remain encrypted on disk.
- Transport sequence numbers are monotonic per active session/chat mapping.
- Signed invite generation uses RSA-PSS over the application’s serialized payload bytes.
- The runtime currently supports RSA-PSS identity proofs only, even though the wire format keeps a `SignatureScheme` field.

## Security-Sensitive Areas

Review carefully before changing:

- `src/network/session.rs`
- `src/core/crypto.rs`
- `src/core/protocol.rs`
- `src/identity/mod.rs`
- `src/app/persistence.rs`

When touching these:

- keep protocol encode/decode symmetric
- preserve transcript-bound AAD expectations
- preserve replay-protection semantics
- never reintroduce plaintext identity persistence
- update protocol/security docs in the same change

## Documentation Maintenance Rules

When behavior changes:

- update `README.md` if user expectations change
- update `docs/USER_GUIDE.md` if usage, commands, or troubleshooting changes
- update `docs/03_architecture.md` if responsibilities or major flows change
- update `docs/04_protocol.md` if wire/runtime behavior changes
- update `SECURITY.md` and `THREAT_MODEL.md` if security claims or assumptions change
- update `CHANGELOG.md` for any user-visible, protocol, or security-relevant change

Avoid adding new top-level docs when an existing canonical document already owns the subject.

## Release Checklist

1. Run format, clippy, and tests.
2. Update docs affected by the change.
3. Update [CHANGELOG.md](CHANGELOG.md) with the release notes.
4. Bump version in `Cargo.toml` when cutting a release.
5. Update packaging artifacts if installer behavior or branding changed.
6. Push `main`, then push the release tag so `.github/workflows/release.yml` publishes the assets.

## Notes for Security and Protocol Work

- Identity proofs and transport packets are authenticated with transcript-bound AAD.
- Rekeying is implemented at the transport layer.
- Legacy invite format is still accepted for compatibility; the UI emits signed v2 invites.
- LAN discovery is optional and disabled by default because of privacy tradeoffs.
