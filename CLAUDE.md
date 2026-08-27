# CLAUDE.md

This file provides repository-specific guidance for coding agents working in this project.

## Workspace Layout

Cargo **workspace** with four members (root `src/` and `tests/` are vestigial — ignore them):

| Crate | Path | What it is |
|-------|------|------------|
| `messenger-core` | `core/` | Crypto, protocol, network/session, transfer, identity, shared `types.rs` (UI-agnostic) |
| `p2pem-classic` | `client/` | App core (ChatManager, persistence, diagnostics) + ratatui TUI. **No GUI toolkit** — the egui GUI was deleted; the crate name is vestigial |
| `messenger-server` | `server/` | Party/Community server (hub, dispatch, connection, state). **lib + bin**: `main.rs` only parses args and binds; `lib.rs` holds `run_accept_loop` and the modules, so a test can stand a real server up in process |
| `p2pem-desktop` | `desktop/src-tauri/` | **The shipped app**: Tauri 2 shell wrapping the React/Vite web UI in `desktop/src/` |

⚠️ **There is one desktop app.** The egui GUI (`client/src/gui/`) and its
`eframe`/`egui`/`egui_commonmark`/`egui_tracing` dependencies are deleted, along
with `setup.iss` and `build-and-package.ps1`. Do not reintroduce a second
front-end: the whole point was that a customer on the releases page should not
have to guess which app to install. `cargo tree` must show no egui anywhere.

**Agent CLI** lives in a **sibling repo** (`../secure-p2p-chat-agent`), not in this workspace — see that repo's README.

`default-members = ["client"]`, so bare `cargo` commands act on the client. Add `--workspace` (CI) or `-p <crate>` to target others.

## Development Commands

```bash
# Build / test (Rust)
cargo build --workspace        # all crates
cargo nextest run --workspace  # all Rust tests (what CI runs)
cargo test --workspace --doc   # doctests, which nextest does not run
cargo test <name> -- --exact   # one test
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings   # CI gate

# The shipped app (Tauri + React)
cd desktop && npx tauri dev      # runs `npm run dev` (Vite :5173) + the Tauri shell
cd desktop && npx tauri build    # packaged installers
cd desktop && npm run dev        # frontend only, in a plain browser (uses the dev mock — see bridge.js)
cd desktop && npm test           # frontend unit tests (CI gate)

# Terminal client (ratatui) + server
cargo run --release                              # TUI
cargo run --release -- --host --port 9000        # host
cargo run --release -- --connect 127.0.0.1:12345 # connect
cargo run --release -- --relay-server --port 23456
cargo run -p messenger-server                    # Party server

# Logging
RUST_LOG="info,p2pem_classic=debug" cargo run
```

> ⚠️ Building under OneDrive can fail; this machine sets `CARGO_TARGET_DIR` outside the synced tree.

## Architecture Overview

`ChatManager` (`client/src/app/chat_manager/` — split by concern into `connect`, `contacts`, `events`, `files`, `invites`, `text`, with the struct + accessors in `mod.rs`) is the single source of truth for all app state — chats, contacts, sessions, transfers, toasts. It has **zero UI dependencies**, so both front-ends drive the same core:

```text
┌── ratatui TUI ──┐  ┌── Tauri webview (React) ──┐
│ client/tui/     │  │ desktop/src/ (frontend)   │
└────────┬────────┘  └─────────────┬─────────────┘
   Arc<Mutex<ChatManager>>         │ Tauri commands + events
         └───────────┬─────────────┘   (desktop/src-tauri/src/lib.rs = bridge)
          ┌──────────▼──────────┐
          │ ChatManager (app/)  │  poll_session_events(), persistence (encrypted)
          └──────────┬──────────┘
                     │ tokio::sync::mpsc
  ┌──────────┬───────┼──────────┬──────────┐
Network    Crypto  Transfer  Identity    Party
(core/network)(core/core)(core/transfer)(core/identity)(core/party + server/)
```

### Key Patterns

