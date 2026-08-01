# Developer Guide

This document is the technical guide for building, testing, changing, and releasing the project. The contribution process (branching, commits, PR checklist) lives in [CONTRIBUTING.md](CONTRIBUTING.md); full architecture and protocol detail live in the dedicated docs indexed at [docs/README.md](docs/README.md).

## Toolchain

- Rust: `1.86+`
- Edition: `2021`
- The repo is a Cargo **workspace** of four crates: `core/` (`messenger-core`),
  `client/` (`p2pem-classic`), `server/` (`messenger-server`), and
  `desktop/src-tauri/` (`p2pem-desktop`). Bare `cargo` commands target the client.
- Main entry points:
  - Terminal client / relay launcher: `client/src/main.rs`
  - Client library root: `client/src/lib.rs` (re-exports `messenger-core`)
  - App coordinator: `client/src/app/chat_manager/` (module split by concern)
  - Core (crypto/protocol/network/identity): `core/src/`
  - Party server: `server/src/main.rs`
  - Tauri desktop bridge: `desktop/src-tauri/src/lib.rs`; React UI: `desktop/src/`

> On crate naming: the client crate is still `p2pem-classic` (renamed from the
> historical `encodeur_rsa_rust`, an RSA-era name; the protocol has long since
> moved to X25519-based session establishment, with RSA kept for identity
> signatures only). The "classic" in that crate name is now vestigial — the egui
> GUI it referred to has been deleted, and the crate is the app core plus the
> terminal UI. The shipped product is **P2PEM Desktop**; the terminal client and
> server ship as **P2PEM Tools**. See [docs/README.md](docs/README.md#naming).

## Build and Run

### Build

```bash
cargo build                    # client (default member)
cargo build --release
cargo build --workspace        # core + client + server + desktop crate
```

### Run the desktop app (the product)

```bash
cd desktop && npx tauri dev    # native window + React UI (Node + Tauri prereqs required)
cd desktop && npm run dev      # frontend only in a plain browser (uses the bridge.js mock)
cd desktop && npx tauri build  # packaged installers
```

### Run the terminal client / server

```bash
cargo run --release                       # ratatui TUI
cargo run -p messenger-server             # Party server
```

### Useful Launch Variants

```bash
cargo run --release -- --host --port 9000
cargo run --release -- --connect 127.0.0.1:12345
cargo run --release -- --relay-server --port 23456
```

To run two peers on one machine, give the extra instance its own data directory:

```bash
P2PEM_DATA_DIR=/tmp/p2pem-test cargo tauri dev   # from desktop/
```

## Quality Gates and CI

The pre-PR check commands and the full PR checklist are in
[CONTRIBUTING.md](CONTRIBUTING.md#local-workflow). Run the whole suite with
`cargo nextest run --workspace` (or `cargo test --workspace`). The
`p2pem-desktop` bridge has its own integration tests
(`desktop/src-tauri/src/tests.rs`) that drive the Tauri command layer over a
mock runtime — no display needed, but compiling the crate requires the
GTK/webkit dev packages CI installs. The webview UI itself cannot be driven
headlessly; it is covered by `npm test` + `npm run build` in `desktop/`.

CI (`.github/workflows/ci.yml`) enforces formatting, clippy with warnings
denied, tests on Ubuntu/Windows/macOS, and a locked Linux build verification.
The tag-based release workflow (`.github/workflows/release.yml`) publishes **one
product**: P2PEM Desktop installers for Windows, macOS (both architectures), and
Linux, built by `tauri-action`. A secondary **P2PEM Tools** archive per OS
carries the terminal client and the community server for self-hosters. The
release body is composed before the artifacts upload, so the page tells a visitor
which file to take rather than making them guess between similar-looking assets —
which is exactly what went wrong while two desktop apps shipped side by side.

## Code Map

The canonical module map and directory tree live in
[docs/architecture.md](docs/architecture.md). The short version:

- `core/` — crypto, protocol, framing, network session, discovery, relay,
  identity, Party protocol, file transfer. UI-agnostic.
- `client/` — `ChatManager` (application state, split by concern under
  `client/src/app/chat_manager/`), persistence, and the ratatui TUI. No GUI
  toolkit: the desktop crate links this as a library.
- `server/` — the Party server (accept loop, `PartyState`, dispatcher, hub).
- `desktop/` — the Tauri command/event bridge (`src-tauri/`) and the React/Vite
  web UI (`src/`).

### Important runtime invariants

- `ChatManager` is the application source of truth.
- Identity keys must remain encrypted on disk.
- Transport sequence numbers are monotonic per active session/chat mapping.
- Signed invite generation uses RSA-PSS over the application's serialized payload bytes.
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

## Release Checklist

1. Run format, clippy, and tests — Rust (`cargo test --workspace`) **and**
   frontend (`cd desktop && npm test`).
2. Update docs affected by the change.
3. Rebuild and commit `desktop/dist/` if the frontend changed (it is tracked).
4. Move relevant `CHANGELOG.md` entries out of `Unreleased` into the new version section.
5. Bump the version in `Cargo.toml` **and sync `Cargo.lock`** (`cargo update --workspace`)
   — the release builds with `--locked`, so a stale lockfile ships zero binaries.
6. Push `main`, then push the release tag so `.github/workflows/release.yml` publishes the assets.
7. Check the published release page as a first-time visitor would: the download
   table should make the choice obvious without reading anything else.

## Notes for Security and Protocol Work

- Identity proofs and transport packets are authenticated with transcript-bound AAD.
- The session key rotates automatically every 100 messages via a `Rekey` message (16-byte HKDF salt); both sides re-derive and the frame is not surfaced to the app. See `docs/protocol.md`.
- Text over 48 KiB is chunked into `TextChunk` frames (hard cap 64 KiB); file chunks are 64 KiB. Both share the per-session replay `seq` namespace.
- Legacy invite format is still accepted for compatibility; the UI emits signed invites (v2 URL format carrying a v3 payload — see `docs/protocol.md`).
- LAN discovery is optional and disabled by default because of privacy tradeoffs.
