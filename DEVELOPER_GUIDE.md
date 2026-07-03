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
- The repo is a Cargo **workspace** of four crates: `core/` (`messenger-core`),
  `client/` (`encodeur_rsa_rust`), `server/` (`messenger-server`), and
  `desktop/src-tauri/` (`p2pem-desktop`). Bare `cargo` commands target the client.
- Main entry points:
  - GUI/TUI launcher: `client/src/main.rs`
  - Client library root: `client/src/lib.rs` (re-exports `messenger-core`)
  - App coordinator: `client/src/app/chat_manager.rs`
  - Core (crypto/protocol/network/identity): `core/src/`
  - Party server: `server/src/main.rs`
  - Tauri desktop bridge: `desktop/src-tauri/src/lib.rs`; React UI: `desktop/src/`

## Build and Run

### Build

```bash
cargo build                    # client (default member)
cargo build --release
cargo build --workspace        # core + client + server + desktop crate
```

### Run GUI / TUI / server

```bash
cargo run --release                       # egui GUI
cargo run --release -- --tui              # ratatui TUI
cargo run -p messenger-server             # Party server
```

### Run the Tauri desktop app

```bash
cd desktop && npx tauri dev    # native window + React UI (Node + Tauri prereqs required)
cd desktop && npm run dev      # frontend only in a plain browser (uses the bridge.js mock)
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
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace              # or: cargo test --workspace
cargo check -p p2pem-desktop               # the desktop crate has no tests
cd desktop && npm run build                # verify the React frontend builds
```

The workspace test suite is currently **290 tests**. The `p2pem-desktop` crate is
not exercised by the test suite and cannot be driven headlessly here, so it is
verified by `cargo check` + `npm run build` only.

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

- `core/src/core/`
  - `crypto.rs`: AEAD, RSA, X25519, HKDF, fingerprints
  - `protocol.rs`: protocol message encoding/decoding (incl. `TextChunk`, `Rekey`)
  - `framing.rs`: length-prefixed packet framing (DoS-hardened `recv_packet`)
- `core/src/network/`
  - `session.rs`: handshake, encrypted transport, replay protection, key rotation
  - `discovery.rs`: optional mDNS registration and discovery
  - `relay.rs`: self-hosted rendezvous / packet relay mode
- `core/src/identity/mod.rs`: identity creation, password-based encryption, invite generation
- `core/src/party/mod.rs`: the Party application protocol shared by client + server
- `core/src/transfer/receiver.rs`: file receive path
- `client/src/app/`
  - `chat_manager.rs`: application state, routing, chat/contact/session operations
  - `party_manager.rs`: client-side Party state and operations
  - `persistence.rs`: encrypted history load/save, migration compatibility
- `client/src/gui/`: egui application, dialogs, help, styling, Party window
- `client/src/tui/`: ratatui application and command-mode workflow
- `server/src/`: the Party server (accept loop, `PartyState`, dispatcher, hub, identity)
- `desktop/src-tauri/src/lib.rs`: the Tauri command/event bridge
- `desktop/src/`: the React/Vite web UI

### Important runtime invariants

- `ChatManager` is the application source of truth.
- Identity keys must remain encrypted on disk.
- Transport sequence numbers are monotonic per active session/chat mapping.
- Signed invite generation uses RSA-PSS over the application’s serialized payload bytes.
- The runtime currently supports RSA-PSS identity proofs only, even though the wire format keeps a `SignatureScheme` field.

## Security-Sensitive Areas

Review carefully before changing:

- `core/src/network/session.rs`
- `core/src/core/crypto.rs`
- `core/src/core/protocol.rs`
- `core/src/identity/mod.rs`
- `client/src/app/persistence.rs`
- `server/src/state.rs` (Party access control, incl. `blob_bytes_for`)
- `desktop/src-tauri/src/lib.rs` (the IPC boundary + CSP)

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
- The session key rotates automatically every 100 messages via a `Rekey` message (16-byte HKDF salt); both sides re-derive and the frame is not surfaced to the app. See `docs/04_protocol.md`.
- Text over 48 KiB is chunked into `TextChunk` frames (hard cap 64 KiB); file chunks are 64 KiB. Both share the per-session replay `seq` namespace.
- Legacy invite format is still accepted for compatibility; the UI emits signed v2 invites.
- LAN discovery is optional and disabled by default because of privacy tradeoffs.
