# CLAUDE.md

This file provides repository-specific guidance for coding agents working in this project.

## Workspace Layout

Cargo **workspace** with four members (root `src/` and `tests/` are vestigial — ignore them):

| Crate | Path | What it is |
|-------|------|------------|
| `messenger-core` | `core/` | Crypto, protocol, network/session, transfer, identity, shared `types.rs` (UI-agnostic) |
| `p2pem-classic` | `client/` | The app: ChatManager + egui GUI + ratatui TUI (lib + binary) |
| `messenger-server` | `server/` | Party/Community server (hub, dispatch, connection, state) |
| `p2pem-desktop` | `desktop/src-tauri/` | **New** Tauri 2 shell wrapping the React/Vite web UI in `desktop/src/` |

**Agent CLI** lives in a **sibling repo** (`../secure-p2p-chat-agent`), not in this workspace — see that repo's README.

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
RUST_LOG="info,p2pem_classic=debug" cargo run

# Windows packaging (egui binary)
./build-and-package.ps1
```

> ⚠️ Building under OneDrive can fail; this machine sets `CARGO_TARGET_DIR` outside the synced tree.

## Architecture Overview

`ChatManager` (`client/src/app/chat_manager/` — split by concern into `connect`, `contacts`, `events`, `files`, `invites`, `text`, with the struct + accessors in `mod.rs`) is the single source of truth for all app state — chats, contacts, sessions, transfers, toasts. It has **zero UI dependencies**, so three front-ends drive the same core:

```text
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
- **LAN peer discovery**: `core/src/network/discovery.rs` advertises `_p2p-messenger._tcp.local.` (mDNS/Bonjour) when hosting and browses for peers, yielding `DiscoveredPeer { name, address, port, fingerprint }`. Surfaced by egui (Connect dialog) and the TUI; the **desktop bridge does not expose it yet** (a known parity gap). TOFU still applies — discovery only supplies the address, never trust.
- **Conversation model (Phase 3)**: every `Chat` (`core/src/types.rs`) carries `kind: ChatKind` (`Dm` / `Group` / `Channel`) and `transport: Transport` (`Direct` / `Relay` / `Server`). Both are `#[serde(default)]` for back-compat with old history files. The UI uses these for badges; all 14 `Chat` construction sites set them explicitly.
- **Protocol v3 handshake (ECDH-first)**: (1) version exchange (plaintext u32), (2) X25519 ephemeral exchange (forward secrecy), (3) HKDF-SHA256 session-key derivation, (4) AES-256-GCM tunnel, (5) identity exchange inside the tunnel (`IdentityProof`, RSA-PSS signature binding ephemeral key → identity), (6) TOFU fingerprint verification.
- **Automatic key rotation (rekey)**: post-handshake, the message loop in `session.rs` rotates the session key every `REKEY_MESSAGE_COUNT` (100) messages by sending a `ProtocolMessage::Rekey` carrying a 16-byte `REKEY_NONCE_SIZE` HKDF salt; both sides re-derive and the `Rekey` frame is *not* surfaced to the app. It shares the per-session `seq` namespace (replay protection covers it). Covered by `client/tests/key_rotation_tests.rs`.
- **Text is chunked like files**: messages over `TEXT_CHUNK_BYTES` (48 KiB) are split into `ProtocolMessage::TextChunk` frames and reassembled; a single message is hard-capped at `MAX_TEXT_MESSAGE_BYTES` (64 KiB) and rejected past it (`protocol.rs`).

Authoritative docs: `docs/README.md`, `docs/architecture.md`, `docs/protocol.md`, `SECURITY.md`. Forward-looking plan (incl. the UI redesign): `docs/platform_spec.md` §10.

## Tauri Bridge (`desktop/src-tauri/src/lib.rs`)

