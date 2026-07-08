# Architecture

This document describes the current codebase structure and the major runtime responsibilities.

## Workspace Layout

The project is a Cargo **workspace** of four crates so the client app, the Party
server, and the desktop shell can share code (see `docs/05_platform_spec.md`):

```text
core/             messenger-core   — crypto, wire protocol, identity, transport, shared types,
                                     and the Party application protocol (core::party)
client/           encodeur_rsa_rust — the unified app (egui GUI + ratatui TUI) and its binary
server/           messenger-server  — the Party server (TCP listener, state, dispatcher, hub)
desktop/src-tauri p2pem-desktop     — the Tauri 2 desktop shell that bridges the same
                                     ChatManager/PartyManager to a React/Vite web UI (desktop/src/)
```

`client` depends on `core` and re-exports it (`pub use messenger_core::*;`), so the
client modules and integration tests reach core types via the usual paths. The
`p2pem-desktop` crate depends on `client` (for the managers) and wraps them in
`#[tauri::command]`s. The client binary keeps the name `encodeur_rsa_rust`
(packaging is unchanged; the release pipeline still ships that egui binary — the
Tauri app is run from source during development). Bare `cargo build`/`test`/`run`
target the client via `default-members`; CI builds the whole workspace with
`--workspace` (installing the WebKitGTK system libraries the desktop crate needs).

## High-Level Shape

```text
GUI (egui)   TUI (ratatui)   Tauri desktop (React webview)
     \            |            /
      \           |           / #[tauri::command] + events (desktop/src-tauri)
       v          v          v
              ChatManager
                  |
  -------------------------------------
  |        |         |        |        |
network   core    identity  transfer  persistence
```

The project is centered around `ChatManager`, which coordinates chats, contacts, sessions, file-transfer state, toasts, and persistence. All three front-ends drive the same `ChatManager` (and `PartyManager`) instance — the egui GUI and TUI hold `Arc<Mutex<ChatManager>>` directly; the Tauri desktop shell wraps the same `Arc<Mutex<…>>` behind commands and events.

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
  party/
    mod.rs          Party application protocol (shared by client + server)
  transfer/
    receiver.rs

client/src/
  main.rs         entry point for GUI/TUI launch
  lib.rs          client exports; re-exports messenger-core
  support.rs      diagnostics export and panic/crash support
  colorgrid.rs    fingerprint color-grid rendering (egui Color32)
  app/
    chat_manager/   ChatManager split by concern:
      mod.rs          struct, constructor, accessors, toasts, data deletion
      connect.rs      hosting, connecting (direct/relay/contact), TOFU confirm
      contacts.rs     contact CRUD, auto-reconnect, group-chat creation
      events.rs       session-event pump (poll + handle)
      files.rs        file transfers (validate, send, receive, wire confirm)
      invites.rs      invite links (v1/v2/v3) + QR codes
      text.rs         text send/chunk/reassembly, typing indicators
      tests.rs        ChatManager unit tests
    party_manager.rs  Party server client-side state and operations
    persistence.rs
  gui/
    app_ui.rs
    chat_view.rs
    dialogs.rs
    help_view.rs
    party_view.rs   Party server window (join, channels, members, DMs)
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
  main.rs         TCP accept loop + server bootstrap
  state.rs        PartyState: members, channels, DM threads, history, persistence
  dispatch.rs     request → response/broadcast routing
  hub.rs          cross-connection broadcast fan-out
  connection.rs   per-connection handshake + serve loop
  identity.rs     persistent owner-only server identity

desktop/
  src-tauri/
    src/lib.rs    the Tauri bridge core: Bridge state, run()/init, the background
                  poll loop that forwards toasts / fingerprint requests / party
                  events and persists history, and its own data dir
                  (ProjectDirs "P2PEM", P2PEM_DATA_DIR override)
    src/commands/ the #[tauri::command] handlers, grouped by concern:
                  auth.rs (identity + settings), chats.rs, connect.rs,
                  contacts.rs, party.rs (Communities + parties.json pinning)
    tauri.conf.json  window, bundle identifier, and CSP
  src/            React/Vite web UI (JSX, plain CSS, lucide-react icons)
    App.jsx       shell: onboarding/unlock gate + tab rail + list/content panes
    lib/bridge.js the JS side of the bridge (invoke wrappers + onBridge events;
                  falls back to an in-memory mock in a plain browser)
    components/   Messages, Parties, Relays, Contacts, Settings, Verify,
                  Onboarding, Creator, ChatDialogs, SafetyGrid, Toasts, ui
```

## Module Responsibilities

### `client/src/main.rs`

- parses CLI mode/launch flags
- configures tracing
- starts GUI or TUI

### `client/src/app/chat_manager/`

- central application state (`mod.rs`)
- contact/chat/session mapping (`contacts.rs`, `connect.rs`)
- message routing and session-event handling (`events.rs`)
- send flows for text, typing, files (`text.rs`, `files.rs`)
- fingerprint-verification workflow (`connect.rs`, `events.rs`)
- invite links and QR codes (`invites.rs`)
- toast notifications (`mod.rs`)

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

### `core/src/party/mod.rs`

- the Party application protocol (requests/responses, envelopes, member/channel
  types) shared by the client and server, framed over the v3 tunnel

### `server/src/`

- the Party server: TCP accept loop, `PartyState` (members/channels/DM
  threads/history + embedded-SQLite persistence), request dispatcher, cross-connection
  broadcast hub, per-connection serve loop, and a persistent owner-only identity

### `client/src/app/party_manager.rs`

- client-side Party state: connect/join, post, DMs, channel creation, history
  fetch, and event polling for the GUI/TUI Party surfaces

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

### `desktop/src-tauri/src/lib.rs` (the Tauri bridge)

- wraps the same `Arc<Mutex<ChatManager>>` + `Arc<Mutex<PartyManager>>` and exposes
  every UI action as a `#[tauri::command]` (auth/unlock, conversations, connect,
  TOFU, contacts, and the Party/Communities surface); non-auth commands are gated
  by `ensure_ready()`
- runs a background poll loop that drains toasts → `toast` events, fingerprint
  requests → `fingerprint-request`, and party events → `party-updated`, **and
  persists encrypted history** (on state change, after mutations, and on window
  close) — the egui GUI's per-frame polling equivalent
- resolves its **own** data dir from `ProjectDirs("com","chat-p2p","P2PEM")` (not
  egui's `"EncryptedMessenger"`), with a `P2PEM_DATA_DIR` override, so the desktop
  app is a distinct identity/peer rather than a self-connection to a running egui

### `desktop/src/` (the React web UI)

- a React 19 + Vite app (JSX, plain CSS, `lucide-react` icons — no TypeScript or
  Tailwind) implementing the tab-rail / list / content shell from
  `docs/05_platform_spec.md` §10; `lib/bridge.js` calls the Tauri commands and
  subscribes to the events (falling back to an in-memory mock in a plain browser)

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
- three front-ends (egui, ratatui, Tauri/React desktop) share backend state but not identical UX depth; the Tauri desktop app is meant to replace egui but the release pipeline still ships the egui binary, so both coexist during the migration
- the desktop crate has no automated tests and cannot be driven headlessly here — it is verified via `cargo check -p p2pem-desktop` and `npm run build`, not the workspace test suite
- persistence and migration are practical, but still lightweight rather than enterprise-grade
