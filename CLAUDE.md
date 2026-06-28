# CLAUDE.md

This file provides repository-specific guidance for coding agents working in this project.

## Workspace Layout

Cargo **workspace** with four members (root `src/` and `tests/` are vestigial — ignore them):

| Crate | Path | What it is |
|-------|------|------------|
| `messenger-core` | `core/` | Crypto, protocol, network/session, transfer, identity, shared `types.rs` (UI-agnostic) |
| `encodeur_rsa_rust` | `client/` | The app: ChatManager + egui GUI + ratatui TUI (lib + binary) |
| `messenger-server` | `server/` | Party/Community server (hub, dispatch, connection, state) |
| `p2pem-desktop` | `desktop/src-tauri/` | **New** Tauri 2 shell wrapping the React/Vite web UI in `desktop/src/` |

`default-members = ["client"]`, so bare `cargo` commands act on the client. Add `--workspace` (CI) or `-p <crate>` to target others.

## Development Commands

```bash
# Build / test (Rust)
cargo build --release          # client (LTO, opt-level=3) — egui GUI binary
cargo build --workspace        # all crates
cargo test --workspace         # all Rust tests (core + client)
cargo nextest run --workspace  # faster runner (preferred per global tooling)
cargo test <name> -- --exact   # one test
cargo fmt --all
cargo clippy --workspace --all-targets

# Run the legacy egui/TUI binary
cargo run --release                              # egui GUI (default)
cargo run --release -- --tui                     # ratatui TUI
cargo run --release -- --host --port 9000        # host
cargo run --release -- --connect 127.0.0.1:12345 # connect
cargo run -p messenger-server                    # Party server

# Run the new Tauri desktop app (the UI direction going forward)
cd desktop && npx tauri dev      # runs `npm run dev` (Vite :5173) + the Tauri shell
cd desktop && npx tauri build    # packaged desktop build
cd desktop && npm run dev        # frontend only, in a plain browser (uses the dev mock — see bridge.js)

# Logging
RUST_LOG="info,encodeur_rsa_rust=debug" cargo run

# Windows packaging (egui binary)
./build-and-package.ps1
```

> ⚠️ Building under OneDrive can fail; this machine sets `CARGO_TARGET_DIR` outside the synced tree.

## Architecture Overview

`ChatManager` (`client/src/app/chat_manager.rs`) is the single source of truth for all app state — chats, contacts, sessions, transfers, toasts. It has **zero UI dependencies**, so three front-ends drive the same core:

```
┌── egui GUI ──┐  ┌── ratatui TUI ──┐  ┌── Tauri webview (React) ──┐
│ client/gui/  │  │ client/tui/     │  │ desktop/src/ (frontend)   │
└──────┬───────┘  └────────┬────────┘  └─────────────┬─────────────┘
       │  Arc<Mutex<ChatManager>>                    │ Tauri commands + events
       └────────────────────┬───────────────────────-┘   (desktop/src-tauri/src/lib.rs = bridge)
                ┌────────────▼────────────┐
                │  ChatManager (app/)     │  poll_session_events(), persistence (encrypted)
                └────────────┬────────────┘
                             │ tokio::sync::mpsc
        ┌──────────┬─────────┼──────────┬──────────┐
     Network     Crypto   Transfer   Identity    Party
   (core/network)(core/core)(core/transfer)(core/identity)(core/party + server/)
```

### Key Patterns

- **ChatManager**: all state changes (persisting, session mapping, chat ops) go through its methods. egui/TUI hold `Arc<Mutex<ChatManager>>` directly; the Tauri bridge wraps the same `Arc<Mutex<ChatManager>>` and exposes it via `#[tauri::command]`s.
- **Async**: `tokio` throughout; `tokio::spawn` background tasks; `tokio::sync::mpsc` (often `unbounded_channel()`) for low-latency messaging. Bridge and egui share one runtime.
- **Session Events**: the network layer emits `SessionEvent` (`Listening`, `Connected`, `NewConnection`, `ShowFingerprintVerification`, `MessageReceived`, `Disconnected`, `Error`, `Warning`); UIs poll `ChatManager::poll_session_events()`.
- **Conversation model (Phase 3)**: every `Chat` (`core/src/types.rs`) carries `kind: ChatKind` (`Dm` / `Group` / `Channel`) and `transport: Transport` (`Direct` / `Relay` / `Server`). Both are `#[serde(default)]` for back-compat with old history files. The UI uses these for badges; all 14 `Chat` construction sites set them explicitly.
- **Protocol v3 handshake (ECDH-first)**: (1) version exchange (plaintext u32), (2) X25519 ephemeral exchange (forward secrecy), (3) HKDF-SHA256 session-key derivation, (4) AES-256-GCM tunnel, (5) identity exchange inside the tunnel (`IdentityProof`, RSA-PSS signature binding ephemeral key → identity), (6) TOFU fingerprint verification.