- `Bridge` holds `Arc<Mutex<ChatManager>>` + `Arc<Mutex<PartyManager>>` (Communities) + identity + history/identity paths + `pending_fp`. A background poll loop drains ChatManager toasts → `toast` events, forwards fingerprint requests → `fingerprint-request`, drains party events → `party-updated`, **and persists history** (on state change, after mutations, and on window close). The frontend listens via `onBridge(...)` in `desktop/src/lib/bridge.js`.
- Commands: auth (`auth_status`/`unlock`/`set_password`/`my_identity`), chats (`list_conversations`/`get_conversation`/`send_message`/`send_file`/`rename_chat`/`delete_chat`), connect (`start_host`/`connect_peer`/`host_via_relay`/`connect_via_relay`/`connect_contact`), TOFU (`confirm_fingerprint`/`pending_fingerprint`), contacts (`list_contacts`/`my_invite_link`/`import_invite`), Communities (`party_join`/`party_list`/`party_history`/`party_post`/`party_create_channel`/`party_send_dm`/`party_dm_history`/`party_clear_error`). Every non-auth command is gated by `ensure_ready()`.
- ⚠️ **Arg-naming footgun**: Tauri 2 binds JS invoke keys by exact name. Use **single-word** Rust params (e.g. `id`, not `chat_id`) or camelCase JS keys — a mismatch makes `invoke` *silently no-op* (was the root cause of "messages send but don't arrive" / "fingerprint verify does nothing"). When `inTauri` is false, `bridge.js` falls back to an in-memory mock so the UI is navigable in a plain browser.
- **Own data dir**: the desktop app resolves its data dir from *its own* `ProjectDirs("com","chat-p2p","P2PEM")`, **not** egui's `"EncryptedMessenger"` — otherwise both apps load the same `identity.json` and become the same peer (a self-connection). `P2PEM_DATA_DIR` overrides it to run extra test peers on one machine.

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
- **Acceptance gate**: `Config::auto_accept_files` is enforced (default **off**): an incoming `FileMeta` holds the transfer in `TransferStatus::AwaitingAcceptance` — chunks spool to the temp file, but nothing reaches the download dir or chat history until `accept_incoming_file()`; `reject_incoming_file()` deletes the spool and discards the rest. All three UIs expose accept/decline (egui transfer row, TUI `:accept`/`:decline`, desktop transfer bar).
- `client/src/app/chat_manager/files.rs` — orchestration + outgoing chunk dispatch + wire-level send confirmation
- `core/src/transfer/receiver.rs` — receiving
- `core/src/core/protocol.rs` — `FILE_CHUNK` encode/decode

## TOFU Flow

Handled in `ChatManager::handle_tofu_verification` (via `handle_session_event`). **Both roles verify**: the client emits `ShowFingerprintVerification`, the host emits `NewConnection`. An unknown fingerprint → UI shows the 64-char fingerprint / colored safety grid → user verifies out-of-band → `confirm_fingerprint()` persists trust on accept. A fingerprint already verified in *another* chat/contact is treated as a returning peer and auto-accepted (no re-prompt).

⚠️ Do **not** pre-fill an incoming chat's `peer_fingerprint` with the peer's own fingerprint — that makes the TOFU check trivially "match" and silently auto-trust every caller (a bug that existed and was fixed). Incoming chats must start with `peer_fingerprint: None`.

## Common Entry Points

| Task | Start Here |
|------|------------|
| Feature work (state/logic) | `client/src/app/chat_manager/` (state in `mod.rs`, events in `events.rs`) |
| Shared types / conversation model | `core/src/types.rs` |
| Protocol changes | `core/src/network/session.rs`, `core/src/core/crypto.rs`, `core/src/core/protocol.rs` |
| egui / TUI changes | `client/src/gui/` / `client/src/tui/` |
| New web UI | `desktop/src/` (React) + `desktop/src-tauri/src/lib.rs` (bridge) |
| Party/Community server | `server/src/` |
| Identity / persistence | `core/src/identity/`, `client/src/app/persistence.rs` |

## Important Constants (`core/src/lib.rs`)

`PORT_DEFAULT = 12345`, `MAX_PACKET_SIZE = 8 MiB`, `FILE_CHUNK_SIZE = 64 KiB`, `MAX_TEXT_MESSAGE_BYTES = 64 KiB`, `TEXT_CHUNK_BYTES = 48 KiB` (leaves metadata headroom), `AES_KEY_SIZE = 32`, `AES_NONCE_SIZE = 12`, `AES_GCM_TAG_SIZE = 16`, `REKEY_NONCE_SIZE = 16`, `RSA_KEY_BITS = 2048`, `HANDSHAKE_TIMEOUT_SECS = 15`, `MAX_FILE_SIZE = 10 GiB`.