- **ChatManager**: all state changes (persisting, session mapping, chat ops) go through its methods. The TUI holds `Arc<Mutex<ChatManager>>` directly; the Tauri bridge wraps the same `Arc<Mutex<ChatManager>>` and exposes it via `#[tauri::command]`s.
- **UI presence is pushed down, not inferred**: `ChatManager` owns no window handle, so the shell reports focus + open conversation via `set_ui_presence(focused, active_chat)` (desktop: the `set_presence` command, driven by `focus`/`blur`/`visibilitychange`). `should_notify_for(chat_id)` consults it so a desktop notification never fires for the thread on screen. A front-end that never reports presence keeps notifying — the default is "unfocused", because a missing notification is worse than an extra one.
- **Unread is persisted, not inferred**: `Chat::read_count` (`core/src/types.rs`, `#[serde(default)]`) is saved inside the encrypted history. `unread_count()` counts trailing peer messages past the mark, so your own sends never badge and anything that arrived while the app was closed is still unread. **Never seed read state from the current message count at startup** — that is the bug this replaced.
- **A send that cannot be delivered must return `Err`, never `Ok` + a toast.** A front-end reads success as "sent": it clears the composer and adds no message row, so the user's text is destroyed and a four-second toast is its only trace. `send_message` errors when there is no session; the desktop composer is additionally disabled offline (`ChatPane`), which is defence in depth, not the fix. Same rule for `Chat::title_is_custom` (`#[serde(default)]`): once the user renames a conversation, `SessionEvent::Connected` must not relabel it with the peer address.
- **Async**: `tokio` throughout; `tokio::spawn` background tasks; `tokio::sync::mpsc` (often `unbounded_channel()`) for low-latency messaging. The bridge and ChatManager share one runtime.
- **Session Events**: the network layer emits `SessionEvent` (`Listening`, `Connected`, `NewConnection`, `ShowFingerprintVerification`, `MessageReceived`, `FileSendComplete`/`TextSendComplete` (wire-seq confirmations used for delivery receipts), `Disconnected`, `Error`, `Warning`); UIs poll `ChatManager::poll_session_events()`.
- **Delivery receipts**: receiving a text (or finalizing a file) queues `ProtocolMessage::Ack { acked_seq }` back to the sender, which marks the matching `Message.delivered` (correlated FIFO via `TextSendComplete`/`FileSendComplete` wire seqs). Old peers drop the unknown frame harmlessly (seq gaps are allowed). See `docs/protocol.md` § Delivery receipts.
- **Relay = rendezvous-first, bridge-second**: `core/src/network/relay.rs` pairs two peers by token, then coordinates a real **TCP hole punch** (`punch.rs`: simultaneous open from the reused control-connection port, token-tag validation, host-led SELECT/ACK socket selection) so sessions go direct whenever NATs allow; it only bridges bytes (`copy_bidirectional`) as a fallback. Control protocol is append-only bincode enums (wire compat with pre-punch peers/servers both ways); peer labels `p2p:<addr>` vs `relay:<server>` tell the UIs which transport won. `P2PEM_NO_HOLEPUNCH=1` forces bridging.
- **LAN peer discovery**: `core/src/network/discovery.rs` advertises `_p2p-messenger._tcp.local.` (mDNS/Bonjour) when hosting and browses for peers, yielding `DiscoveredPeer { name, address, port, fingerprint }`. Surfaced by the TUI and the desktop app (`list_discovered_peers` + "Nearby peers" in the connect pane); gated behind `Config::enable_mdns` (off by default — it reveals presence on the LAN). TOFU still applies — discovery only supplies the address, never trust. A `ServiceResolved` event **replaces** what the list held for that `fullname` (`merge_resolved`) rather than being skipped as a duplicate, and adds an entry per advertised address: skipping on `fullname` meant a peer that changed IP kept its stale, unreachable entry forever, and taking only `addresses[0]` left a dual-homed peer reachable on one interface out of however many it advertised.
- **Conversation model (Phase 3)**: every `Chat` (`core/src/types.rs`) carries `kind: ChatKind` (`Dm` / `Group` / `Channel`), `transport: Transport` (`Direct` / `Relay` / `Server`), and `read_count`. All are `#[serde(default)]` for back-compat with old history files. The UI uses kind/transport for badges; every `Chat` construction site sets all of them explicitly.
- **Password floor**: `MIN_PASSWORD_LEN` (`core/src/lib.rs`, 12) is enforced in `Identity::encrypt`, **not** in the UIs — a front-end that forgets to validate must still be unable to create a weakly-protected keystore. `decrypt` deliberately has no check, so raising the floor never locks anyone out. `auth_status` publishes it as `min_password_len` so the set-password screen validates against the real rule.
- **Protocol v3 handshake (ECDH-first)**: (1) version exchange (plaintext u32), (2) X25519 ephemeral exchange (forward secrecy; all-zero/low-order peer keys rejected at parse + `was_contributory()` at agreement), (3) HKDF-SHA256 session-key derivation, (4) AES-256-GCM tunnel, (5) identity exchange inside the tunnel (`IdentityProof`, RSA-PSS signature binding ephemeral key → identity), (6) TOFU fingerprint verification (with SAS — see below).
- **Short Authentication String (SAS)**: `derive_sas` (`core/src/core/crypto.rs`) turns the transport AAD (transcript hash) into six digits + three emoji, carried on the `sas` field of `NewConnection`/`ShowFingerprintVerification`. Both peers compute it identically; a MITM's two handshakes differ. Both UIs lead the verification prompt with it (fingerprint/grid demoted to "advanced"). Derivation + emoji table are frozen (known-answer test) — never reorder or change them.
- **Automatic key rotation (rekey)**: post-handshake, the message loop in `session.rs` rotates the session key every `REKEY_MESSAGE_COUNT` (100) messages by sending a `ProtocolMessage::Rekey` carrying a 16-byte `REKEY_NONCE_SIZE` HKDF salt; both sides re-derive and the `Rekey` frame is *not* surfaced to the app. It shares the per-session `seq` namespace (replay protection covers it). Covered by `client/tests/key_rotation_tests.rs`.
- **Text is chunked like files**, and the limits are easy to misread. `MAX_TEXT_MESSAGE_BYTES` (64 KiB) is **not** a cap on a message — it is the threshold above which one gets chunked, and the cap on a single `Text` *frame*. A message over it is split into `ProtocolMessage::TextChunk` frames of `TEXT_CHUNK_BYTES` (48 KiB) each, up to `MAX_TEXT_CHUNKS` (512), so the real ceiling on a message is about **24 MiB** — enforced symmetrically, refused on send and on decode. Reassembly is additionally bounded by `MAX_CONCURRENT_PARTIAL_TEXT_PER_CHAT` (16) partial messages per chat and a 120-second timeout, so a peer cannot open buffers and sit on them. (This entry previously claimed 64 KiB was a hard message cap; it is not, and anyone reasoning about memory bounds from that number would be out by a factor of 384.) ⚠️ **Text decodes as strict UTF-8, never lossy.** `from_utf8_lossy` turns each invalid byte into a 3-byte U+FFFD, so the length check bounded the wire and not the result: a `Text` frame whose 65,536-byte payload is all `0xFF` passed the cap (the frame is 65,557 bytes: a 21-byte header plus the payload), decoded to 196,608 bytes of text, and then could not be re-encoded — 196,629 bytes back on the wire, three times over the cap — a decoder accepting a state its own encoder cannot produce. The practical cost was the ~24 MiB reassembly budget being out by 3×. `decode_text` (`core/src/core/protocol.rs`) rejects instead; the encoder writes a Rust `String`, so no peer of any version can legitimately send text this refuses.