Authoritative docs: `docs/README.md`, `docs/03_architecture.md`, `docs/04_protocol.md`, `SECURITY.md`. UI-redesign roadmap: `docs/superpowers/specs/2026-06-03-ui-redesign-tauri-design.md`.

## Tauri Bridge (`desktop/src-tauri/src/lib.rs`)

- `Bridge` holds `Arc<Mutex<ChatManager>>` + identity + history/identity paths + `pending_fp`. A background poll loop drains ChatManager toasts → emits `toast` events and forwards fingerprint requests → `fingerprint-request` event. The frontend listens via `onBridge(...)` in `desktop/src/lib/bridge.js`.
- Commands: `auth_status`, `unlock`, `set_password`, `my_identity`, `list_conversations`, `get_conversation`, `send_message`, `start_host`, `connect_peer`, `confirm_fingerprint`, `pending_fingerprint`, `rename_chat`, `delete_chat`, `list_contacts`, `my_invite_link`, `import_invite`, `connect_contact`.
- ⚠️ **Arg-naming footgun**: Tauri 2 binds JS invoke keys by exact name. Use **single-word** Rust params (e.g. `id`, not `chat_id`) or camelCase JS keys — a mismatch makes `invoke` *silently no-op* (this was the root cause of the "messages send but don't arrive" / "fingerprint verify does nothing" bugs). When `inTauri` is false, `bridge.js` falls back to an in-memory mock so the UI is navigable in a plain browser.

## Security-Sensitive Areas

⚠️ Extra scrutiny:
- `core/src/network/session.rs` — handshake, message loop, replay protection
- `core/src/core/crypto.rs` — key derivation, AEAD, signing
- `core/src/identity/` — private-key storage (Argon2 + ChaCha20-Poly1305)

**Critical constraints**:
- `to_plain_bytes()`/`from_plain_bytes()` in `core/src/core/protocol.rs` must stay **symmetric** — change both sides together or break wire compat.
- Session keys bind the full handshake transcript via HKDF salt/info — only touch key derivation if you understand the implications.
- Replay protection uses per-session sequence numbers; file-transfer packets share that namespace (`last_recv_seq` in `Session`).
- Plaintext private keys must never be persisted (`zeroize`).
- The app blocks all UI until password unlock/set-password completes.

## File Transfer

Chunked (`FILE_CHUNK_SIZE = 64 KiB`), tracked via `FileTransferState`, sharing the per-chat monotonic sequence namespace (so replay protection covers transfers too).
- `client/src/app/chat_manager.rs` — orchestration + outgoing chunk dispatch
- `core/src/transfer/receiver.rs` — receiving
- `core/src/core/protocol.rs` — `FILE_CHUNK` encode/decode

## TOFU Flow

Handled in `ChatManager::handle_session_event`: unknown fingerprint → `ShowFingerprintVerification` → UI shows the 64-char fingerprint / colored safety grid → user verifies out-of-band → `ChatManager::confirm_fingerprint()` persists trust on accept.

## Common Entry Points

| Task | Start Here |
|------|------------|
| Feature work (state/logic) | `client/src/app/chat_manager.rs` |
| Shared types / conversation model | `core/src/types.rs` |
| Protocol changes | `core/src/network/session.rs`, `core/src/core/crypto.rs`, `core/src/core/protocol.rs` |
| egui / TUI changes | `client/src/gui/` / `client/src/tui/` |
| New web UI | `desktop/src/` (React) + `desktop/src-tauri/src/lib.rs` (bridge) |
| Party/Community server | `server/src/` |
| Identity / persistence | `core/src/identity/`, `client/src/app/persistence.rs` |

## Important Constants (`core/src/lib.rs`)

`PORT_DEFAULT = 12345`, `MAX_PACKET_SIZE = 8 MiB`, `FILE_CHUNK_SIZE = 64 KiB`, `AES_KEY_SIZE = 32`, `AES_NONCE_SIZE = 12`, `HANDSHAKE_TIMEOUT_SECS = 15`, `MAX_FILE_SIZE = 10 GiB`.

## Testing

- Unit tests live in source files (e.g. `core/src/network/session.rs` has handshake tests); integration tests in each crate's `tests/`.
- Async tests use `#[tokio::test]`. Handshake tests must verify derived keys match on both sides. Protocol changes require new (de)serialization tests. Adding `serde(default)` fields requires a back-compat test that loads old JSON.

## Documentation Discipline

- Update the canonical doc instead of adding a parallel explanation elsewhere.
- Prefer deleting superseded docs after merging their useful content.
- Keep this file short; it is an agent hint, not the main project manual.