## Testing

- Unit tests live in source files (e.g. `core/src/network/session.rs` has handshake tests); integration tests in each crate's `tests/`.
- Async tests use `#[tokio::test]`. Handshake tests must verify derived keys match on both sides. Protocol changes require new (de)serialization tests. Adding `serde(default)` fields requires a back-compat test that loads old JSON.

## Non-obvious gotchas (learned the hard way)

- **The three UIs are one app sharing one `ChatManager`.** The P2P protocol is UI-independent, so a connection bug is almost never in `session.rs` — it's config/wiring in the front-end. Distinct binaries (egui vs Tauri) **must** use distinct `ProjectDirs`; sharing one makes them the same identity/peer, so connecting them on one machine is a self-connection (the core completes it, but it's semantically broken and both apps race on one `history.json.enc`).
- **`connection_password` is a field on `ChatManager`, not a per-call arg.** It's session-only (not persisted) and read inside `start_host`/`connect_to_host`. The `None` in `connect_to_host(host, port, None, pk)` is `existing_chat_id`, **not** the password — a common misread.
- **The host creates a NEW chat per incoming connection**, keyed by the *client's* random `chat_id` (`chat_id_mapping` maps incoming→session). There is no persistent per-peer host chat, so returning peers are recognised by **fingerprint** across chats/contacts, not by chat id.
- **`p2pem-desktop` has NO automated tests** and is not exercised by `cargo nextest run --workspace` (that's core/client/server only). Verify bridge changes with `cargo check -p p2pem-desktop`; verify the React frontend with `npm run build` in `desktop/`. The Tauri GUI can't be driven headlessly here, so GUI behaviour stays build-verified only.
- **`desktop/dist/` is tracked in git** (committed build artifacts). Rebuild (`npm run build`) and commit `dist/` alongside frontend source changes, or it goes stale.
- **Party file downloads must go through `PartyState::blob_bytes_for(member, hash)`** (access-checked), never the raw `blob_bytes`. Content-addressed blobs are stored globally (dedup), so the *download endpoint* is what enforces who may see a file (public-channel members, or DM participants).
- **`recv_packet` (`core/src/core/framing.rs`) is DoS-hardened**: rejects oversized length prefixes and reads in 64 KiB chunks. `MAX_PACKET_SIZE` (8 MiB) bounds every frame. Finer per-field caps: message **text** is capped at `MAX_TEXT_MESSAGE_BYTES` (64 KiB, chunked at 48 KiB — see protocol notes above); the **Party server** caps usernames (`MAX_USERNAME_CHARS` 32), channel names (`MAX_CHANNEL_NAME_CHARS` 64), and channel/DM text (`MAX_MESSAGE_TEXT_BYTES` = 64 KiB) in `server/src/state.rs`. UI truncation (ellipsis) is display-only, not a wire limit.

## Preferred Tools (opencode)

These tools are installed globally — always use them instead of slower alternatives:

| Tool | Purpose | Instead of |
|------|---------|------------|
| `rg` (ripgrep) | Fast code search | `grep -r`, slow searches |
| `fd` | Fast file finding | `find`, `ls -R` |
| `cargo-nextest` | Test runner (~2x faster) | `cargo test` |
| `cargo-deny` | License/advisory checks | — |
| `cargo-audit` | CVE scanning | — |
| `cargo-watch` | Auto-run on file changes | — |
| `cargo-tauri` | Tauri CLI | manual tauri builds |
| `npm` | Node.js package manager | — |

Rules:
- Use `rg` for all content searching (`rg "pattern" --include "*.rs"`)
- Use `fd` for all file finding (`fd "*.rs"`)
- Use `cargo nextest run` instead of `cargo test` (faster)
- Run `cargo-deny check` before committing new deps

## Documentation Discipline

- Update the canonical doc instead of adding a parallel explanation elsewhere.
- Prefer deleting superseded docs after merging their useful content.
- Keep this file short; it is an agent hint, not the main project manual.
