# Code quality, maintainability and best practices audit

Auditor 12 of 13. Read-only review of HEAD `62b952f` (the PR #188 merge). Nothing in the repository was modified and no cargo build or test was run. The working tree already held changes made by other agents: `M Cargo.lock`, `M deny.toml` and an untracked `core/tests/zz_lead_pem_check.rs`. They were left alone and are not part of this audit.

## Scope covered

- **Production Rust:** `core/src`, `client/src`, `server/src`, `desktop/src-tauri/src`, about 25.8k non-test lines. Unit-test modules and `tests.rs`/`*_tests.rs` files were excluded mechanically. Every `#[cfg(test)] mod` was cut at its attribute.
- **Tests,** reviewed for quality: `core/tests`, `client/tests`, `server/src/state/tests.rs`, `desktop/src-tauri/src/tests.rs`, plus the in-file test modules.
- **Manifests:** `Cargo.toml` (workspace) and the four member manifests, plus duplicated versions in `Cargo.lock`.
- **Out of scope, cited only:**
  - Items the TUI auditor already reported from the `cmd_*.rs` split: five copies of the "no community joined" block instead of `require_party()`, two truncate helpers, `let _ = id;` (`client/src/tui/app/mod.rs:1246`), `let _ = settings_keys();` (`client/src/tui/overlays.rs:382`), three settings tables, and `unreachable!()` at `client/src/tui/app/cmd_party.rs:97`.
  - The handshake, invite format and module splits deduplicated in PR #188.
- **Vestigial root `src/` and `tests/`:** they **do not exist** any more (`ls src tests` returns "No such file or directory"). The CLAUDE.md sentence "root `src/` and `tests/` are vestigial — ignore them" is stale and should be deleted (QUAL-29).

## Metrics

### File sizes (lines, including in-file tests)

| File | Lines | Notes |
|---|---:|---|
| server/src/state/tests.rs | 2879 | test |
| client/src/app/chat_manager/tests.rs | 2299 | test |
| client/src/app/party_manager.rs | 2236 | about 1,000 lines are production code: one file holds the connection, protocol state machine, upload/download and history |
| core/src/network/session.rs | 2211 | |
| client/src/tui/app/mod.rs | 1624 | |
| core/src/identity/mod.rs | 1526 | |
| server/src/state/blobs.rs | 1475 | |
| core/src/core/crypto.rs | 1375 | |
| core/src/party/mod.rs | 1290 | |
| server/src/dispatch.rs | 1131 | |
| core/src/network/relay.rs | 1128 | |
| server/src/state/mod.rs | 1123 | |
| server/src/connection.rs | 1116 | |
| desktop/src-tauri/src/commands/party.rs | 1034 | |

Per-crate totals (all `.rs` under `src/`): core 12,704; client 15,533; server 9,094; desktop 4,073. Production only: core 7,631; client 10,444; server 4,513; desktop 3,254.

### Longest production functions (measured by brace matching)

| Lines | Function |
|---:|---|
| 213 | `client/src/app/party_manager.rs:978` `PartyServerConn::apply` |
| 204 | `server/src/dispatch.rs:407` `handle_member_request` |
| 199 | `server/src/state/db.rs:154` `reload_from_db` |
| 192 | `client/src/tui/overlays.rs:543` `render_party` |
| 165 | `server/src/connection.rs:119` `serve_connection` |
| 165 | `client/src/app/chat_manager/events.rs:172` `handle_session_event` |
| 136 | `client/src/tui/command.rs:370` `parse_command` |
| 133 | `core/src/network/session.rs:1309` `run_message_loop` |
| 125 | `client/src/app/chat_manager/files.rs:46` `send_file` |
| 121 | `client/src/app/party_manager.rs:390` `connect_and_join` |
| 111 | `core/src/network/relay.rs:501` `host_flow` |
| 106 | `core/src/core/protocol.rs:361` `to_plain_bytes` |

Ten more functions are between 78 and 102 lines.

### Panics in production code (non-test)

| Crate | `.unwrap()` | `.expect(` | `panic!`/`unreachable!`/`todo!` |
|---|---:|---:|---:|
| core | 0 | 8 | 0 |
| client | 0 | 2 | 1 (`cmd_party.rs:97`, already reported by the TUI auditor) |
| server | 7 (6 of them `std::sync::Mutex::lock().unwrap()` in `hub.rs`) | 14 | 0 |
| desktop | 0 | 5 | 0 |

Almost all of these state invariants and are fine (serializing a derived type, "checked above"). Two need action:

- **The real panic risk is not an unwrap.** It is a **string slice on user-pasted input** (QUAL-01).
- **`hub.rs`:** its `.lock().unwrap()` calls are inconsistent with the desktop's `lock_or_recover` (QUAL-15).

### Swallowed results (non-test)

| Crate | `let _ =` | `.ok();` |
|---|---:|---:|
| core | 15 | 3 |
| client | 37 | 1 |
| server | 1 | 0 |
| desktop | 6 | 3 |

Most are channel sends to a receiver that has gone away, or best-effort cleanup, and are correct. The harmful ones are listed in QUAL-02, QUAL-06 and QUAL-12.

### `#[allow(...)]` attributes (non-test)

There are 9:

- **8 × `#[allow(clippy::too_many_arguments)]`:** `core/src/network/session.rs:103,197,226,786,821,872` and `core/src/network/relay.rs:192,221`.
- **1 × `#![allow(dead_code)]`:** `server/src/state/mod.rs:23`.

There is no `#[expect]`, no `[workspace.lints]`, no `clippy.toml`, and no `#![forbid(unsafe_code)]`.

### Other metrics

- **`unsafe`:** zero blocks in the workspace.
- **TODO/FIXME/XXX/HACK:** 0 in production code.
- **Logging:** 265 `tracing` macros in production code: trace 14, debug 34, info 98, warn 74, error 45. 114 use structured fields and 77 interpolate values into the format string, so the style is mixed.
- **Public API surface (non-test):**

  | Crate | `pub fn` | `pub struct` | `pub enum` | `pub const` | `pub` fields | `pub(crate)`/`pub(super)` |
  |---|---:|---:|---:|---:|---:|---:|
  | core | 156 | 33 | 22 | 33 | 153 | 8 |
  | client | 177 | 15 | 7 | 5 | 122 | 81 |
  | server | 77 | 5 | 3 | 6 | 4 | 83 |
  | desktop | 1 | 0 | 0 | 0 | 0 | 95 |

- **`pub fn` never referenced from production code:** 36. Some are test-only by design. The full list is in QUAL-09.
- **Tests:** 668 test functions (core 233, client 290, server 128, desktop 17); 1 is `#[ignore]` (a measurement probe). There are **0 doctests**, yet CI runs `cargo test --workspace --doc`.
- **Duplicated code blocks found:** 11, listed in QUAL-10, QUAL-11, QUAL-13, QUAL-14, QUAL-22 and QUAL-24:
  - `fingerprint_pubkey` / `Identity::calculate_fingerprint`
  - the async and sync file receivers
  - invite candidate-address building
  - the diagnostics report builder
  - data-dir resolution
  - enum↔string mapping for roles and channel kinds
  - test helpers: `free_port` ×5, `pump_until` ×4, `manager_in` ×3, `start_server` ×2, `wait_until`/`awaiting_confirmation` ×2

## Summary

The codebase is unusually well commented and has clearly been hardened by many review rounds:

- no `unsafe`;
- almost no bare `unwrap()` in production;
- deliberate invariants documented at their point of use;
- one shared address parser and one handshake.

The remaining quality problems fall into five themes.

1. **Indexing and slicing on untrusted input, and failure paths that destroy data.**
   - `fingerprint_pubkey` slices a string between two `find()` results without checking their order. It runs on the public key inside a pasted **v1 invite link before any PEM parsing**, so a crafted link crashes the app (QUAL-01). The release profile uses `panic = "abort"`, so the whole process dies.
   - Both front-ends treat a failed history load as "start empty". The next save, at the latest on window close, **overwrites the encrypted history with nothing** (QUAL-02). This is the exact pattern CLAUDE.md forbids for `identity.json`.
2. **Error handling has no types.**
   - `anyhow` is used everywhere, `thiserror` is declared and never used, and server state, the desktop bridge and the Party client all use `Result<_, String>` (desktop has 59 copies of `map_err(|e| e.to_string())`).
   - About 20 tests assert on error *text*.
   - A schema migration decides what to do by matching a SQLite error message.
   - The server logs and ignores every persistence error, and writes multi-row changes without transactions (QUAL-04, QUAL-05).
3. **Observability gaps.**
   - The TUI's in-app log buffer drops every structured field, so the error behind "failed to persist…" never reaches the overlay or a diagnostics bundle.
   - The shipped desktop app has no log sink and no panic hook in production, yet declares the unused `tauri-plugin-log` and `log` crates (QUAL-06, QUAL-07).
4. **Module boundaries and API surface.**
   - The desktop app links the whole TUI crate (`ratatui`, `crossterm`, `arboard`, `clap`).
   - `ChatManager` exposes 11 `pub` fields, including the history key. UIs mutate `config` and `contacts` directly, bypassing validation that exists only at load time.
   - Test helpers are part of the public API, there are 36 unused `pub fn`s, a complete dead copy of the file receiver, and a stale `#![allow(dead_code)]` (QUAL-08, QUAL-09, QUAL-10, QUAL-16).
5. **Duplication and stringly-typed seams.**
   - Session runners take 8–9 positional channel arguments, while the client already has a `SessionChannels` struct that is unpacked just before each call (QUAL-03).
   - The transport kind is encoded as a `"p2p:"`/`"relay:"` string prefix and parsed back in the client.
   - Roles and channel kinds are mapped to strings twice (QUAL-11, QUAL-12).
   - The test suite repeats fixtures and synchronizes with fixed sleeps on races-prone free-port helpers (QUAL-19, QUAL-20).

## Findings — ordered by impact

### QUAL-01 — Pasting a crafted v1 invite link crashes the app: string slice on unvalidated PEM

- Severity: High
- Category: error-handling (panic on user-controlled input)
- Status: VERIFIED (by code reading and arithmetic; not executed)
- Location:
  - `core/src/core/crypto.rs:75-95` (`fingerprint_pubkey`, slice at :82)
  - `core/src/identity/mod.rs:233-250` (`Identity::calculate_fingerprint`, a verbatim copy, slice at :241)
  - Reached from `core/src/identity/invite.rs:98-99` (`check_fingerprint_binding`), called from `decode_unsigned` at :185-193
- Problem:

  ```rust
  if let (Some(begin), Some(end)) = (s.find("-----BEGIN PUBLIC KEY-----"), s.find("-----END PUBLIC KEY-----")) {
      let body = &s[begin + "-----BEGIN PUBLIC KEY-----".len()..end];
  ```

  - Nothing checks that `end >= begin + 26`. The `public_key` value `"-----END PUBLIC KEY----------BEGIN PUBLIC KEY-----"` gives `begin = 24` and `end = 0`, so the slice is `s[50..0]` and it panics.
  - `decode_signed` calls `pem_decode_public` *first*, so v2 links are safe.
  - `decode_unsigned` (legacy v1, still accepted) passes the attacker's `public_key` string straight to `check_fingerprint_binding`, which calls `fingerprint_pubkey` with no prior parsing.
  - The P2P handshake is safe for the same reason v2 is: `pem_decode_public` runs at `session.rs:328` before `fingerprint_pubkey` at `:553`.
- Impact:
  - Anyone who can get a user to paste or import an invite (TUI `:import`, desktop `import_invite`) crashes the app.
  - `[profile.release] panic = "abort"`, so in a release build that kills the whole process, including the desktop app and its unsaved history window.
  - The same function is the TOFU fingerprint primitive, so a panic there is the worst place for one.
  - The "hash raw bytes" fallback also means a v1 invite whose `public_key` is not PEM at all still passes the binding check if `fingerprint = sha256(raw)`. That is a security question for the crypto auditor, but it comes from the same unvalidated input.
- Recommendation:
  1. Have `fingerprint_pubkey` take a parsed `&RsaPublicKey`, or parse internally with `pem_decode_public`, and hash `to_public_key_der()`. Return `Result<String>`.
  2. Delete `Identity::calculate_fingerprint` and call the one function.
  3. In `decode_unsigned`, run `pem_decode_public(&payload.public_key)?` before the binding check, as `decode_signed` already does.
  4. Add a proptest case to `core/tests/fuzz_parsers.rs` that feeds arbitrary strings containing both markers in random order.
  5. Adopt `clippy::string_slice` and `clippy::indexing_slicing` for `core/src/core` and `core/src/identity` (QUAL-27). Either lint would have flagged this.
- Effort: S

### QUAL-02 — A failed history load leads to the encrypted history being overwritten with an empty one

- Severity: High
- Category: error-handling (data loss)
- Status: VERIFIED
- Location:
  - Desktop: `desktop/src-tauri/src/commands/auth.rs:81-83` (unlock) and `:137-139` (set_password) log a warning and continue. `desktop/src-tauri/src/lib.rs:360-369` persists unconditionally on `CloseRequested`. `lib.rs:128-145` (`persist_history`) saves whenever a history key is set.
  - TUI: `client/src/tui/app/mod.rs:153-155` (startup, warn and continue). `:1159-1162` and `:1202-1205` use `let _ = …load_history_auto(…)`, with no log at all. Later `save_history_now()` calls (for example `:1149` in `resolve_fingerprint`, and `cmd_contacts.rs:58`) write the file.
  - The load fails on a version mismatch (`client/src/app/persistence.rs:15-16`, `:152-153` "Unsupported history version"), on JSON or AEAD errors, or on a truncated file.
- Problem: when `load_history_auto` returns `Err`, the history key is already installed (`set_history_key` runs before the load) and in-memory state is empty. Any later save then replaces `history.json.enc` atomically with the empty state:
  - the desktop close handler always saves;
  - the poll loop saves on the first signature change;
  - the TUI saves after any structural command.

  `write_file_atomic` makes the overwrite durable and keeps no backup.
- Impact:
  - A **downgrade** to an older build that only knows versions `1.0`/`1.1` silently destroys every message and contact the newer build wrote.
  - So does a one-off read error or a bit-flip.
  - This is the same failure CLAUDE.md documents and forbids for `identity.json` ("never replace an identity that merely failed to *load*"). The rule was never applied to the history file.
  - The TUI's `try_unlock` also shows "Identity unlocked" in green while history is empty.
- Recommendation:
  1. On a load error, do **not** install the history key. Alternatively, add `ChatManager::history_load_failed: Option<String>`, have `history_snapshot()` return `Err` while it is set, and surface it like the identity error state (`auth_status.state == "error"`, or a TUI error overlay).
  2. Before any save that would replace an unreadable file, rename that file to `history.json.enc.unreadable-<ts>`.
  3. Add a test: write a v`9.9` history, unlock, close, then assert the original bytes are still on disk.
- Effort: S/M

### QUAL-03 — Session runners take 8–9 positional arguments; `SessionChannels` exists but stays client-private

- Severity: Medium
- Category: api-design / maintainability
- Status: VERIFIED
- Location:
  - `core/src/network/session.rs:103` (`run_host_session`), `:197` (`run_client_session`), `:226` (`run_client_session_multi`), `:786` (`verify_then_run`), `:821` (`run_host_session_over_stream`), `:872` (`run_client_session_over_stream`)
  - `core/src/network/relay.rs:192,221` (`run_*_session_via_relay`)
  - The struct already exists at `client/src/app/chat_manager/connect.rs:9-14` (`pub(super) struct SessionChannels`); it is built at `:98-103` and immediately destructured at `:186-196` and its three siblings.
- Problem:
  - Every runner repeats `to_app_tx, from_app_rx, file_rx, confirm_rx, chat_id, connection_password`, and each needs `#[allow(clippy::too_many_arguments)]`.
  - The relay variants pass `None` for `connection_password` positionally (`relay.rs:216`), which is easy to misread. CLAUDE.md even warns about a similar `None` misread on `connect_to_host`.
  - Three of the parameters have the same type (`UnboundedReceiver<…>`), so a transposition between lanes would still type-check for `from_app_rx` and `confirm_rx` if their message types ever converged.
- Recommendation: move the struct to core and bundle the per-session parameters.

  ```rust
  // core/src/network/session.rs
  pub struct SessionChannels {
      pub to_app: mpsc::UnboundedSender<SessionEvent>,
      pub from_app: mpsc::UnboundedReceiver<ProtocolMessage>,
      pub file_lane: mpsc::Receiver<ProtocolMessage>,
      pub confirm: mpsc::UnboundedReceiver<bool>,
  }
  pub struct SessionParams { pub chat_id: Uuid, pub privkey: RsaPrivateKey, pub connection_password: Option<String> }
  pub async fn run_host_session(listener: TcpListener, p: SessionParams, ch: SessionChannels) -> Result<()>;
  pub async fn run_host_session_over_stream<S>(stream: &mut S, peer_label: PeerRoute, p: SessionParams, ch: SessionChannels) -> Result<()>;
  ```

  - Also add `SessionChannels::new() -> (SessionHandleEnds, SessionChannels)`, so `ChatManager::spawn_session` stops building the channels by hand and tests can reuse it. `for_test_control` and the hand-built pairs in `adversarial_tests.rs:29-30` become one call.
  - Better still, replace `confirm: UnboundedReceiver<bool>` with a `oneshot::Receiver<TofuDecision>`. It is answered exactly once and a `bool` carries no meaning at the call site.
  - All 8 `allow`s disappear.
- Effort: M (mechanical, touching about 8 core signatures and 4 call sites)

### QUAL-04 — Errors are untyped across every module boundary

- Severity: Medium
- Category: error-handling / api-design
- Status: VERIFIED
- Location:
  - `thiserror = "2"` sits in `[workspace.dependencies]` (`Cargo.toml`) but **no crate depends on it**. The only error enums are `server/src/state/mod.rs:101` `JoinError` and the private `core/src/network/relay.rs:271` `AttemptError`.
  - `Result<_, String>`: `server/src/state/blobs.rs` (19), `server/src/state/mod.rs` (11), `server/src/dispatch.rs` (3), `client/src/tui/command.rs` (6), and every desktop command (62 signatures).
  - `.map_err(|e| e.to_string())` appears 59 times in `desktop/src-tauri/src`.
  - Tests match on text: about 20 production-adjacent assertions such as `err.to_string().contains("at least")` (`core/src/identity/mod.rs:940,966,1057`), `"Unknown relay token"` (`relay.rs:902,952`) and `"Session not found"` (`client/tests/bug_diagnosis.rs:23`).
  - Production string matching: `server/src/state/db.rs:430-434` treats `msg.contains("duplicate column name")` as "migration already applied". `client/src/app/chat_manager/files.rs:624-626` classifies OS errors by English substrings (`"cloud"`, `"offline"`), which never match on a localized Windows.
  - `PartyRequest::from_bytes` returns `Option` (`server/src/connection.rs:195`), so the decode reason is thrown away and the client gets "malformed request".
- Impact:
  - The front-end cannot tell "wrong password" from "identity unreadable" from "disk full" except by text.
  - Rewording a message breaks tests, or worse, silently changes behaviour, as with the SQLite message match.
  - String errors also lose `source()` chains in logs.
- Recommendation:
  1. Define per-boundary enums with `thiserror`: `core::identity::IdentityError` (`PasswordTooShort { min }`, `WrongPassword`, `Unreadable(io::Error)`, …), `core::network::SessionError`, `server::state::PartyError` (`NotPermitted`, `UnknownChannel`, `QuotaExceeded { … }`, `Invalid(&'static str)`), and `core::party::DecodeError`.
  2. Keep `anyhow` for the binaries' `main` and glue code.
  3. In the desktop crate, add `#[derive(Serialize)] struct CmdError { code: &'static str, message: String }` with `impl<E: Into<anyhow::Error>> From<E>`, so commands use `?` and the frontend can switch on `code`.
  4. Replace the SQLite string match with `PRAGMA user_version`-based migrations (QUAL-05).
  5. Map OS errors by `ErrorKind` and raw OS code (`ERROR_CLOUD_FILE_*` 362–398 on Windows), not by message text.
- Effort: L (incremental per module)

### QUAL-05 — Server persistence logs and ignores every error and has no transactions or schema versioning

- Severity: Medium
- Category: error-handling / best-practice
- Status: VERIFIED
- Location:
  - `server/src/state/db.rs:36-150`: `persist_member`, `persist_file_ref`, `delete_file_ref_row`, `delete_blob_row`, `delete_channel_rows`, `persist_audit`, `persist_channel`, `persist_message`, `persist_dm` and the blob row helpers all return `()` and `tracing::error!` on failure.
  - Migrations at `db.rs:416-436`.
  - The only transaction in the server is `db.rs:636` (the legacy import).
  - No `PRAGMA journal_mode`/`synchronous` anywhere.
  - The calls run synchronously while `state.lock().await` is held on a tokio worker (`server/src/connection.rs:197-199`).
- Problem:
  - In-memory state is mutated and the client receives `MessagePosted` even if the row was never written. A restart then loses the message, or keeps a `file_refs` row whose blob row is gone.
  - `delete_channel_rows` runs three independent `DELETE`s, so a failure in the middle leaves orphans.
  - Channel reordering after a delete, and posting a file (blob row, `file_refs`, message), are multi-statement mutations without atomicity.
  - Migrations are idempotent only by matching an error string, and nothing records which schema version a database is at.
  - This breaks the project's own rule ("A send that cannot be delivered must return `Err`, never `Ok` + a toast") on the server side.
- Recommendation:
  1. Make `persist_*` return `rusqlite::Result<()>`. Have the mutating `PartyState` methods persist **before** committing to memory, or roll memory back, and return a `PartyError::Storage` that `dispatch` maps to `ActionFailed`.
  2. Wrap each multi-row mutation in `conn.transaction()`. That needs `&mut Connection`, which `&mut self` already has.
  3. Set `PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON` at open.
  4. Add `PRAGMA user_version` migrations as numbered steps.
  5. Longer term, move SQLite writes off the async worker: a dedicated writer thread fed by a channel, or `spawn_blocking`. A slow fsync currently stalls every connection, which is the same reasoning that moved blob I/O off the lock.
- Effort: M

### QUAL-06 — The TUI log buffer drops every structured field, so the overlay and diagnostics lose the actual error

- Severity: Medium
- Category: logging
- Status: VERIFIED
- Location: `client/src/logbuf.rs:95-131` (`MessageVisitor` keeps `message` and falls back to the *first* other field only when there is no message). It is wired at `client/src/main.rs:77-83`, the TUI's only subscriber layer, and its output is exported by `client/src/support.rs:70-83`.
- Problem: 114 of the 265 production log calls use structured fields with a message, for example:
  - `tracing::warn!(error = %e, path = %…, "could not delete legacy plaintext history…")`
  - `tracing::error!(error = %e, "failed to persist party member")`
  - `FINGERPRINT MISMATCH` with `old`/`new` fields

  In the TUI log overlay (Ctrl+L) and in every diagnostics bundle, these appear as the bare message with **no error, path, id or fingerprint**. Meanwhile the 77 format-string-style calls keep their values, so whether context survives depends on which style an author happened to use.
- Recommendation:
  - Have the visitor append every non-`message` field as `key=value`, the way `tracing_subscriber::fmt` does.
  - Pick one style: structured fields with a static message. Mechanically convert the 77 interpolated calls, mainly in `events.rs`, `text.rs`, `session.rs` and `receiver.rs`.
  - At the same time, decide a redaction policy (QUAL-18) so the buffer never captures full fingerprints, IP addresses or file paths at info level.
- Effort: S (visitor) + M (style unification)

### QUAL-07 — The shipped desktop app has no persistent log, no panic hook, and unused logging dependencies

- Severity: Medium
- Category: logging / dependency
- Status: VERIFIED
- Location:
  - `desktop/src-tauri/src/lib.rs:330-336` (an `fmt` subscriber to stdout only)
  - `desktop/src-tauri/src/main.rs:2` (`windows_subsystem = "windows"` in release, so there is no stdout)
  - `desktop/src-tauri/src/commands/auth.rs:280-283` (the diagnostics bundle ships the text "Desktop bridge logs go to stdout… no in-app log buffer is captured")
  - `support::install_panic_hook` is called only by `client/src/main.rs:72`
  - `desktop/src-tauri/Cargo.toml` declares `tauri-plugin-log = "2"`, `log = "0.4"` and `anyhow`; `rg` finds no use of `tauri_plugin_log`, `log::` or `anyhow` in `desktop/src-tauri/src`
- Problem:
  - In a release build on Windows (and on macOS/Linux when launched from the desktop) every `warn!`/`error!` vanishes. That includes "Failed to load history after unlock" (QUAL-02), "desktop history save failed" and "identity load failed".
  - With `panic = "abort"` and no hook, a crash leaves no trace.
  - The diagnostics bundle, the support tool, says outright that it has no logs.
- Recommendation:
  - Reuse `p2pem_classic::logbuf::LogBuffer` as a second layer, as the TUI does, and pass its text to `export_diagnostics_bundle`.
  - Add a `tracing-appender` daily-rotating file under `<data_dir>/logs`, capped at N files.
  - Call a variant of `install_panic_hook(dir)` that takes the desktop's data dir. Today `default_diagnostics_dir()` (`client/src/support.rs:86-91`) hard-codes the TUI's `"EncryptedMessenger"` ProjectDirs.
  - Drop `tauri-plugin-log`, `log` and `anyhow` from the desktop manifest, or actually use `tauri-plugin-log` as the sink.
- Effort: S/M

### QUAL-08 — The desktop app compiles the terminal UI; `ChatManager` is not in a UI-agnostic crate

- Severity: Medium
- Category: dependency / maintainability (module boundaries)
- Status: VERIFIED
- Location:
  - `desktop/src-tauri/Cargo.toml` (`p2pem-classic = { path = "../../client" }`, with a stale comment saying it "pulls in egui/ratatui transitively")
  - `client/Cargo.toml` (unconditional `ratatui`, `ratatui-crossterm`, `arboard`, `clap`, `unicode-width`)
  - `client/src/lib.rs:26-31` (`pub use messenger_core::*;` plus `pub mod tui`)
- Problem:
  - The shipped app links `ratatui`, `crossterm`, `arboard` (clipboard, with X11/Wayland deps on Linux) and `clap` for nothing.
  - Its library is called `p2pem_classic`, and the glob re-export makes `p2pem_classic::core` / `crate::types` aliases for core modules, so every path has two spellings.
  - Core also carries UI-only persisted settings (`core/src/types.rs` `Config::theme`, `font_size`, `show_log_terminal`). No front-end reads them except to print them (QUAL-21).
- Recommendation:
  - Split `client/` into an `app` crate (`p2pem-app`: `ChatManager`, `PartyManager`, persistence, logbuf, support, colorgrid) and a `tui` crate (the binary). The desktop depends only on `p2pem-app`.
  - As a stop-gap, put the TUI behind a default `tui` feature and set `default-features = false` in the desktop.
  - Replace the glob re-export with explicit `use messenger_core::{…}` and delete `crate::core` aliasing.
  - Rename `messenger_core::core` (a module called `core` shadows the `core` crate in paths) to `messenger_core::crypto_proto` or split it into `crypto` and `protocol`.
- Effort: M

### QUAL-09 — 36 public functions have no production caller; test helpers are public API

- Severity: Medium
- Category: dead-code / api-design
- Status: VERIFIED. Grep-based: no reference anywhere in non-test code of the four crates. The frontend talks only through Tauri commands, which are Rust.
- Location (grouped):
  - **Test helpers in the production API:**
    - `ChatManager::create_local_chat_for_test`, `add_session_for_test`, `add_fingerprint_confirm_sender_for_test`, `queue_fingerprint_prompt_for_test` (`client/src/app/chat_manager/mod.rs:501-526`)
    - `SessionHandle::for_test_control` (`mod.rs:59`)
    - `invite::resign_for_test` (`core/src/identity/invite.rs:201`, `#[doc(hidden)]`)
  - **Dead crypto and identity API:**
    - `generate_rsa_keypair_async` (`core/src/core/crypto.rs:35`)
    - `generate_ed25519_keypair` (`:276`): the seed is not zeroized
    - `ed25519_public_to_hex`/`_from_hex` (`:318,323`)
    - `ed25519_private_to_hex`/`_from_hex` (`:338,343`): exports a **private key as an un-zeroized `String`**
    - `Identity::remove_password` (`core/src/identity/mod.rs:444`, always returns `Err`)
    - `Identity::generate_invite_link` (`:562`, documented as "DEPRECATED" but without `#[deprecated]`)
    - `ChatManager::generate_invite_link` (`client/src/app/chat_manager/invites.rs:94`, a second unsigned-invite generator)
    - `ChatManager::generate_invite_qr` (`invites.rs:130`)
  - **Dead session and transfer API:**
    - `run_client_session` (`core/src/network/session.rs:198`)
    - all of async `IncomingFile` (QUAL-10)
    - `progress_percent` (`core/src/transfer/receiver.rs:225`)
    - `RateLimiter::with_max_tracked`, `tracked_addresses` (`core/src/network/ratelimit.rs:154,235`)
  - **Dead ChatManager and PartyManager API:**
    - `deleting_contact_would_unblock` (`contacts.rs:177`). CLAUDE.md says "it exists so the dialog can say so", but neither UI calls it. The desktop re-derives it in JS from `blocked` (`desktop/src/components/Contacts.jsx:149`), and the TUI's `:contact-remove` lifts a block with no warning at all (`client/src/tui/app/cmd_contacts.rs:51-61`).
    - `ui_presence`, `total_unread`, `has_connection_password`, `get_chat_mut`, `delete_all_data` (`mod.rs:295,333,445,607,799`)
    - `PartyManager::set_last_error`, `clear_last_error`, `fetch_history` (`party_manager.rs:354,364,843`)
    - `EditableField::insert_str` (`client/src/tui/input.rs:95`)
  - **Server test-only API:** `PartyState::post_file`, `post_file_dm`, `blob_bytes_for`, `staged_upload_bytes`, `set_max_member_blob_bytes` (`server/src/state/blobs.rs:1213,1248,1316,1406`, `mod.rs:358`). These are documented as convenience and test entry points, which is fine, but they are not gated.
- Impact:
  - Unused secret-handling helpers are attack surface and audit noise.
  - Test hooks such as `add_session_for_test` let any linked crate inject sessions and fingerprint senders, bypassing TOFU invariants.
  - The dead unsigned-invite generators keep the deprecated format alive.
- Recommendation:
  - Delete the dead crypto, identity and invite helpers, `run_client_session`, `remove_password` and `generate_invite_qr` (or wire QR into the TUI or desktop if it is wanted).
  - Add a `test-support` cargo feature on `p2pem-classic` and `messenger-core` and gate the `*_for_test` items with `#[cfg(any(test, feature = "test-support"))]`. Integration tests and the desktop's dev-dependency enable it. This is the standard way integration tests reach test-only API.
  - Either have the TUI call `deleting_contact_would_unblock` and expose it to the desktop (or put it on the contact DTO), or delete it and fix CLAUDE.md.
  - Run `cargo +nightly udeps` and a `pub`-item lint such as `unreachable_pub` periodically.
- Effort: S/M

### QUAL-10 — A complete second file receiver (async `IncomingFile`) is dead and has already drifted

- Severity: Medium
- Category: dead-code / duplication
- Status: VERIFIED
- Location: `core/src/transfer/receiver.rs:105-243` (`IncomingFile`: `start_meta`, `append_chunk`, `finalize`, `abort_cleanup`, `progress_percent`, `received`, `expected`). The production path is `IncomingFileSync` at `:245-365`.
- Problem:
  - About 140 lines duplicate the most security-sensitive receive path: sanitizing, the size cap, reserving a free name, and the rename.
  - Only tests use them, so tests cover a path users never run.
  - They have already drifted. `IncomingFile` spools to a separate `tmp_dir` and then `rename`s into `dest_dir`, which fails or becomes a copy across filesystems. That is exactly the bug CLAUDE.md says `IncomingFileSync` fixed by spooling *inside* the download dir.
  - Any future fix to `IncomingFileSync` has to remember a twin nobody runs.
- Recommendation: delete `IncomingFile` and move its tests that still matter (overflow-before-write, collision handling) onto `IncomingFileSync`.
- Effort: S

### QUAL-11 — Stringly-typed seams: transport carried as a `"p2p:"`/`"relay:"` prefix; roles and kinds mapped twice

- Severity: Medium
- Category: api-design / duplication
- Status: VERIFIED
- Location:
  - Transport: `core/src/network/relay.rs:257-261` builds `format!("p2p:{a}")` or `format!("relay:{relay_server}")` into `SessionEvent::Connected { peer: String }`, and `client/src/app/chat_manager/events.rs:781-784` (`is_hole_punched`) parses it back with `starts_with("p2p:")`.
  - Role and ChannelKind ↔ string:
    - `desktop/src-tauri/src/commands/party.rs:76-112` (`role_name`/`parse_role`/`kind_name`/`parse_kind`)
    - `client/src/tui/app/cmd_party.rs:346-348, 385-388` (a second, independent parser)
  - Chat kind, transport and trust state ↔ string: `desktop/src-tauri/src/commands/chats.rs:28-43` and `contacts.rs:33-40`.
  - `list_discovered_peers` returns ad-hoc `serde_json::json!` (`desktop/src-tauri/src/commands/connect.rs:80`).
- Problem:
  - Adding a variant (for example a `Role::Moderator` or a third transport) needs edits in three to four places, and nothing fails at compile time.
  - The TUI parser has no `"owner"` arm. That is deliberate, but it is undocumented and differs from the desktop's `parse_role` only by accident of copy.
  - Peer labels mix display text with a machine-readable flag.
- Recommendation:
  - Add `SessionEvent::Connected { peer: String, route: PeerRoute }` with `enum PeerRoute { Direct, HolePunched, Relayed { server: String } }` and delete `is_hole_punched`.
  - Give `Role`, `ChannelKind`, `ChatKind`, `Transport` and `TrustState` an `as_str()` and a `FromStr` in core (one table each).
  - Do **not** use `#[serde(rename_all)]` on `Role`/`ChannelKind`: `server/src/state/db.rs:519,535` persists `serde_json::to_string(&role)` = `"\"Member\""`, so renaming would break existing databases. Add an explicit `as_str`.
  - Replace `json!` DTOs with `#[derive(Serialize)]` structs.
- Effort: S/M

### QUAL-12 — Swallowed results that hide real failures (judged list)

- Severity: Medium (aggregate)
- Category: error-handling
- Status: VERIFIED
- **Harmful** (the result decides what the user sees, or the failure matters):
  - `client/src/tui/app/mod.rs:1142,1145`: `let _ = confirm_fingerprint(…)` and `let _ = reject_fingerprint(…)`, then an unconditional "Fingerprint accepted" toast. If the session died, the user is told the peer is trusted when nothing was recorded. This breaks the project's "never `Ok` + toast" rule.
  - `client/src/tui/app/mod.rs:1159-1162, 1202-1205`: `let _ = load_history_auto(…)`, with no log (QUAL-02).
  - `client/src/app/chat_manager/mod.rs:791`: `let _ = self.save_history(history_path);` in `clear_history`, then `tracing::info!("History cleared and saved")` even when the save failed.
  - `desktop/src-tauri/src/commands/connect.rs:37,85`: `Discovery::new().ok()` with no log. When mDNS cannot start, the UI reports `enabled: true, peers: []` forever and a new daemon is attempted on every poll.
  - `desktop/src-tauri/src/lib.rs:401` and `client/src/tui/app/mod.rs:137`: `create_dir_all(data).ok()`. The next error ("identity could not be saved") then hides the root cause.
  - `desktop/src-tauri/src/lib.rs:418-424`: when no data dir resolves, history and identity go to the **process working directory** (`PathBuf::from("history.json.enc")`) with only a `warn!` that nobody sees (QUAL-07). The TUI does the same (`client/src/tui/app/mod.rs:157-159` region).
  - `desktop/src-tauri/src/commands/party.rs:502` and `client/src/tui/app/cmd_party.rs:287`: `let _ = fetch_dm_history/refresh_files`. These are acceptable only because the next `server()` lookup fails, but an error should be returned or toasted.
- **Acceptable** (keep, ideally with a comment): channel sends to a dropped receiver (`session.rs` ×8, `party_manager.rs` ×12, `connect.rs:109-110`, `files.rs:202-241`), best-effort temp-file cleanup (`util.rs:62,68,79`, `receiver.rs:219,360`, `blobs.rs:181`, `support.rs:145,158` inside the panic hook), notifications (`mod.rs:593`), `app.emit` (`lib.rs:630-646,767`), `set_nodelay(...).ok()` (`punch.rs:357`), the idempotent `tracing` init (`lib.rs:330`), and typing indicators (`tui/app/mod.rs:874,883`).
- Recommendation:
  - Fix the harmful items.
  - For the acceptable ones, use a named helper (`fn send_or_closed<T>(tx, msg)`) or `.ok(); // receiver gone: the session is shutting down`, so a reviewer can tell intent from oversight.
  - Consider `#![warn(clippy::let_underscore_must_use)]` with targeted `#[expect]`.
- Effort: S

### QUAL-13 — `ChatManager` is a god object with 11 public mutable fields; validation happens only at load time

- Severity: Medium
- Category: maintainability / api-design
- Status: VERIFIED
- Location:
  - `client/src/app/chat_manager/mod.rs` struct (31 fields, 11 `pub`): `chats`, `contacts`, `contact_to_chat`, `toasts`, `config`, `history_key: Option<[u8;32]>`, `is_hosting`, `hosting_port`, `connection_password`, `conversation_locked`, `external_address`.
  - There are 69 `pub fn` across the `chat_manager/` submodules.
- Direct writes from the UIs:
  - `desktop/src-tauri/src/commands/auth.rs:232,370-377,400` (`mgr.config.* = …`)
  - `client/src/tui/app/cmd_contacts.rs:68` (`contacts.get_mut(&id)` to rename)
  - `client/src/tui/app/mod.rs:1295` (`cfg.download_dir = PathBuf::from(value)`)
- Problem:
  - `sanitize_loaded_config` (`client/src/app/persistence.rs:86-95`) rejects dangerous or relative `download_dir`/`temp_dir`, but only **on load**. Both UIs assign `download_dir` directly. A user who types `:set download-dir /usr/bin` in the TUI, or a relative path, gets it accepted and saved, then silently reset to the default on the next launch.
  - The desktop bridge's `state_signature` has to know every persisted field because mutations do not go through a method that could mark state dirty.
  - `history_key` and `connection_password` are public, `Copy`/`Clone`, and never zeroized. No crate other than core uses `zeroize` at all: user passwords received by `unlock`/`set_password` (`auth.rs:67,116`), Party join passwords and the server `--password` are plain `String`s.
- Recommendation:
  - Make the fields private with accessors. Add `ChatManager::update_config(|c| …) -> Result<()>`, which runs `sanitize`/`validate` and bumps a `revision: u64` counter. Add `rename_contact`.
  - The bridge's `state_signature` can then compare one `revision` instead of hashing everything (removing the class of bug described in CLAUDE.md "state_signature covers contacts too").
  - Store `history_key` as `Zeroizing<[u8;32]>` and passwords as `Zeroizing<String>` / `secrecy::SecretString`.
  - Longer term, split out the transfer state (`active_transfers`, `incoming_streams`, `pending_file_end`, …) into a `TransferManager`, and the TOFU queue (`fingerprint_verification_requests`, `fingerprint_confirm_senders`) into a `TofuQueue`, each owned by `ChatManager`.
- Effort: M (accessors) / L (decomposition)

### QUAL-14 — Logic duplicated between the TUI and the desktop shell, already drifting

- Severity: Low/Medium
- Category: duplication
- Status: VERIFIED
- Location:
  - **Invite candidate addresses** (UPnP external first, then LAN): `desktop/src-tauri/src/commands/contacts.rs:74-90` uses `format_host_port(&ip, port)` (IPv6-safe), while `client/src/tui/app/cmd_contacts.rs:82-100` uses `format!("{}:{}", ip, port)`.
  - **Data dir and identity bootstrap:**
    - `desktop/src-tauri/src/lib.rs:376-450`: `"P2PEM"`, fallback name `"User"`, error state.
    - `client/src/tui/app/mod.rs:130-160`: `"EncryptedMessenger"`, fallback name `"TUI_User"`, `?`.
    - `client/src/support.rs:86-91`: `"EncryptedMessenger"` hard-coded again.
  - **Diagnostics report construction:** `desktop/src-tauri/src/commands/auth.rs:~250-290` and `client/src/tui/app/mod.rs:1387-1420`.
  - **Auto-host / auto-connect after unlock:** `desktop/src-tauri/src/commands/auth.rs:91-110` versus the TUI startup path.
  - **Password floor pre-check:** `client/src/tui/app/mod.rs:1176` duplicates `Identity::encrypt`'s check. This is intentional for the message, but it could use the core error variant (QUAL-04).
  - **Fingerprint hashing:** `crypto.rs:75` and `identity/mod.rs:233` (QUAL-01).
- Recommendation:
  - Add `ChatManager::invite_candidate_addresses()`.
  - Add `app::bootstrap::{resolve_data_dir(app_id), open_identity(data_dir) -> Result<BootState>}` and `support::build_report(&ChatManager, &Identity, paths)`.
  - Add `ChatManager::run_startup_automation(pk)`.
  - Each shell then supplies only its app id and its UI.
- Effort: M

### QUAL-15 — Inconsistent mutex-poisoning policy: the server hub panics, the desktop recovers

- Severity: Low
- Category: best-practice
- Status: VERIFIED
- Location:
  - `server/src/hub.rs:46-49, 54, 62, 86, 110-113`: `std::sync::Mutex::lock().unwrap()` ×6.
  - Compare `desktop/src-tauri/src/lib.rs:74-87` (`lock_or_recover`, with a doc comment explaining why `unwrap` is wrong).
- Problem: a panic while the hub lock is held makes every later broadcast panic, killing every connection task one by one. That is the "permanent, cascading failure" the desktop comment describes.
- Recommendation: share one `lock_or_recover` helper from `messenger_core::util` and use it in both places, or switch the hub to `parking_lot::Mutex`, which does not poison.
- Effort: S

### QUAL-16 — `#![allow(dead_code)]` on the whole server state module is stale

- Severity: Low
- Category: dead-code
- Status: SUSPECTED (cannot compile to confirm; strong evidence)
- Location: `server/src/state/mod.rs:20-23`
- Analysis:
  - The justifying comment ("accessors … exercised by the tests now and consumed by the network runtime; allow them ahead of that wiring") describes wiring that has since landed.
  - `state` is a `pub mod` of a library crate, so its `pub` items can never trigger `dead_code`.
  - Every private (`pub(super)`/private) function in `state/{mod,blobs,db}.rs` has at least one production reference (scripted check).
  - Every field of `Member`, `Channel`, `FileRef`, `PendingUpload`, `AuditRecord`, `DmThread`, `BlobRecord` and `PartyState` is read at least twice in non-test code.
  - What is genuinely dead in production is only the five `pub` test entry points listed in QUAL-09, which the allow does not affect.
- Recommendation: delete the `#![allow(dead_code)]` and run `cargo clippy -p messenger-server`. If anything fires, apply a targeted `#[cfg_attr(not(test), expect(dead_code))]` or the `test-support` feature (QUAL-09).
- Effort: S

### QUAL-17 — Typestate expressed as `Option` plus `expect`

- Severity: Low
- Category: api-design
- Status: VERIFIED
- Location:
  - `server/src/state/db.rs:155-158` (`self.db.as_ref().expect("reload_from_db requires an open database")`)
  - `db.rs:247-250` (`blob_dir … expect`)
  - `server/src/state/mod.rs:422` (`state.db.as_ref().unwrap()`)
  - `PartyState` carries `db: Option<Connection>` and `blob_dir: Option<PathBuf>` (`mod.rs:315-317`), so every `persist_*` starts with `let Some(conn) = &self.db else { return };`.
- Recommendation: introduce a `trait Store` (or `enum Backing { Memory, Sqlite { conn, blob_dir } }`) with an in-memory no-op implementation. The `expect`s and the silent `else { return }` disappear, and tests can inject a failing store to cover QUAL-05.
- Effort: M

### QUAL-18 — Logging of identifying metadata at info and debug, with debug on by default

- Severity: Low
- Category: logging (privacy)
- Status: VERIFIED
- Location:
  - The default filters enable debug for the app crates:
    - `desktop/src-tauri/src/lib.rs:333`: `"info,p2pem_classic=debug,messenger_core=debug"`
    - `client/src/main.rs:80`: `"info,p2pem_classic=debug"`
  - Examples:
    - `client/src/app/chat_manager/events.rs:173`: `debug!("Handling session event … {:?}", event)` logs `SessionEvent` with derived `Debug`, so full fingerprints, the SAS and peer addresses.
    - `events.rs:100`: full old and new fingerprints on a mismatch.
    - `events.rs:182`: peer IP at info.
    - `events.rs:605`: peer filename at info.
    - `client/src/app/chat_manager/contacts.rs:36`: contact name at info.
    - `core/src/transfer/receiver.rs:211,352`: full saved path at info.
    - `core/src/identity/mod.rs:631`: identity name and path at info.
    - `desktop/src-tauri/src/lib.rs:763`: peer-supplied `peer_name` at info.
    - `client/src/main.rs:87`: `debug!(?args)` includes `--relay-token`. It is dropped by the logbuf, but printed by any `fmt` layer added later.
- Done well: `ProtocolMessage`'s hand-written `Debug` (`core/src/core/protocol.rs:82-…`) redacts text and chunk bodies, and `DiagnosticsReport` stores only a fingerprint *prefix*.
- Recommendation:
  - Default to `info` in release builds and keep debug for `cfg(debug_assertions)`.
  - Write a hand-made redacting `Debug` for `SessionEvent` and `PartyRequest`. `PartyRequest::Join` carries the community **password**: it is never logged today, but nothing stops a future `?req`.
  - Log fingerprint prefixes (`&fp[..8]`) and no IP addresses or paths at info.
  - Treat peer-supplied names as data, with fields escaped by `?`, not `%`.
- Effort: S

### QUAL-19 — Test fixtures copy-pasted across test binaries; port allocation is racy

- Severity: Low/Medium
- Category: testing
- Status: VERIFIED
- Location:
  - `free_port()` (bind `:0`, read the port, drop the listener, hope it is still free) ×5:
    - `core/src/network/punch.rs:390`
    - `core/src/network/relay.rs:782`
    - `core/tests/relay_e2e.rs:60`
    - `client/tests/file_transfer_e2e.rs:36`
    - `client/tests/reconnect_e2e.rs:28`
  - `pump_until` ×4 (`file_transfer_e2e.rs:62`, `reconnect_e2e.rs:47`, `party_e2e.rs:71`, `attack_trust_boundaries.rs:157`).
  - `manager_in` ×3 and `start_server` ×2 (`party_e2e.rs:52`, `attack_trust_boundaries.rs:141`).
  - `wait_until` / `awaiting_confirmation` ×2 (`core/tests/session_e2e.rs:22-31`, `relay_e2e.rs:27-34`).
  - `ChatManager::new(Config::default())` appears 70 times.
  - About 140 fresh RSA-2048 key generations across tests (`core/src/identity/mod.rs` 36, `server/src/connection.rs` 31, `client/tests/party_e2e.rs` 16, `chat_manager/tests.rs` 15, …), with no cached fixture.
  - 21 separate integration-test binaries in `client/tests/`, each linking the whole crate. `client/tests/bug_diagnosis.rs:3` wraps its tests in a redundant `#[cfg(test)] mod`.
- Impact:
  - `free_port` is a classic TOCTOU flake under nextest's process-per-test parallelism.
  - Repeated key generation dominates debug test time.
  - Every fix to `pump_until` (timeouts, diagnostics) has to be made four times.
- Recommendation:
  - Create a `test-support` module or crate (`messenger-test-support`, a dev-dependency) with:
    - `bind_loopback() -> (TcpListener, u16)`, passing the bound listener into the server (add `run_relay_server_on(listener)` the way `run_accept_loop` already takes a listener);
    - one generic `pump_until`;
    - `test_identity(n)` backed by a `static KEYS: OnceLock<[RsaPrivateKey; 4]>`;
    - `manager_in`.
  - Merge `client/tests/*.rs` into one `tests/it/main.rs` with modules to cut link time.
- Effort: M

### QUAL-20 — Synchronization by sleeping, absence checks bounded by time, and an empty test

- Severity: Low
- Category: testing
- Status: VERIFIED
- Location:
  - **Startup by sleeping:** `core/src/network/relay.rs:785-793` (`start_relay`: `free_port`, spawn, `sleep(200ms)`, assume it is listening), plus `relay.rs:818,842`.
  - **Timing-dependent assertions:** `client/src/app/chat_manager/tests.rs:1258,1282` (backpressure: `sleep(200ms)` then assert bytes in flight ≤ cap, and `sleep(100ms)` then assert progress).
  - **Negative checks that pass by waiting long enough:** `client/tests/party_e2e.rs:350-366` and `attack_trust_boundaries.rs:257,287,402-408,452` ("poll 40 × 25 ms, then assert the private channel never arrived"). These pass trivially if the server is merely slow, and a leak arriving after 1 s goes unnoticed.
  - **Empty test:** `core/src/network/session.rs:2195-2210` `test_issue_6_replay_protection_complete` has no body, only comments. They claim "Phase 3 (Key Rotation) — FUTURE WORK", but rekeying shipped (`REKEY_MESSAGE_COUNT`, `client/tests/key_rotation_tests.rs`).
- Recommendation:
  - Pass bound listeners instead of sleeping.
  - For absence checks, send a *sentinel* message after the one that must not leak, wait until the sentinel arrives, then assert the forbidden item is absent. Ordering on one connection makes this deterministic.
  - For the backpressure test, poll until progress stops changing, with a deadline, instead of a fixed sleep.
  - Delete the empty test.
- Effort: S/M

### QUAL-21 — Persisted config fields with no consumer; a security-relevant knob nobody can set

- Severity: Low
- Category: dead-code
- Status: VERIFIED
- Location:
  - `core/src/types.rs:393-440`:
    - `theme: Theme` (5 variants), `font_size: u8` and `show_log_terminal: bool` are read only by the diagnostics export (`client/src/support.rs:17-38`) and printed in the TUI settings list (`client/src/tui/overlays.rs:370`). They look like egui leftovers; the desktop keeps its theme in the frontend.
    - `auto_trust_on_first_use: bool` (`types.rs:409`) gates a TOFU bypass at `client/src/app/chat_manager/events.rs:111`, but no UI or command can set it, so the branch is reachable only by editing the encrypted history.
- Recommendation:
  - Drop the UI fields from core `Config`. `serde(default)` keeps old files loading, and `#[serde(skip_serializing)]` can be used during a deprecation window.
  - Either delete `auto_trust_on_first_use` and its branch, or document it as a test-only knob behind `test-support`. A dormant TOFU bypass is a liability.
- Effort: S

### QUAL-22 — Diagnostics and crash dir hard-coded to the TUI's ProjectDirs

- Severity: Low
- Category: maintainability
- Status: VERIFIED
- Location: `client/src/support.rs:86-91` (`default_diagnostics_dir` → `ProjectDirs("com","chat-p2p","EncryptedMessenger")`) and `:144` (the panic hook writes there).
- Problem: CLAUDE.md requires the two apps to keep separate data dirs. Any future use of `install_panic_hook` from the desktop (QUAL-07) would write crash reports into the TUI's directory.
- Recommendation: make `install_panic_hook(dir: PathBuf)` and `default_diagnostics_dir(app_id)` take the directory or app id.
- Effort: S

### QUAL-23 — `expect` on the AEAD nonce counter

- Severity: Info
- Category: error-handling
- Status: VERIFIED
- Location: `core/src/core/crypto.rs:616-660` (`AesCipher::encrypt` → `expect("AES-GCM nonce counter exhausted; rekey required")` and `expect("AES-GCM encryption should not fail")`).
- Note:
  - The counter is a `u64` and the session rekeys every 100 messages, so exhaustion is unreachable in practice.
  - The API still returns `Vec<u8>`, not `Result`, so the impossibility rests on a protocol invariant enforced elsewhere.
  - With `panic = "abort"`, any future regression (for example a rekey path that keeps the counter) would abort the process instead of erroring the session.
- Recommendation: return `Result<Vec<u8>>`. The callers are already fallible.
- Effort: S

### QUAL-24 — Cargo manifests: unused dependencies, broad features, stale metadata

- Severity: Low
- Category: dependency
- Status: VERIFIED by `rg` over `use`/path references; `cargo udeps` was not run
- Location and items:
  - **Unused:**
    - `thiserror` (workspace dependency used by no crate)
    - `tokio-test` (dev-dependency of `core` and `client`; no `tokio_test::` anywhere)
    - `tauri-plugin-log`, `log`, `anyhow` in `desktop/src-tauri/Cargo.toml` (QUAL-07)
  - **Broad features:** `tokio = { features = ["full"] }` is inherited by `core`, which needs `net`, `io-util`, `time`, `sync`, `rt`, `fs`, `macros`. `full` pulls `process`, `signal` and `parking_lot` into the library.
  - **Duplicate versions in `Cargo.lock`:** `rand` 0.8/0.9/0.10, `rand_core` 0.6/0.9/0.10, `getrandom` 0.2/0.3/0.4, `base64` 0.21/0.22/0.23, `hashbrown` 0.12/0.16/0.17, `thiserror` 1/2, `syn` 1/2/3, `windows-sys` 0.45/0.52/0.59/0.61. Most come in transitively (Tauri and the RustCrypto 0.10/0.11 split); the workspace comment on `x25519-dalek` already explains one of them. Worth a `cargo deny check bans` with `multiple-versions = "warn"` and an explicit `skip` list, so new duplicates are noticed.
  - **Metadata:**
    - `authors = ["fibo3090 <fibo3090@example.com>"]` is a placeholder address.
    - No `repository`, `homepage`, `readme` or `keywords`.
    - No `publish = false` on four crates that are not meant for crates.io.
    - `p2pem-desktop`'s description still says "Phase 1 bridge".
    - The `p2pem-classic` dependency comment in the desktop manifest still mentions egui.
    - The server binary is named generically: `[[bin]] name = "server"`.
- Recommendation:
  - Remove the unused dependencies.
  - Give core an explicit tokio feature list.
  - Add `[workspace.package] repository = "https://github.com/fibo3090-code/secure-p2p-chat"` and `publish = false`.
  - Rename the server binary to `p2pem-party-server`.
  - Refresh the stale comments.
- Effort: S

### QUAL-25 — Long functions concentrated in protocol state machines

- Severity: Low
- Category: maintainability
- Status: VERIFIED (metrics above)
- Location: `PartyServerConn::apply` (213), `handle_member_request` (204), `reload_from_db` (199), `serve_connection` (165), `handle_session_event` (165), `run_message_loop` (133), `send_file` (125), `connect_and_join` (121).
- Recommendation:
  - `apply` and `handle_member_request` are large `match`es over response and request enums. Split each arm into a `fn on_<variant>(&mut self, …)`, the pattern `events.rs` already uses partly (`receive_file_offer`, `handle_peer_frame`).
  - `reload_from_db` naturally splits into `load_members`, `load_channels`, `load_dm_threads` and `load_blobs`.
  - `client/src/app/party_manager.rs` (2,236 lines) should become `party_manager/{mod,conn,apply,transfers}.rs`, like `chat_manager/`.
- Effort: M

### QUAL-26 — No doctests while CI runs `cargo test --doc`

- Severity: Info
- Category: testing
- Status: VERIFIED (no Rust code fences in any `src/`)
- Recommendation: either add examples to the few genuinely public entry points (`parse_host_port`, `sanitize_filename`, `derive_sas`, `connect_and_join`), which turns docs into checked tests, or drop the CI step to save a build.
- Effort: S

### QUAL-27 — No workspace lint policy; selected clippy lints worth adopting

- Severity: Info
- Category: best-practice
- Status: VERIFIED (no `[workspace.lints]`, `clippy.toml` or crate-level `#![deny]`/`#![forbid]`)
- Recommendation: add to the workspace `Cargo.toml`:

  ```toml
  [workspace.lints.rust]
  unsafe_code = "forbid"          # zero unsafe today; keep it that way
  unreachable_pub = "warn"        # surfaces QUAL-09 automatically
  [workspace.lints.clippy]
  unwrap_used = "warn"            # allow in tests via clippy.toml allow-unwrap-in-tests = true
  expect_used = "warn"            # same; existing invariant expects get #[expect(..., reason = "...")]
  string_slice = "warn"           # would have caught QUAL-01
  indexing_slicing = "warn"       # enable per-module in core/src/core, identity, party decoders
  let_underscore_must_use = "warn" # QUAL-12
  cast_possible_truncation = "warn" # 80 `as` casts in prod; DB i64<->u64 and usize<->u32 sites
  large_enum_variant = "warn"
  needless_pass_by_value = "warn" # RsaPrivateKey / String passed by value through the session runners
  too_many_lines = "warn"         # with too-many-lines-threshold = 120 in clippy.toml
  ```

  Then add `[lints] workspace = true` to each member. Turn the warnings into a CI gate gradually, per crate.
- Effort: S (config) + M (cleanup)

### QUAL-28 — Non-atomic channel removal in `delete_channel` *(cross-reference)*

- Severity: Info
- Category: error-handling
- Status: VERIFIED
- Location: `server/src/state/mod.rs:934-960`. In-memory removal happens first, then `delete_channel_rows` runs three unconnected `DELETE`s (`db.rs:91-103`), then positions are rewritten.
- Note: this is the concrete instance of QUAL-05 most likely to leave a restart showing a half-deleted channel. Fix it in the same transaction work.

### QUAL-29 — Documentation drift in CLAUDE.md affecting maintainers

- Severity: Info
- Category: maintainability
- Status: VERIFIED
- Items:
  - "root `src/` and `tests/` are vestigial — ignore them": these directories do not exist.
  - "`deleting_contact_would_unblock` exists so the dialog can say so": no caller (QUAL-09).
  - The desktop `Cargo.toml` comment about egui is stale.
- Recommendation: correct these in the same PR as the code changes, following the repo's "update the canonical doc" rule.

## Refactoring roadmap (ordered, with dependencies)

1. **Immediate fixes** (independent, S each; ship first):
   1. QUAL-01: make `fingerprint_pubkey` parse instead of slicing, delete the duplicate, add a PEM-first check in `decode_unsigned`, and add a proptest.
   2. QUAL-02: a failed history load leaves no key installed and keeps a `.unreadable` backup, plus a regression test.
   3. QUAL-12 (harmful items): the TUI fingerprint toast, the `clear_history` log, and the `Discovery::new()` log.
   4. QUAL-07 (part): install a panic hook and a log buffer in the desktop app, and drop the unused log dependencies.
2. **Lint policy** (QUAL-27). Add `[workspace.lints]` with `forbid(unsafe_code)`, `string_slice`, `let_underscore_must_use` and `unreachable_pub` as warnings. This turns several later steps into compiler-guided work. It depends on nothing.
3. **Dead code removal** (QUAL-09, QUAL-10, QUAL-16, QUAL-21, QUAL-24). Introduce the `test-support` feature first, move the `*_for_test` items behind it, delete the dead crypto, invite and receiver code, remove `#![allow(dead_code)]`, and drop the unused dependencies. This depends on step 2 for `unreachable_pub` guidance.
4. **`SessionChannels` / `SessionParams` in core** (QUAL-03). Depends on step 3 (`run_client_session` is already gone) and removes the 8 `allow`s. Do it together with QUAL-11's `PeerRoute` on `SessionEvent::Connected`, since both touch the same runner signatures.
5. **Typed errors** (QUAL-04), module by module: core identity → core session/party decode → server state (`PartyError`) → desktop `CmdError`. The server part is a prerequisite for step 6.
6. **Server storage correctness** (QUAL-05, QUAL-17, QUAL-28). Add the `Store` abstraction, fallible `persist_*` returning `PartyError::Storage`, transactions, WAL and `user_version` migrations, and optionally a writer thread. Needs step 5.
7. **App and TUI crate split** (QUAL-08). Extract `p2pem-app`, and make the desktop depend on it alone. Do this before step 8 so the shared helpers land in the new crate.
8. **De-duplicate shell logic** (QUAL-14, QUAL-22): bootstrap, invite addresses, diagnostics report, startup automation. These go into `p2pem-app`.
9. **`ChatManager` encapsulation** (QUAL-13): private fields, `update_config` with validation, zeroized secrets, and a `revision` counter replacing `state_signature`. Then decompose `TransferManager` and `TofuQueue`. Needs steps 7 and 8, so the UIs only call accessors.
10. **Logging unification** (QUAL-06, QUAL-18): the logbuf field visitor, a structured-field style, redacting `Debug`, and release default `info`. Can run in parallel from step 2 onward.
11. **Test infrastructure** (QUAL-19, QUAL-20, QUAL-26): a shared test-support crate with bound listeners, cached keys and one `pump_until`, sentinel-based absence checks, and merged integration binaries. Easier after step 4 (`SessionChannels::new()` serves as a fixture).
12. **Long-function splits** (QUAL-25): last, once types and boundaries are stable.

## Things done well

- **No `unsafe` anywhere** and almost no bare `unwrap()` in production. The `expect`s that remain state a real invariant in their message ("checked above", "just inserted").
- **Panic and poisoning awareness in the desktop bridge:** `lock_or_recover` comes with a precise rationale, and the identity-load error state never regenerates keys.
- **Redaction where it matters most.** `ProtocolMessage` has a hand-written `Debug` that hides message text and chunk bytes, and diagnostics carry only a fingerprint prefix.
- **One address parser** (`messenger_core::util::parse_host_port`) used by the TUI, the Party client, contacts, invites and the relay, with a dedicated adversarial test file (`core/tests/adversarial_parsing.rs`).
- **A shared session spawner.** `ChatManager::spawn_session` puts failure reporting (`Error` followed by `Disconnected`) in one place for every session kind, and the client-side `SessionChannels` is the right abstraction, just one layer too high (QUAL-03).
- **Atomic file writes** (`write_file_atomic`), the escalating disposal of plaintext history (delete, then truncate, then an explicit error toast), and the server's three-phase blob upload with reservations. These are careful, well-documented designs.
- **Tests that exercise real objects.** The end-to-end suites drive real `ChatManager`s and a real in-process community server over loopback. Property tests assert *byte* fixpoints, and the fuzz seed corpus is guarded by a test. Error-path coverage (adversarial and attack-boundary files) is broad.
- **Comments explain *why*,** usually with the bug that motivated the code. That makes the remaining refactors much safer than in a typical codebase this size.
- **TODO/FIXME count is zero:** deferred work is tracked in docs rather than in rotting inline markers.