Authoritative docs: `docs/README.md`, `docs/architecture.md`, `docs/protocol.md`, `SECURITY.md`. Forward-looking plan (incl. the UI redesign): `docs/platform_spec.md` §10.

## Tauri Bridge (`desktop/src-tauri/src/lib.rs`)

- `Bridge` holds `Arc<Mutex<ChatManager>>` + `Arc<Mutex<PartyManager>>` (Communities) + identity + history/identity paths + `pending_fp`. A background poll loop drains ChatManager toasts → `toast` events, forwards fingerprint requests → `fingerprint-request`, drains party events → `party-updated`, **and persists history** (on state change, after mutations, and on window close). The frontend listens via `onBridge(...)` in `desktop/src/lib/bridge.js`.
- **`state_signature` covers contacts too.** It hashes chat titles/fingerprints *and* every contact's identity, address, relay details and trust state, plus `contact_to_chat`. Ignoring contacts meant the poll loop could not see an imported invite, a block, or a fingerprint promoted to `Verified` — those changes survived only as far as the next clean shutdown. `import_invite` also saves immediately, like the other contact mutations.
- Commands: auth (`auth_status`/`unlock`/`set_password`/`change_password`/`my_identity`), chats (`list_conversations`/`get_conversation`/`send_message`/`send_file`/`open_file`/`file_preview`/`rename_chat`/`delete_chat`), connect (`start_host`/`connect_peer`/`host_via_relay`/`connect_via_relay`/`connect_contact`), TOFU (`confirm_fingerprint`/`pending_fingerprint`), contacts (`list_contacts`/`my_invite_link`/`import_invite`), Communities (`party_join`/`party_list`/`party_history`/`party_post`/`party_create_channel`/`party_send_dm`/`party_dm_history`/`party_clear_error`). Every non-auth command is gated by `ensure_ready()`.
- **History writes are coalesced.** Every save re-encrypts and fsyncs the *whole* history, so saving per message is O(n²) over a conversation's life. `send_message` and `mark_read` deliberately do **not** save; the poll loop notices the changed `state_signature` and writes at most once per `HISTORY_SAVE_MIN_INTERVAL`, and the window-close handler flushes synchronously. Rare structural changes (rename, delete, settings, accepted fingerprint, contacts) still save immediately — add new commands to whichever group they belong in.
- **Native dialogs go through `native_file_dialog(window, …)`**, which parents them to the Tauri window. An unowned rfd dialog can open *behind* the app: clicks stop working with nothing on screen to explain it, which reads as a freeze. Commands that open one take `window: tauri::WebviewWindow<R>` and are generic over `R: tauri::Runtime` (the handler is generic, so a bare `WebviewWindow` will not compile).
- `open_file` returns `OpenOutcome { opened, blocked, filename }` rather than `()`. A file **received from a peer** whose extension (or Unix exec bit) means the OS would *run* it is refused unless `confirm: true` — "open" on a received `.exe`/`.lnk`/`.desktop` executes attacker-chosen code, and a chat file card is where people click without thinking. Revealing in the file manager is never gated; files the user sent are not gated.
- ⚠️ **Arg-naming footgun**: Tauri 2 binds JS invoke keys by exact name. Use **single-word** Rust params (e.g. `id`, not `chat_id`) or camelCase JS keys — a mismatch makes `invoke` *silently no-op* (was the root cause of "messages send but don't arrive" / "fingerprint verify does nothing"). When `inTauri` is false, `bridge.js` falls back to an in-memory mock so the UI is navigable in a plain browser.
- **Own data dir**: the desktop app resolves its data dir from *its own* `ProjectDirs("com","chat-p2p","P2PEM")`, **not** the terminal client's `"EncryptedMessenger"` — otherwise both apps load the same `identity.json` and become the same peer (a self-connection). `P2PEM_DATA_DIR` overrides it to run extra test peers on one machine.

## Security-Sensitive Areas

⚠️ Extra scrutiny:
- `core/src/network/session.rs` — handshake, message loop, replay protection
- `core/src/core/crypto.rs` — key derivation, AEAD, signing
- `core/src/identity/` — private-key storage (Argon2 + ChaCha20-Poly1305)

**Critical constraints**:
- **`identity.json` is the only key to the message history** (`history_key` derives from the private key). Write it only via `messenger_core::util::write_file_atomic` (temp + fsync + rename, 0600 from creation), and **never** replace an identity that merely failed to *load* — `get_or_create` returns an error for a present-but-unreadable file, the desktop bridge surfaces it as `auth_status.state == "error"`, and `unlock`/`set_password`/`ensure_ready` all refuse. Regenerating there silently makes every stored message undecryptable and breaks TOFU with every contact, while looking like a fresh install.
- `apply_history` **replaces** in-memory state (it does not merge), so a deleted chat cannot reappear on a reload. Live host placeholders are preserved because they represent a listener, not history.
- `to_plain_bytes()`/`from_plain_bytes()` in `core/src/core/protocol.rs` must stay **symmetric** — change both sides together or break wire compat.
- Session keys bind the full handshake transcript via HKDF salt/info — only touch key derivation if you understand the implications.
- Replay protection uses per-session sequence numbers; file-transfer packets share that namespace (`last_recv_seq` in `Session`).
- Plaintext private keys must never be persisted (`zeroize`).
- The app blocks all UI until password unlock/set-password completes.

## File Transfer

