# Architecture

This document describes the current codebase structure and the major runtime responsibilities.

## Workspace Layout

The project is a Cargo **workspace** of four crates so the client app, the Party
server, and the desktop shell can share code (see `docs/platform_spec.md`):

```text
core/             messenger-core   — crypto, wire protocol, identity, transport, shared types,
                                     and the Party application protocol (core::party)
client/           p2pem-classic — the app core (ChatManager, PartyManager, persistence)
                                     plus the ratatui TUI binary. No GUI toolkit.
server/           messenger-server  — the Party server (TCP listener, state, dispatcher, hub)
desktop/src-tauri p2pem-desktop     — the Tauri 2 desktop shell that bridges the same
                                     ChatManager/PartyManager to a React/Vite web UI (desktop/src/)
```

`client` depends on `core` and re-exports it (`pub use messenger_core::*;`), so the
client modules and integration tests reach core types via the usual paths. The
`p2pem-desktop` crate depends on `client` (for the managers) and wraps them in
`#[tauri::command]`s. The client crate/binary is still named `p2pem-classic` for
continuity, but it is now the *terminal* client; the release pipeline ships the
Tauri app as the primary product and the client binary as a secondary tool
archive. Bare `cargo build`/`test`/`run`
target the client via `default-members`; CI builds the whole workspace with
`--workspace` (installing the WebKitGTK system libraries the desktop crate needs).

## High-Level Shape

```text
TUI (ratatui)      Tauri desktop (React webview)
       \                    /
        \                  / #[tauri::command] + events (desktop/src-tauri)
         v                v
              ChatManager
                  |
  -------------------------------------
  |        |         |        |        |
network   core    identity  transfer  persistence
```

The project is centered around `ChatManager`, which coordinates chats, contacts,
sessions, file-transfer state, toasts, and persistence. Both front-ends drive the
same `ChatManager` (and `PartyManager`) instance — the TUI holds
`Arc<Mutex<ChatManager>>` directly; the Tauri desktop shell wraps the same
`Arc<Mutex<…>>` behind commands and events.

`ChatManager` has **no UI dependencies at all**, which is what made deleting the
egui GUI a packaging change rather than a rewrite. The one thing it cannot know
on its own is what the user can see, so the shell pushes that down via
`set_ui_presence(focused, active_chat)`; without it, "notify when a message
arrives in the background" would fire for the conversation on screen.

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
  main.rs         entry point: terminal UI + relay-server mode
  lib.rs          client exports; re-exports messenger-core
  support.rs      diagnostics export and panic/crash support
  logbuf.rs       bounded in-process tracing buffer (log overlay + diagnostics)
  colorgrid.rs    fingerprint safety-grid colours as plain (r,g,b) — toolkit-agnostic
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
- coordinates TCP hole punching between punch-capable peers (exchanges
  observed public endpoints + LAN candidates, collects outcome reports) and
  only bridges traffic when the punch fails; wire-compatible with pre-punch
  clients and servers
- forwards already-encrypted session traffic without terminating chat encryption

### `core/src/network/punch.rs`

- TCP hole punching engine (simultaneous open): re-binds the relay control
  connection's local port (`SO_REUSEADDR`/`SO_REUSEPORT`), listens and dials
  every peer candidate in parallel
- validates each established socket with a token-derived hello tag, then the
  host/joiner SELECT/ACK exchange deterministically picks exactly one socket
- all phases deadline-bounded; any failure falls back to the bridged relay

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
  fetch, and event polling for the Communities surfaces

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
  close)
- resolves its **own** data dir from `ProjectDirs("com","chat-p2p","P2PEM")` —
  distinct from the terminal client's `"EncryptedMessenger"` — with a
  `P2PEM_DATA_DIR` override, so the two apps are separate peers on one machine
  (which is what lets you connect them to each other for testing) rather than a
  self-connection racing on one `history.json.enc`
- is covered by an IPC-level test suite (`desktop/src-tauri/src/tests.rs`) that
  drives the real handlers through Tauri's mock runtime; CI runs it on Linux and
  macOS (see `SECURITY.md` for why Windows is skipped)

### `desktop/src/` (the React web UI)

- a React 19 + Vite app (JSX, plain CSS, `lucide-react` icons — no TypeScript or
  Tailwind) implementing the tab-rail / list / content shell from
  `docs/platform_spec.md` §10; `lib/bridge.js` calls the Tauri commands and
  subscribes to the events (falling back to an in-memory mock in a plain browser)

## Important Runtime Rules

- `ChatManager` is the source of truth for app state.
- Identity files must remain encrypted on disk.
- Signed invite generation and parsing must stay aligned.
- Protocol serialization and deserialization must stay symmetric.
- Sequence validation and transcript-bound AAD are transport invariants.

## Architecture Gaps

These are real limitations, not hidden assumptions:

- relay-assisted WAN transport exists, but every WAN path still requires someone to self-host (port forward, UPnP, or a relay)
- direct P2P has **no offline delivery**: both peers must be online at once. Only the community server buffers history
- discovery subsystem is optional and not privacy-neutral
- two front-ends (Tauri/React desktop, ratatui TUI) share backend state but not identical UX depth; the desktop app is the product and the TUI is for headless/terminal use
- the desktop crate's *bridge* is tested over mock IPC, but the webview itself cannot be driven headlessly here — rendering is verified by `npm run build` + `npm test` over the frontend's pure logic
- there is no mobile client
- **history writes are O(total history) per change.** The whole `HistoryFile` is
  re-serialized, encrypted, and rewritten whenever the conversation surface
  changes, which for an active chat means once per message. Compact JSON keeps
  the constant factor down, and the write is atomic + `fsync`ed, but the cost
  still grows with history size and there is no retention policy or incremental
  format. A per-conversation or append-only store is the real fix
- persistence and migration are practical, but still lightweight rather than enterprise-grade
