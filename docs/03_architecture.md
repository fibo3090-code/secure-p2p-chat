# Architecture

This document describes the current codebase structure and the major runtime responsibilities.

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
src/
  main.rs         entry point for GUI/TUI launch
  lib.rs          exports and shared constants
  types.rs        shared application data structures
  util.rs         helpers and parsing utilities
  app/
    chat_manager.rs
    persistence.rs
  core/
    crypto.rs
    framing.rs
    protocol.rs
  network/
    discovery.rs
    session.rs
  identity/
    mod.rs
  transfer/
    receiver.rs
  gui/
    app_ui.rs
    chat_view.rs
    dialogs.rs
    help_view.rs
    sidebar.rs
    styling.rs
    widgets.rs
  tui/
    app.rs
    ui.rs
```

## Module Responsibilities

### `src/main.rs`

- parses CLI mode/launch flags
- configures tracing
- starts GUI or TUI

### `src/app/chat_manager.rs`

- central application state
- contact/chat/session mapping
- message routing
- send flows for text, typing, files
- fingerprint-verification workflow
- toast notifications

### `src/app/persistence.rs`

- encrypted history serialization/deserialization
- compatibility with history versions `1.0` and `1.1`
- background-save snapshot support
- loaded-config sanitization

### `src/core/crypto.rs`

- RSA helpers
- AES-GCM wrapper
- X25519 and HKDF helpers
- fingerprints
- invite-signature helpers

### `src/core/protocol.rs`

- protocol message definitions
- binary/plain encoding and decoding

### `src/core/framing.rs`

- packet framing for the TCP transport

### `src/network/session.rs`

- secure handshake
- session message loop
- transport replay protection
- rekey handling

### `src/network/discovery.rs`

- optional mDNS registration/discovery
- LAN peer advertisement and lookup

### `src/identity/mod.rs`

- identity creation and load/save
- password-based encryption
- history-key derivation
- invite generation

### `src/transfer/receiver.rs`

- receiving and finalizing inbound file data

### `src/gui/`

- egui interface, dialogs, state presentation, and log/help UI

### `src/tui/`

- ratatui interface, command mode, keyboard workflows

## Important Runtime Rules

- `ChatManager` is the source of truth for app state.
- Identity files must remain encrypted on disk.
- Signed invite generation and parsing must stay aligned.
- Protocol serialization and deserialization must stay symmetric.
- Sequence validation and transcript-bound AAD are transport invariants.

## Architecture Gaps

These are real limitations, not hidden assumptions:

- no NAT traversal or relay layer
- discovery subsystem is optional and not privacy-neutral
- GUI and TUI share backend state but not identical UX depth
- persistence and migration are practical, but still lightweight rather than enterprise-grade