Chunked (`FILE_CHUNK_SIZE = 64 KiB`), tracked via `FileTransferState` (now carries a `direction: TransferDirection`), sharing the per-chat monotonic sequence namespace (so replay protection covers transfers too).
- **Acceptance gate**: `Config::auto_accept_files` is enforced (default **off**): an incoming `FileMeta` holds the transfer in `TransferStatus::AwaitingAcceptance` — chunks spool to the temp file, but nothing reaches the download dir or chat history until `accept_incoming_file()`. The spool is capped at `MAX_UNACCEPTED_SPOOL_BYTES` (64 MiB) and the offer is declined automatically past it: the sender streams without flow control, so an uncapped hold let a peer write up to `MAX_FILE_SIZE` (10 GiB) to the disk of someone who had not agreed to receive anything. `reject_incoming_file()` deletes the spool, **emits `FileCancel`** (a decline the sender never hears about still pulls the whole file across the wire), and discards the rest. Both UIs expose accept/decline (TUI `:accept`/`:decline`, desktop transfer bar).
- **A dying session must fail its receives, not just its sends**: `SessionEvent::Disconnected` calls `fail_incoming_transfers` (before the `chat_id_mapping` entries are dropped — that mapping is how a host-side session id resolves to the conversation) so a mid-receive disconnect leaves no stuck progress row, no orphaned temp spool, and a free incoming slot for the retry.
- `client/src/app/chat_manager/files.rs` — orchestration + outgoing chunk dispatch + wire-level send confirmation. **Sends stream from a spawned task** (`spawn_file_stream`) so a large send never holds the manager lock; the task checks an `AtomicBool` cancel flag between chunks, sets an `AtomicBool` `failed` flag on a local I/O error (open/read → `sync_outgoing_transfer_progress` marks the transfer `Failed`), and reports bytes via an `AtomicU64` (mirrored into `active_transfers` by `sync_outgoing_transfer_progress` each poll). Outgoing seqs are placeholders — the session loop stamps the real wire sequence.
- **Two outbound lanes** (see `SessionHandle`): bulk file data (`FileMeta`/`FileChunk`/`FileEnd`) rides a **bounded** `file_tx` (`FILE_LANE_CAPACITY` chunks), so `send().await` applies real backpressure and a slow peer paces the disk reader instead of ballooning the outbound queue up to `MAX_FILE_SIZE` in RAM; the unbounded `from_app_tx` control lane carries text/typing and the `FileCancel` abort (never stuck behind queued chunks). `run_message_loop` (`session.rs`) selects both lanes via the shared `send_outbound_frame` helper, stamping one monotonic wire sequence across both.
- **Cancellation**: `ProtocolMessage::FileCancel { seq }` (binary tag 12, symmetric encode/decode, replay-protected). `cancel_transfer(id)` aborts either direction (stops the stream + emits FileCancel for outgoing; sends FileCancel + `abort_cleanup` for incoming); `handle_peer_file_cancel` handles an inbound FileCancel. Incoming vs outgoing are routed separately (`active_incoming_transfer_id_for_chat` / `active_outgoing_transfer_id_for_chat`) so a simultaneous send+receive on one chat never cross-routes. UI: TUI Transfers overlay (↑/↓ + `c`), desktop transfer cards (`cancel_transfer` command).
- `core/src/transfer/receiver.rs` — receiving. The declared-size check runs **before** the write, not after: checking afterwards still put the excess bytes on disk. ⚠️ **The peer's filename is sanitised at the receiver, by contract.** `IncomingFileSync::new(dest_dir, filename, size)` takes the directory and the name as *separate* arguments: the directory is ours and used as given, the name is the peer's and goes through `sanitize_filename` there. It used to take one joined `dest_path` and keep it verbatim, and `finalize` handed `dest_path.file_name()` straight to `reserve_unique_path_sync` — nothing on that path sanitised at all, so `download_dir.join("../../escaped.txt")` wrote two directories up (spool *and* final rename, because by then the traversal was in the directory component). Unreachable in practice only because `from_plain_bytes` sanitises `FileMeta.filename` at decode, which left the whole traversal defence resting on one call in the decoder with nothing pinning the two together. Both now sanitise; `sanitize_filename` is idempotent so neither is load-bearing alone, and `a_traversing_filename_cannot_escape_the_download_directory` pins the receiver boundary. `sanitize_filename` (`core/src/util.rs`) budgets **bytes** not chars (`MAX_FILENAME_BYTES`, leaving room for the receiver's `tmp_<uuid>_` prefix under NAME_MAX), defuses Windows device names (`CON`, `NUL`, `COM1`…), strips control characters and bidi overrides (U+202E renders `photo_gnp.exe` as `photo_exe.png`), and trims the trailing dots/spaces Windows would drop silently.
- `core/src/core/protocol.rs` — `FILE_CHUNK` / `FileCancel` encode/decode

## TOFU Flow

Handled in `ChatManager::handle_tofu_verification` (via `handle_session_event`). **Both roles verify**: the client emits `ShowFingerprintVerification`, the host emits `NewConnection`. An unknown fingerprint → UI shows the session SAS (short code, primary) with the 64-char fingerprint / colored safety grid demoted to an "advanced" section → user compares out-of-band → `confirm_fingerprint()` persists trust on accept. The pending prompt is `PendingFingerprint { fingerprint, peer_name, sas, session_id }`. A fingerprint already verified in *another* chat/contact is treated as a returning peer and auto-accepted (no re-prompt).

⚠️ Do **not** pre-fill an incoming chat's `peer_fingerprint` with the peer's own fingerprint — that makes the TOFU check trivially "match" and silently auto-trust every caller (a bug that existed and was fixed). Incoming chats must start with `peer_fingerprint: None`.

⚠️ **An invite link is not verification.** A contact imported from a link starts `Unverified`, and `known_trusted` (`events.rs`) only auto-accepts a fingerprint that a *chat* already stores or that a contact holds with `trust_state` `Verified`/`Trusted`. Matching any contact meant pasting a link pre-trusted whatever fingerprint it named, so that peer connected with no SAS prompt at all. `promote_contact_verified` is what moves a contact to `Verified`, after the user accepts.

⚠️ **An invite's `fingerprint` and `public_key` must agree.** `parse_invite_link` checks `fingerprint_pubkey(public_key) == fingerprint` for **both** v1 and v2. The v2 signature only proves the maker holds the private key for the key *inside* the invite; `fingerprint` is a separate field and it is the one every trust decision is made against. `ChatManager::invite_link_is_signed` tells the UI to warn about unsigned v1 links.

⚠️ **Deleting a contact revokes its trust.** `remove_contact` clears the peer's `peer_fingerprint` from every chat (unless another contact still vouches for it), so the confirmation dialog's promise — that they will need verifying again — is true. It also lifts any block, because the block lives only on the contact; `deleting_contact_would_unblock` exists so the dialog can say so. `import_contact` deduplicates by fingerprint and refuses the user's own invite (`set_my_fingerprint`, pushed down by the shell).

⚠️ **Pending prompts are a queue, not a slot.** A host can have several peers mid-handshake at once; with one `Option` the second prompt overwrote the first and that session blocked until its 30-minute timeout with nothing on screen. Read the head with `pending_fingerprint()` (peek — **never consume**), enqueue via `queue_fingerprint_request`, and let `confirm_fingerprint(session_id, …)` remove the matching entry. A UI that *takes* the prompt also breaks accept: `confirm_fingerprint` needs the queued entry to persist the fingerprint onto the chat. `Disconnected` drops a dead session's prompt so it can't block the queue behind it.

## Common Entry Points

| Task | Start Here |
|------|------------|
| Feature work (state/logic) | `client/src/app/chat_manager/` (state in `mod.rs`, events in `events.rs`) |
| Shared types / conversation model | `core/src/types.rs` |
| Protocol changes | `core/src/network/session.rs`, `core/src/core/crypto.rs`, `core/src/core/protocol.rs` |
| TUI changes | `client/src/tui/` |
| New web UI | `desktop/src/` (React) + `desktop/src-tauri/src/lib.rs` (bridge) |
| Party/Community server | `server/src/` |
| Identity / persistence | `core/src/identity/`, `client/src/app/persistence.rs` |

## Important Constants (`core/src/lib.rs`)

`PORT_DEFAULT = 12345`, `MAX_PACKET_SIZE = 8 MiB`, `FILE_CHUNK_SIZE = 64 KiB`, `MAX_TEXT_MESSAGE_BYTES = 64 KiB`, `TEXT_CHUNK_BYTES = 48 KiB` (leaves metadata headroom), `AES_KEY_SIZE = 32`, `AES_NONCE_SIZE = 12`, `AES_GCM_TAG_SIZE = 16`, `REKEY_NONCE_SIZE = 16`, `RSA_KEY_BITS = 2048`, `MIN_PASSWORD_LEN = 12`, `HANDSHAKE_TIMEOUT_SECS = 15`, `MAX_FILE_SIZE = 10 GiB`.

## Testing

- Unit tests live in source files (e.g. `core/src/network/session.rs` has handshake tests); integration tests in each crate's `tests/`.
- Async tests use `#[tokio::test]`. Handshake tests must verify derived keys match on both sides. Protocol changes require new (de)serialization tests. Adding `serde(default)` fields requires a back-compat test that loads old JSON.
- **Fuzzing comes in two halves, and they are not interchangeable.**
  `core/tests/fuzz_parsers.rs` is property-based (proptest), runs on stable in the
  ordinary suite, and gates every PR — that is what makes it useful, and it is
  what found the filename bypass in GHSA-6q3g-734c-22jm by asserting
  `sanitize_filename` is idempotent. `core/fuzz/` is coverage-guided
  (cargo-fuzz + libFuzzer), needs nightly, and is run deliberately via
  `./scripts/fuzz.sh [target] [seconds]`. Two environment traps that script
  absorbs: `cargo fuzz` shells out to `cargo`, so a distro `/usr/bin/cargo`
  shadowing the rustup proxy makes the nested build silently use stable and fail
  on `-Z` (prefix `~/.cargo/bin`; `+nightly` alone does not help), and
  `core/fuzz` is its own workspace so a sanitizer build never lands in the path
  of an ordinary `cargo build`. Corpora and crashes are gitignored; the targets
  are tracked, because a fuzz target nobody can find is one nobody runs.
  Three things that made the coverage-guided half weaker than it looked, now fixed: `scripts/fuzz.sh` never set `-max_len`, so libFuzzer ran at its 4096-byte default while every interesting cap in these decoders sits at 48–64 KiB — the targets could not reach the branches they assert about (which is why `protocol_frame` could not find the lossy-UTF-8 round-trip violation it was asserting against). `core/fuzz` declares its own workspace, so CI's workspace-scoped `fmt`/`clippy` never touched it and the targets were not even compile-checked — `cargo check --manifest-path core/fuzz/Cargo.toml` now runs in Fast Checks. And the corpus is gitignored, so every run started from nothing; `core/fuzz/seeds/<target>/` is tracked starting material, which matters most for the bincode targets where four random bytes are essentially never a valid enum variant index. The script also no longer aborts the whole loop on the first crash.
- **The frontend is linted** (`cd desktop && npm run lint`, a CI gate). The rule
  set is deliberately narrow — bugs, not style — because a linter that reports
  400 opinions on install gets switched off. `exhaustive-deps` is a warning, not
  an error, so existing deliberate dependency arrays do not block a merge.
- **CI runs `cargo nextest run --workspace`** (plus `cargo test --workspace --doc`, which nextest does not cover). Process-per-test is what the suite is written for: the relay tests read a process-global env var and many bind loopback listeners. `.config/nextest.toml` sets the slow-test warning and a terminate-after, so a hung networked test fails instead of blocking the job.
- **End-to-end tests, by subsystem** — each drives the real objects the shipped app drives, not a mock:
  - `core/tests/session_e2e.rs` — the v3 handshake and transport over a duplex stream.
  - `core/tests/relay_e2e.rs` + `relay.rs`'s own tests — punch vs bridge, reconnect, a token reused after its pairing, concurrent pairings, unknown/duplicate/expired tokens. `run_relay_server_with_wait_timeout` exists only so expiry is testable without a five-minute wait.
  - `client/tests/file_transfer_e2e.rs` — two real `ChatManager`s over loopback: `send_file` → wire → spool → the acceptance gate → disk, plus decline, cancel, and the one-send-per-conversation guard. Note the spool lives *in* the download dir (`tmp_<uuid>_`), because finalizing is a rename and a rename is only atomic within one filesystem.
  - `client/tests/party_e2e.rs` — a real `messenger-server` on a loopback port driven by the real `PartyManager`: two-step trust, a changed pin refused before any credential is sent, durable history, private-channel invisibility, file upload/download/permission, a refused post taken back off screen, and role escalation refused.
- The frontend has its own suite: `cd desktop && npm test` (vitest). Two environments in one run — pure modules (password policy, safety grid, unread accounting, theme registry, CSP drift, design-token drift against `design/tokens.json`) in node, and **React component tests** in jsdom, opted into per file with `// @vitest-environment jsdom`. It is a CI gate; run it alongside `cargo nextest run --workspace`.
- **Component tests stub the bridge, never hand-write it.** `src/test/render.jsx`'s `stubApi` builds the stub *from the real `api` object* and throws on a command that does not exist, so a renamed command fails a test instead of becoming a silent `invoke` no-op in production. jsdom here exposes no `localStorage`; `src/test/setup.js` supplies one, or the storage-backed modules would test nothing.
- **Smoke tests are the only gate that starts the app** (`cd desktop && npm run smoke`, Playwright + headless Chromium, a CI gate). Everything else tests pieces: a `script-src 'self'` meta tag once blocked the inline React Refresh preamble Vite injects in dev, so every screen came up **blank**, and all 844 other tests plus 15 CI checks stayed green — nothing anywhere had loaded the page. jsdom cannot catch it (it does not enforce CSP at all), which is why this half needs a real browser. Two projects, because they are different code paths and only one broke: **dev** (Vite dev server, inline preamble) and **built** (`vite preview` over `dist/`, the production policy). Each test asserts a ladder — document + CSP, React mounted, shell landmark visible, nothing logged — and the `problems` fixture attaches every `securitypolicyviolation`, console error and page error to *any* failure, because the rung that actually trips is "React did not mount", which on its own says nothing about why. Vitest owns `src/**/*.test.{js,jsx}`; Playwright owns `desktop/smoke/`. Keep them disjoint or vitest's default glob sweeps up the `.spec.js` files and fails on Playwright's API.
- **Changing `MIN_PASSWORD_LEN` breaks the bridge tests** that call `set_password` — they use passwords that must clear the floor. That is the check working, not a nuisance.

## Non-obvious gotchas (learned the hard way)

- **Both UIs are one app sharing one `ChatManager`.** The P2P protocol is UI-independent, so a connection bug is almost never in `session.rs` — it's config/wiring in the front-end. The two binaries (terminal vs desktop) **must** keep distinct `ProjectDirs`; sharing one makes them the same identity/peer, so connecting them on one machine is a self-connection (the core completes it, but it's semantically broken and both apps race on one `history.json.enc`).
- **`connection_password` is a field on `ChatManager`, not a per-call arg.** It's session-only (not persisted) and read inside `start_host`/`connect_to_host`. The `None` in `connect_to_host(host, port, None, pk)` is `existing_chat_id`, **not** the password — a common misread.
- **The host creates a NEW chat per incoming connection**, keyed by the *client's* random `chat_id` (`chat_id_mapping` maps incoming→session). There is no persistent per-peer host chat, so returning peers are recognised by **fingerprint** across chats/contacts, not by chat id. Corollary: **anything that tears down a conversation must resolve its session id through `chat_id_mapping` first** — `delete_chat(incoming_id)` removing only `chats[id]` left the socket open and the peer's later messages were dropped with a log line while their client showed them as sent.
- **A host session owns exactly one peer, and the listener is released the moment it accepts.** `start_host` binds via `bind_host_listener` **before** creating any chat/session state (a bind failing inside the spawned task was only logged, leaving a phantom "Host on :port" chat that reported itself Connected), and `run_host_session` drops the listener right after `accept()` so auto-rehost can rebind. Session tasks are tracked in `session_tasks` and **aborted** on teardown — dropping the `SessionHandle` alone never wakes a task parked in `accept()`, so it would hold the port for the life of the process. Auto-rehost is *not* gated on `auto_host_on_startup` (that setting is about launch); it is gated on `hosting_port.is_some()`, which is what distinguishes a direct listener from a relay rendezvous that must not be silently replaced by a TCP one.
- **A new session restarts the wire sequence at 1, but `recv_seq` lives on the `Chat`.** Call `reset_chat_sequences` whenever a session attaches to an existing conversation (`Connected`, and the `NewConnection` reconnect path) or the peer's first N messages are rejected as replays. Per-session replay protection still holds — it is enforced in `run_message_loop` on a per-session key.
- **`ProtocolMessage::FileChunk` carries no transfer id**, so two concurrent sends on one conversation interleave into whichever spool the receiver has open and corrupt both files. `send_file` refuses a second outgoing transfer per chat; that guard is what makes the wire format safe, not a UI nicety.
- **`p2pem-desktop` bridge tests run over mock IPC** (`desktop/src-tauri/src/tests.rs`, via tauri's `test` feature): they drive the real command handlers with the exact payload keys `bridge.js` sends, so an arg-name mismatch (the silent-no-op footgun) fails `cargo nextest run -p p2pem-desktop` (or plain `cargo test -p p2pem-desktop` as fallback) instead of production. The command list is registered through the shared `invoke_handler()` in `lib.rs` — add new commands there, never inline in `run()`. Compiling the crate needs the GTK/webkit dev packages (CI installs them; see `ci.yml`). The suite is gated off Windows (a Rust test-harness exe linking tauri aborts at startup there — see the dev-dependencies note in `desktop/src-tauri/Cargo.toml`); Linux/macOS CI runs it. The webview GUI itself still can't be driven headlessly — visual behaviour is covered by `npm test` + `npm run build` in `desktop/` only.
- **`desktop/dist/` is tracked in git** (committed build artifacts). Rebuild (`npm run build`) and commit `dist/` alongside frontend source changes, or it goes stale.
- **Party file downloads must go through `PartyState::blob_bytes_for(member, hash)`** (access-checked), never the raw `blob_bytes`. Content-addressed blobs are stored globally (dedup), so the *download endpoint* is what enforces who may see a file. Access is decided over the **`file_refs` table**, not by rescanning messages — a deleted reference has to stop granting access immediately. Blob bytes are read from disk on demand — a disk-backed `PartyState` keeps `BlobRecord.data: None`, so the storage ceiling is not also an RSS ceiling. ⚠️ **The bytes move outside the state lock, and that is load-bearing.** `serve_connection` holds `state.lock().await` across the whole of `handle_request`, so a 100 MiB read inside it queued every other member's messages behind one person's download — and the chunk endpoint made it far worse by reading the *whole* blob to slice out each 64 KiB chunk. The request path therefore uses `blob_read_for` / `blob_chunk_read_for`, which authorise under the lock and return a `BlobRead` plan; `dispatch` puts it in `Dispatch::deferred` and `serve_connection` resolves it after dropping the guard. `blob_bytes_for` still reads inline and is for tests and synchronous callers only. Writes are split the same way: `take_upload` (lock) → `stage_upload` (hash + write, no lock) → `commit_upload` (lock), routed from `serve_connection` via `dispatch::handle_finish_upload`, and phase 3 **re-checks permissions** because the lock was released in between. `blocking_io` frees a runtime worker, not the mutex — it never fixed this.
- **The client verifies a downloaded blob against its content hash.** `FileData` is matched by the hash the *server* echoed back, which says nothing about the bytes attached to it; `apply` recomputes `blob_hash(&data)` and fails the download on a mismatch. The hash is the integrity check — using it costs one SHA-256.
- **Party roles are ordered and enforced server-side.** `Role` (`core/src/party/mod.rs`) is `Guest < Member < Admin < Owner`, so every check reads as `role >= Role::Admin`. The **first member to join is the Owner** — the operator starts the server and then joins it, and nothing else can bootstrap an admin. Never let a role be granted at or above the granter's own, and never let the owner be demoted; either would hand the community to whoever got an admin seat. The desktop UI hides controls the caller cannot use, but that is politeness — the server refuses regardless.
- **`ChannelKind` is a real access rule.** `Public` is read/write for everyone; `Locked`/`Announce` are readable by all and writable by admins; `Private` is limited to the channel's own `members` list (plus admins, who must be able to moderate it). See `ChannelKind::may_read`/`may_post`.
- **The Party channel list is per member and must never be broadcast.** `channels_for(member)` filters out private channels the member is not in, while the hub sends one *identical* frame to every connection — broadcasting one member's view would either leak the private channels or hide them from their own members. Push `PartyResponse::DirectoryChanged` and let each client re-fetch its own.
- **Deleting a Party file does not delete its message.** Sequence numbers are what clients merge history on, so removing an envelope renumbers the channel and desynchronises everyone who already fetched it. `DeleteFile` drops the `file_refs` row and releases the blob (reclaiming the bytes when the last reference goes); the message stays put and its download starts failing. Deleting a *channel* must release every file shared in it, or those bytes are stranded with nothing holding a count.
- **Party history is paged.** `history_since`/`dm_history` return at most `MAX_HISTORY_BATCH` (200) envelopes; the client asks again with the last `seq` it saw. A whole channel in one frame stopped fitting past `MAX_PACKET_SIZE`, and `send_framed`'s error propagated out of `serve_connection` — the connection just dropped, with nothing on screen, and the community became unjoinable. Client-side, `History` **merges by seq** (it no longer replaces), because a later page must not discard the earlier one.
- **Channel history is seeded when the *channel list* arrives, not on `Joined`.** The client pipelines `Join` → `ListChannels` → `ListMembers`, so `Joined` is applied while `conn.channels` is still empty; seeding there iterated nothing and durable channel history was never fetched at all. `seed_history` runs after `Joined`, `Channels` **and** `Members` (any of the three can complete the picture) and is idempotent via `history_requested`.
- **A community send that the server refuses is taken back off the screen.** Outgoing posts are appended optimistically, so `dispatch` answers a failed post/DM/upload with `PartyResponse::ActionFailed` (not the generic `Error`), and `pending_sends` — a FIFO of `(thread, index)` — correlates each ordered reply with the message it belongs to: `MessagePosted` stamps the real seq, `ActionFailed` removes the row. Leaving it there told the user a message was delivered that was never stored.
- **Party frames carry a sequence number inside the AEAD.** `send_framed`/`recv_framed` take a `FrameSeq` per direction and reject a frame that does not advance it. Without it an on-path attacker could replay a captured `PostMessage` or `Message` and both ends accepted it — the P2P message loop has enforced this per session since v3. Both codecs also **reject trailing bytes** (`party_codec()` in `core/src/party/mod.rs`): bincode 1.x stops at the end of the value and ignores the rest, so `frame` and `frame || junk` decoded identically — two byte strings with one meaning, which makes a frame paddable in flight. The configuration is byte-identical to `bincode::serialize` (fixint, little-endian) so the wire format is unchanged; there is a test pinning that.
- **The connection loop must never abandon a part-read frame.** `serve_connection` selects over "a broadcast to write" and "a request to read"; `select!` drops the future in the losing branch, and `recv_framed` is **not** cancel-safe (length prefix, then body). A dropped read used to consume bytes and throw them away, so the next read parsed body bytes as a length and the connection died on a length/decrypt error — silently, needing only that a broadcast arrive while a client was mid-frame, which is what a busy community does. The read future is therefore held **across** loop iterations (`read_frame` owns the read half and the `FrameSeq` and hands both back), so a cancelled poll resumes instead of restarting. Covered by `a_broadcast_arriving_mid_frame_does_not_break_the_connection`. Any new branch added to that select must be cancel-safe or be built the same way.
- **The community broadcast lane is bounded** (`hub::BROADCAST_QUEUE_DEPTH`, 256, `try_send`). A joined client that stopped reading its socket used to accumulate every broadcast on the server's heap; now it is dropped from the hub and catches up via `FetchHistory` on reconnect. Channel creation is capped at `MAX_CHANNELS` (128) because any member may create a public one. File storage is capped twice — `MAX_TOTAL_BLOB_BYTES` (1 GiB) server-wide and `MAX_MEMBER_BLOB_BYTES` (128 MiB) per member, admins exempt — because a server-wide ceiling alone lets the first member to reach it deny the feature to everyone else. Both count *distinct* content, so one file shared into three channels costs its size once.
- **Joining a community is a two-step trust decision, and no credential moves before step two.** `connect_and_join` takes `expected_fingerprint` *and* `trust_new_identity`, and returns a `PartyJoinOutcome`. With a pin, it compares before writing the `Join` frame — that frame carries the user's community username and password, so checking afterwards means a server that swapped its key has already been handed them. With **no** pin (a first join) and `trust_new_identity: false`, it hangs up after the handshake and returns `NeedsVerification { fingerprint, sas }`; the shell shows the SAS (desktop: the verify card in `JoinForm`; TUI: `:party-connect` then `:party-trust`) and calls again with `true`. First contact used to send the credentials to whatever key answered and pin it afterwards — trust-on-first-use with the trust step missing.
- **`parties.json` holds every community's pin, so it is written like `identity.json`**: `write_file_atomic` (temp + fsync + rename, 0600), and `load_saved_parties` returns `Err` on a damaged file instead of an empty list. Swallowing a parse failure silently discarded every pin and turned the next join back into an unverified first contact — the exact thing the pin exists to prevent. A missing file is still fine (first run). The saved entry, and with it the pin, is written by the **poll loop once `status == Joined`** — never at connect time, or a typo'd address leaves a permanent pinned entry for a community nobody joined.
- **A returning peer belongs in ONE conversation.** The host keys incoming chats by the client's chat id, so before creating one it looks for an existing, session-free chat with the same verified fingerprint and routes into that instead — otherwise every reconnect opened another "Peer ab12cd34" and history fragmented. `connect_to_contact` does the mirror image on the client, and the desktop's `connect_contact` command must call it (not `connect_to_host` directly, which skipped the blocked check, the extra candidate addresses, the relay fallback, and the contact↔chat association).
- **The relay binds dual-stack, and the limiter canonicalises before it masks.** `bind_dual_stack` (`core/src/network/relay.rs`) creates an IPv6 socket with `IPV6_V6ONLY` explicitly **off** (Windows defaults it on, so a plain `[::]` bind there silently stops accepting IPv4) and falls back to `0.0.0.0` where IPv6 is unavailable. That makes the per-/64 IPv6 accounting in `ratelimit.rs` live rather than dead code — and it is why `limiter_key` must call `to_ipv4_mapped` **before** masking: an IPv4 peer arrives as `::ffff:a.b.c.d`, whose address bytes *and* `0xffff` marker both sit inside the range the /64 mask zeroes, so masking first mapped every IPv4 client in the world to `::` and one address could lock out all the others.
- **Community-server join is rate-limited in two places**: `dispatch::MAX_JOIN_ATTEMPTS` caps rejected joins per connection (so one handshake does not buy unlimited password guesses), and `messenger_core::network::ratelimit::RateLimiter` in the accept loop caps connections per IP (the **relay** server uses the same limiter, plus `RELAY_HELLO_TIMEOUT` and a `MAX_PENDING_RENDEZVOUS` cap — a peer that connects and never speaks used to hold a task and socket forever). Both accept loops log-and-continue on a failed `accept()` rather than `?`, so one transient EMFILE does not take the listener down for the life of the process (so reconnecting is not the guessing loop). The password compare is `subtle`-based and constant-time — `Option<&str>` equality short-circuits on the first differing byte, which leaks the secret a character at a time to anyone who can measure it.
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

