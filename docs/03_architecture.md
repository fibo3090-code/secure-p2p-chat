# Architecture

This document describes the current codebase structure and the major runtime responsibilities.

## Workspace Layout

The project is a Cargo **workspace** of three crates so the client app and the
(forthcoming) Party server can share code (see `docs/05_platform_spec.md`):

```text
core/    messenger-core  — crypto, wire protocol, identity, transport, shared types
client/  encodeur_rsa_rust — the unified app (egui GUI + ratatui TUI) and its binary
server/  messenger-server  — Party server (Phase 0 placeholder; built in Phase 1)
```

`client` depends on `core` and re-exports it (`pub use messenger_core::*;`), so the
client modules and integration tests reach core types via the usual paths. The
client binary keeps the name `encodeur_rsa_rust` (packaging is unchanged). Bare
`cargo build`/`test`/`run` target the client via `default-members`; CI builds the
whole workspace with `--workspace`.

## High-Level Shape

```text
GUI (egui) or TUI (ratatui)
        |
        v
   ChatManager
        |
  -----------------------------
  |        |         |        |
network   core    identity  persistence/transfer
```

The project is centered around `ChatManager`, which coordinates chats, contacts, sessions, file-transfer state, toasts, and persistence.

## Directory Structure

```text
core/src/
  lib.rs          core exports and shared constants
  types.rs        shared application data structures
  util.rs         helpers and parsing utilities
  core/
    crypto.rs
    framing.rs
    protocol.rs
  network/
    discovery.rs
    relay.rs
    session.rs
  identity/
    mod.rs
  transfer/
    receiver.rs

client/src/
  main.rs         entry point for GUI/TUI launch
  lib.rs          client exports; re-exports messenger-core
  support.rs      diagnostics export and panic/crash support
  colorgrid.rs    fingerprint color-grid rendering (egui Color32)
  app/
    chat_manager.rs
    persistence.rs
  gui/
    app_ui.rs
    chat_view.rs
    dialogs.rs
    help_view.rs
    sidebar.rs
    styling.rs
    widgets.rs
  tui/
    app.rs          state machine, key routing, command execution
    command.rs      command language (TuiCommand) + parser + registry
    input.rs        EditableField (cursor-aware UTF-8 text editing)
    overlays.rs     modal overlays (verify, contacts, settings, etc.)
    ui.rs           frame composition (chat list, messages, toasts)
client/tests/      integration tests (link against the client lib)

server/src/
  main.rs         Party server placeholder (Phase 1 fills this in)
```

## Module Responsibilities

### `client/src/main.rs`

- parses CLI mode/launch flags
- configures tracing
- starts GUI or TUI

### `client/src/app/chat_manager.rs`

- central application state
- contact/chat/session mapping
- message routing
- send flows for text, typing, files
- fingerprint-verification workflow
- toast notifications

### `client/src/app/persistence.rs`

- encrypted history serialization/deserialization
- compatibility with history versions `1.0` and `1.1`
- background-save snapshot support
- loaded-config sanitization

### `core/src/core/crypto.rs`

- RSA helpers
- AES-GCM wrapper
- X25519 and HKDF helpers
- fingerprints
- invite-signature helpers

### `core/src/core/protocol.rs`

- protocol message definitions
- binary/plain encoding and decoding

### `core/src/core/framing.rs`

- packet framing for the TCP transport

### `core/src/network/session.rs`

- secure handshake
- session message loop
- transport replay protection
- rekey handling

### `core/src/network/discovery.rs`

- optional mDNS registration/discovery
- LAN peer advertisement and lookup

### `core/src/network/relay.rs`

- self-hosted rendezvous and packet relay server mode
- relay transport setup for WAN/NAT-constrained peers
- forwards already-encrypted session traffic without terminating chat encryption

### `core/src/identity/mod.rs`

- identity creation and load/save
- password-based encryption
- history-key derivation
- invite generation

### `core/src/transfer/receiver.rs`

- receiving and finalizing inbound file data

### `client/src/gui/`

- egui interface, dialogs, state presentation, and log/help UI

### `client/src/tui/`

- ratatui interface: command mode with live autocomplete, modal overlays
  (fingerprint verification, password unlock, contacts, settings, identity,
  transfers, help), auto-scrolling message view, toast stack, and a typed
  command language that exposes every action (so the app is fully usable
  from the keyboard or driven programmatically)
- shares the same `ChatManager` backend as the GUI, including fingerprint
  confirmation, encrypted-history persistence, and auto-rehost

## Important Runtime Rules

- `ChatManager` is the source of truth for app state.
- Identity files must remain encrypted on disk.
- Signed invite generation and parsing must stay aligned.
- Protocol serialization and deserialization must stay symmetric.
- Sequence validation and transcript-bound AAD are transport invariants.

## Architecture Gaps

These are real limitations, not hidden assumptions:

- relay-assisted WAN transport exists, but GUI configuration and broader operational polish are still limited
- discovery subsystem is optional and not privacy-neutral
- GUI and TUI share backend state but not identical UX depth
- persistence and migration are practical, but still lightweight rather than enterprise-grade
