# Tauri 2 desktop shell (Rust bridge) and its security configuration

## Scope covered

Read in full:
- `desktop/src-tauri/src/lib.rs` (780 lines), `src/main.rs`, `src/commands/{mod,auth,chats,connect,contacts,party}.rs`, and `src/tests.rs` (structure plus the binding and auth tests)
- `desktop/src-tauri/Cargo.toml`, `build.rs`, `tauri.conf.json`, `capabilities/default.json`, and the generated ACL schema (`gen/schemas/desktop-schema.json`, used to expand `core:default`)
- `desktop/index.html`, `desktop/vite.config.js`, `desktop/src/lib/csp.js`, and the built `desktop/dist/index.html` (the CSP meta tag)
- `desktop/src/lib/bridge.js` and `desktop/src/lib/invokeContract.js`. Every one of the 66 `invoke()` calls was checked against its Rust signature.

Read where a bridge behaviour depends on them:
- `client/src/app/chat_manager/{connect,files,mod,events,contacts}.rs`
- `client/src/app/party_manager.rs` (`connect_and_join`)
- `core/src/identity/mod.rs` (encrypt, decrypt, change_password)
- `core/src/network/session.rs` (password gate)
- `core/src/party/mod.rs` (`PartyClient::connect`)
- `core/src/util.rs` (`write_file_atomic`, `guess_mime`)
- `client/src/app/persistence.rs`
- `desktop/src/App.jsx` (the refresh fan-out)
- `.github/workflows/release.yml` (signing)
- the workspace `[profile.release]`

No code was built or run.

## Summary

The bridge is in good shape. Its comments explain the reasons behind the code, and the reasons usually hold up.
- **Path arguments are never accepted from the webview.** Files are addressed by (chat id, message id).
- **Gating is consistent.** Every non-auth command calls `ensure_ready`; the scripted check found only `auth_status` and `my_identity` ungated, both by design.
- **The CSP is strict** and drift-checked against `tauri.conf.json`.
- **No plugin opens a filesystem, shell or opener scope.**
- **Devtools are compiled out of release builds.**
- **History writes are coalesced**, and they go through an atomic write with a unique temp file.
- **The JS/Rust argument names all match.** All 66 invoke key sets agree with the Rust parameter names, and the only registered command never invoked is `my_identity`.

The real problems are:
0. **(High) A history file that fails to load is silently overwritten with empty state on the first poll tick after unlock (TAURI-30).** The identity file has this protection; the history file does not. Related backup problems:
   - `export_identity` can truncate `identity.json` to zero bytes if the destination is the source. This was reproduced against `std::fs::copy` on Linux (TAURI-31).
   - Any export, to any location, marks the identity "Backed up".
   - The backup covers only the identity (TAURI-33).
   - There is no restore flow (TAURI-32) and no "delete all data" flow (TAURI-34).

   The coordinator's extra items (1)–(4) are addressed in TAURI-31 to TAURI-34. Item (1) as phrased ("the UI promises identity AND history") is refuted: no screen makes that promise, so the gap is an omission.
1. **(High) The community two-step trust check can be bypassed.** The second `party_join` call (`trust: true`) does not bind to the fingerprint the user just verified. A MITM who lets the first connection through untouched and intercepts the second receives the community username and password.
2. **`set_password` works on an already-ready identity.** This bypasses `change_password`'s current-password check. `unlock` can also be re-entered, which reloads history over live state.
3. **The single `connection_password` field is shared between hosting and dialling.** The user's host password is sent to any peer they then dial that claims a password is required (contacts, auto-reconnect). After a manual connect, an auto-rehost silently loses the password.
4. **Lock and runtime hygiene.**
   - `party_join` holds the party mutex across an untimed TCP connect, which stalls the whole poll loop (all P2P events, TOFU prompts and saves) for as long as the connect takes.
   - Argon2 runs on async workers under a std mutex.
   - There is no single-instance guard, so two copies race on one history file.
5. **Defence in depth on received files is thin.** No Mark-of-the-Web or quarantine attribute is set, and the "executes on open" extension list misses several well-known launchers.

## Findings — ordered by severity

### TAURI-01 — The verified community fingerprint is not bound to the credential-sending connection
- Severity: High
- Category: security
- Status: VERIFIED
- Location:
  - `desktop/src-tauri/src/commands/party.rs:312-349` (`party_join`; pin lookup at 332-336; `connect_and_join(..., pinned.as_deref(), trust.unwrap_or(false))` at 339-348)
  - `client/src/app/party_manager.rs:403-428`
  - `desktop/src/lib/bridge.js:167-168`
- Problem:
  - Step 1 (`trust: false`) opens a connection, returns `{status:"verify", fingerprint, sas}` and hangs up.
  - Step 2 (`trust: true`) opens a brand-new TCP connection. For a first join, `pinned` is `None`, because `parties.json` is written only after `Joined`. `connect_and_join` therefore takes the `None => {}` arm (party_manager.rs:428) and sends the `Join` frame, carrying the username and password, to whatever key answers.
  - Nothing carries step 1's fingerprint into step 2. `party_join` has no parameter for it, and bridge.js does not send one.
- Impact: This defeats the documented "no credential moves before step two" property. An on-path attacker passes the first connection through untouched, so the user sees and verifies the genuine SAS. The attacker then intercepts the second connection and receives the credentials, and the attacker's key is what gets pinned once it answers `Joined`.
- Recommendation:
  - Add a single-word `fingerprint: Option<String>` parameter to `party_join`.
  - Have the UI pass back the fingerprint it showed.
  - When `trust == Some(true)`, require `fingerprint` and pass it as `expected_fingerprint` (`pinned.or(fingerprint)`). `connect_and_join` then refuses a different key before writing `Join`.
  - Add a bridge test that `trust: true` without a fingerprint is refused.
  - The TUI `:party-trust` path should get the same check (outside this scope).
- Effort: S

### TAURI-30 — A history file that fails to decrypt or parse is silently overwritten with empty state after unlock
- Severity: High
- Category: bug (data loss)
- Status: VERIFIED
- Location:
  - `desktop/src-tauri/src/commands/auth.rs:79-83` (`unlock`: `set_history_key(key)`, then a `load_history_auto` failure is only `tracing::warn!`ed)
  - Same pattern in `set_password` at `auth.rs:135-139`
  - `client/src/app/persistence.rs:205-215, 298-301` (an error leaves the in-memory state empty)
  - `persistence.rs:315-318` (`history_snapshot` needs only the key)
  - `desktop/src-tauri/src/lib.rs:626-628, 728-735` (first poll tick: `saved_sig` is `None` ≠ the current signature, so `persist_history` runs)
- Problem: When `history.json.enc` exists but `HistoryFile::load_encrypted` fails, unlock still succeeds and the history key is installed. Within about 250 ms the poll loop saves the empty manager state over the file. Settings are reset to defaults in the same write, since config lives inside it. Ways to reach this:
  - AEAD failure from bit-rot or a partial disk restore
  - a serde shape change after a downgrade to an older build
  - an `identity.json` restored "over the file" (see TAURI-32) that is not the identity which wrote the history
  - any future history-key derivation change (which `history_key_derivation_is_frozen` guards against, but cannot recover from)

  This is exactly the rule CLAUDE.md states for `identity.json` ("never replace an identity that merely failed to *load*"), and it is not applied to the history file.
- Impact: Permanent, silent loss of every conversation, contact, trust state and setting. A possibly recoverable file is destroyed and replaced by an empty one, and the user sees a "fresh" app.
- Recommendation:
  - On a load error for an *existing* file, do not install the history key for saving. Alternatively, set a `history_load_failed` flag that makes `history_snapshot`/`persist_history` refuse, and move the unreadable file aside (`history.json.enc.unreadable-<ts>`) before allowing any write.
  - Surface the condition to the UI (an `auth_status.state == "error"`-style screen, or a persistent banner) instead of a log line.
  - Add a bridge test: write garbage to `history.json.enc`, unlock, run a poll or save, and assert the bytes are unchanged.
- Effort: S

### TAURI-02 — `set_password` re-keys an unlocked, already-protected identity without the current password; `unlock` is re-entrant
- Severity: Medium
- Category: security
- Status: VERIFIED
- Location:
  - `desktop/src-tauri/src/commands/auth.rs:114-142` (`set_password`: only `ensure_identity_loaded`, then `encrypt`/`save`)
  - `auth.rs:65-86` (`unlock`)
  - `core/src/identity/mod.rs:263-313` (`encrypt` needs only the in-memory plaintext key)
- Problem:
  - `set_password` never checks `needs_password_setup()`. Once the app is ready, the plaintext key is in memory, so `encrypt(new)` succeeds and `save` overwrites `identity.json` under a password the caller chose.
  - `change_password` exists precisely to require `current`, and its core doc says this protects against "anyone who reaches an unlocked machine". `set_password` sidesteps that.
  - `unlock` similarly never checks `is_locked()`. A second call (a double-submit racing on the identity mutex, or any caller in the webview) re-runs Argon2 and then `load_history_auto`. `apply_history` *replaces* state, so up to `HISTORY_SAVE_MIN_INTERVAL` of unsaved messages and any not-yet-persisted incoming chats are dropped while their sessions stay live. `auto_host_if_configured` then runs again.
- Impact: Script running in the webview (XSS, or a compromised dependency) can lock the owner out of their identity, and with it their whole history. A UI double-submit can drop recent messages.
- Recommendation:
  - In `set_password`, return `Err("Password already set — use change password")` unless `state.needs_password_setup()`.
  - In `unlock`, return `Ok(())` early (or `Err`) when `!is_locked()`.
  - Add a bridge test for each.
- Effort: S

### TAURI-03 — One `connection_password` serves both hosting and dialling: the host password leaks to dialled peers and is lost on auto-rehost
- Severity: Medium
- Category: security / bug
- Status: VERIFIED
- Location:
  - `desktop/src-tauri/src/commands/connect.rs:25` (`start_host` sets it), `:63` (`connect_peer` overwrites it)
  - `desktop/src-tauri/src/commands/contacts.rs:186-204` (`connect_contact` dials with whatever is set)
  - `client/src/app/chat_manager/connect.rs:185, 344`
  - `desktop/src-tauri/src/lib.rs:711` (auto-rehost calls `start_host`)
  - `core/src/network/session.rs:732-737` (the client sends the password whenever the host says `required`)
- Problem:
  - (a) A user hosts with password P, then clicks Connect on a contact. `connect_to_contact` → `connect_to_host_candidates` reads `self.connection_password` (= P), and `client_password_gate` sends it to any host that sets the "required" flag. The same applies to `auto_reconnect_contacts`. The gate runs before TOFU, so the remote learns P before the user has verified who they are.
  - (b) If the user later calls `connect_peer` with no password, the field becomes `None`. The next auto-rehost (lib.rs:711 → connect.rs:185) then listens with no password, silently dropping the host protection after the first accepted peer.
- Impact: The host password is disclosed to arbitrary peers, and password protection is silently downgraded.
- Recommendation:
  - Split the field into `host_password` (captured by `start_host` and reused by rehost) and a per-call client password passed explicitly into `connect_to_host*`.
  - In the desktop bridge, never let `start_host`'s password reach a dial path.
- Effort: M

### TAURI-04 — `party_join` holds the party mutex across an untimed TCP connect and handshake, freezing the whole poll loop
- Severity: Medium
- Category: bug / performance / ux
- Status: VERIFIED
- Location:
  - `desktop/src-tauri/src/commands/party.rs:338-349`
  - `client/src/app/party_manager.rs:403-404` (`TcpStream::connect` with no timeout)
  - `desktop/src-tauri/src/lib.rs:605` and `:661-662` (every tick first awaits `self.party.lock()`)
- Problem: The poll loop is sequential and begins each tick with `poll_parties`, which awaits the party mutex. While `party_join` dials an unreachable or blackholed address, the loop stops. That means:
  - no P2P `poll_session_events`
  - no toasts
  - no TOFU prompts (peers wait on a prompt nobody sees)
  - no history saves
  - every `party_*` command hangs as well

  `TcpStream::connect` is bounded only by the OS (about 21 s on Windows, about 2 minutes on Linux with default SYN retries). The handshake adds its own timeouts on top.

  When the lock is released, `tokio::time::interval` with the default `MissedTickBehavior::Burst` (lib.rs:598) replays the missed ticks back to back.
- Impact: The app appears frozen for up to minutes after a typo in a community address, and P2P sessions starve.
- Recommendation:
  - Do the connect and handshake *outside* the `PartyManager` lock: build the `PartyClient` first, then lock only to register the connection. Alternatively, split `connect_and_join` into a lock-free dial and a locked insert.
  - Wrap the dial in `tokio::time::timeout` (for example 10 s).
  - Set `interval.set_missed_tick_behavior(MissedTickBehavior::Skip)`.
  - Consider running P2P polling and party polling as independent tasks.
- Effort: M

### TAURI-05 — No single-instance guard: two app instances race on one identity, history file and port
- Severity: Medium
- Category: bug
- Status: VERIFIED (absence)
- Location: `desktop/src-tauri/src/lib.rs:352-372` (no `tauri-plugin-single-instance`); `Cargo.toml` (not a dependency); no lock file anywhere (`rg flock|lockfile|single_instance` finds nothing)
- Problem:
  - A second launch (double-click, autostart plus manual start) loads the same `identity.json` and `history.json.enc`. Each process rewrites the *whole* encrypted history from its own in-memory copy, so the last writer wins and messages received only by the other instance are destroyed. `write_file_atomic` prevents torn files, not lost updates.
  - Both instances are the same peer, and both may auto-host on the same port.
  - `change_password` in one instance leaves the other holding an identity that no longer matches the file.
- Impact: Silent history loss and confusing connection behaviour.
- Recommendation:
  - Register `tauri-plugin-single-instance`, focusing the existing window on a second launch.
  - Additionally, take an advisory lock on `<data_dir>/.lock` in `init_bridge`, so `P2PEM_DATA_DIR` test instances in different directories still work.
- Effort: S

### TAURI-06 — Received files carry no Mark-of-the-Web or quarantine attribute, and the "executes on open" list has gaps
- Severity: Medium
- Category: security
- Status: VERIFIED (MOTW absence, list contents); SUSPECTED (content-sniffing behaviour of `xdg-open`)
- Location:
  - `desktop/src-tauri/src/commands/chats.rs:357-428` (`RISKY_EXTENSIONS`, `execution_risk`), `:452-482` (`open_file`)
  - Mirror list: `desktop/src/lib/bridge.js:54-64`
  - No `Zone.Identifier` / `com.apple.quarantine` anywhere in the workspace
- Problem:
  - Once the user clicks through the single in-app confirmation, the OS opens a peer-supplied file *without* its own protections: SmartScreen, Office Protected View, Gatekeeper.
  - The extension list omits well-known launchers and containers, which open without any confirmation at all:
    - Windows: `.iso .img .vhd .vhdx` (mounting bypasses MOTW on older systems and shows an `.exe` inside), `.appref-ms .settingcontent-ms .library-ms .search-ms .diagcab .appinstaller .msix .appx .xll .wsc .sct .vb .ws .ps1xml .psc1 .jnlp .website .mde .cab`, plus macro documents (`.docm .xlsm .pptm`).
    - HTML (`.html .htm .mht .svg`) opens in a browser with a `file://` origin.
    - macOS: `.terminal .tool .scpt .applescript .workflow .webloc .inetloc .fileloc .dmg .mpkg`.
  - On Linux, a file with no extension and no exec bit is MIME-sniffed by `xdg-open`, so a PE file can be handed to Wine if Wine is registered.
- Impact: One click on a peer's file card can execute attacker-controlled content.
- Recommendation:
  - Write `Zone.Identifier` (ZoneId=3) on Windows and `com.apple.quarantine` on macOS when a received file is finalized. This belongs in the core receiver, but `open_file` depends on it.
  - Extend the list, or invert it to an allow-list of known-inert types (images, pdf, txt, media) that open without confirmation; everything else asks.
  - Keep bridge.js generated from, or tested against, the Rust list.
- Effort: M

### TAURI-07 — Argon2 runs on async runtime workers under a std mutex; sync commands block the main thread on it
- Severity: Medium
- Category: performance / ux
- Status: VERIFIED (code); SUSPECTED (size of the visible stall)
- Location:
  - `desktop/src-tauri/src/commands/auth.rs:71-78` (`unlock`: `decrypt` under `lock_identity`)
  - `auth.rs:120-131` (`set_password`: `encrypt` + fsync `save` + `decrypt`, two Argon2 runs at 64 MiB × 3 under the lock)
  - Sync (main-thread) commands that take the same lock: `auth_status`, `my_identity`, `pending_fingerprint` (`connect.rs:240`), `set_display_name`
  - `lib.rs:694` (`auto_rehost` takes the same std mutex inside the async poll task)
- Problem: `change_password` correctly uses `spawn_blocking` on a clone. `unlock` and `set_password` do not. While they run (roughly 0.5–2 s), the tokio worker is pinned and the identity mutex is held. Any sync Tauri command, which in Tauri 2 runs on the **main/UI thread**, and the poll loop's `auto_rehost` block on that mutex.
- Impact: UI freezes during unlock or setup, and the poll loop stalls.
- Recommendation:
  - Mirror `change_password`: clone the identity, run Argon2 in `spawn_blocking`, then swap the result in.
  - Mark the sync commands `#[tauri::command(async)]`, or make them `async`, so they never run on the main thread.
- Effort: S

### TAURI-08 — mDNS keeps advertising and browsing after the user turns it off or hosting stops
- Severity: Medium
- Category: security (privacy)
- Status: VERIFIED
- Location:
  - `desktop/src-tauri/src/commands/auth.rs:377` (`update_settings` only flips `config.enable_mdns`)
  - `connect.rs:34-44` (register on `start_host`), `:78-90` (lazy `Discovery::new()` and browse), `:132-138` (unregister only on `set_locked(true)`)
- Problem:
  - Turning `enable_mdns` off never calls `Discovery::unregister()` and never drops the `Discovery` daemon in `state.discovery`, so the user's display name and port stay advertised and the browse daemon keeps running. `list_discovered_peers` merely stops *reporting* results.
  - Hosting that ends any other way (a rehost failure with "Stopped hosting", or a relay host) also leaves the registration up, advertising a dead address.
- Impact: The opt-out does not take effect, and presence leaks onto the LAN against the setting's documented purpose ("it reveals presence on the LAN").
- Recommendation:
  - In `update_settings`, when `enable_mdns` goes from true to false, call `unregister()` and set `*state.discovery = None` (also clear `discovered`).
  - Have the poll loop unregister when `!is_hosting`.
- Effort: S

### TAURI-09 — Every `state-updated` re-serializes the whole open conversation, up to 4×/s during transfers
- Severity: Medium
- Category: performance
- Status: VERIFIED
- Location:
  - `desktop/src-tauri/src/lib.rs:474-478` (transfer progress is in `ui_signature`), `:639-642` (emit)
  - `commands/chats.rs:188-199` (`get_conversation` returns the full `Chat` via `serde_json::to_value`)
  - `desktop/src/App.jsx:195-213` (each `state-updated` calls `listConversations`, `listTransfers`, `pendingFingerprint`, `lockState` and `getConversation(active)`)
- Problem: Transfer byte counts change every tick, so during any transfer the webview refetches and re-renders the entire active thread four times a second. Individual messages can be up to about 24 MiB, conversations are unbounded, and all of it crosses IPC as JSON while the manager lock is held for the serialization.
- Impact: CPU and battery drain, UI jank, and lock contention with the poll loop on long conversations.
- Recommendation:
  - Emit a separate `transfers-updated` event (driven by a transfer-only signature) so progress does not trigger a conversation refetch.
  - Add a paged or delta `get_conversation(id, since)`, or return only messages after a cursor.
- Effort: M

### TAURI-31 — `export_identity` can destroy the identity it is backing up, and records "backed up" for any destination
- Severity: Medium
- Category: bug / security (data loss)
- Status: VERIFIED. The self-copy truncation was reproduced: a standalone `rustc` test of `std::fs::copy("f.json","f.json")` on this Linux host returned `Ok(0)` and left the file at 0 bytes. `tokio::fs::copy` delegates to `std::fs::copy`.
- Location: `desktop/src-tauri/src/commands/auth.rs:206-236` (dialog at 216-221, `tokio::fs::copy(&source, &dest)` at 225-227, `identity_backed_up_at = now` at 230-233)
- Problem:
  - (a) No check that `dest` is not the source. If the user saves the backup into the app's data folder under the name `identity.json`, the dialog's native overwrite prompt (if any) is answered "Replace". On Linux, `copy` then truncates `identity.json` to zero bytes. That file is the only key to the history, and this returns `Ok`. Hard links and symlinks to the source behave the same way.
  - (b) The copy is neither atomic nor verified. A short write or full disk on the destination still reaches `identity_backed_up_at = now` only if `copy` returned `Ok`, but nothing re-reads the copy to prove it parses or matches.
  - (c) Any destination counts, including the data folder itself (same disk, no protection) or a temp directory. The settings badge then turns green ("Backed up") permanently.
  - (d) `change_password` (`auth.rs:157-178`) does not clear or flag `identity_backed_up_at`. Every older backup still requires the *old* password, yet the UI keeps saying "Backed up".
  - The overwrite confirmation itself is delegated to the native rfd save dialog. Windows, macOS and GTK normally confirm, but the bridge does not enforce it (SUSPECTED per platform).
- Impact: The "make a backup" action can irreversibly destroy the identity. The backup indicator can also be green while no usable off-device backup exists.
- Recommendation:
  - Canonicalize `dest` and its parent. Refuse if `dest == source`, if it is a hard link to the source (compare `(dev, ino)` on Unix and the file index on Windows), or if it lies inside the data directory.
  - Write via `write_file_atomic(dest, &bytes)` after reading the source into memory, then re-read and compare (or `Identity::load(dest)`) before setting `identity_backed_up_at`.
  - Clear `identity_backed_up_at` (or record "backup predates password change") in `change_password`.
- Effort: S

### TAURI-10 — Shutdown flush only covers `CloseRequested`; concurrent saves can land an older snapshot last
- Severity: Low
- Category: bug
- Status: SUSPECTED
- Location: `desktop/src-tauri/src/lib.rs:360-370`; `persist_history` at `:128-145`
- Problem:
  - The flush runs only on `WindowEvent::CloseRequested`. Quitting via the macOS app menu (Cmd+Q, which in Tauri 2 raises `RunEvent::ExitRequested` without per-window `CloseRequested`), OS logoff or shutdown, and `app.exit()` skip it, losing up to `HISTORY_SAVE_MIN_INTERVAL` (3 s) of messages and read marks.
  - Saves are not serialized. A poll-loop save that snapshotted earlier can finish *after* the close handler's save, so the older state reaches disk and the process exits before the next tick heals it.
  - The `Box::leak`ed runtime (`lib.rs:342-348`) is never shut down, so cleanup tasks never run at exit: the UPnP unmap on `upnp_cancel` drop (`client/.../connect.rs:258-260`) and the mDNS goodbye.
- Recommendation:
  - Use `.build(..)?.run(|app, ev| ...)` and flush on `RunEvent::ExitRequested` / `RunEvent::Exit` as well.
  - Guard `persist_history` with a `tokio::sync::Mutex<()>` save lock plus a monotonically increasing generation, so a stale snapshot never overwrites a newer one.
  - Call `stop_hosting()` and unregister mDNS on exit.
- Effort: S

### TAURI-11 — `open_url` validates by string prefix and launches through `rundll32 url.dll,FileProtocolHandler` on Windows
- Severity: Low
- Category: security
- Status: SUSPECTED
- Location: `desktop/src-tauri/src/commands/chats.rs:522-537`
- Problem:
  - The only check is `starts_with("http://"|"https://")`. Whitespace, quotes and control characters are allowed, and the string is never parsed.
  - On Windows, `rundll32` passes its raw command tail (Rust's argv quoting included) to `FileProtocolHandler`, which re-parses it with its own rules. Abuse of this launcher has a history of argument-confusion tricks.
  - Linux and macOS spawn `xdg-open`/`open` with the URL as a single argv entry, which is fine.
- Recommendation:
  - Parse with the `url` crate, require scheme http/https and a host, and reject any input containing whitespace, `"` or control characters.
  - Launch with `open::that_detached(url.as_str())` (already a dependency, and it uses ShellExecuteW on Windows) or `tauri-plugin-opener`.
  - Re-serialize the parsed URL rather than passing the raw input through.
- Effort: S

### TAURI-12 — `core:default` grants the webview more core IPC than it uses
- Severity: Low
- Category: security (least privilege)
- Status: VERIFIED
- Location: `desktop/src-tauri/capabilities/default.json:7-9`; expanded in `gen/schemas/desktop-schema.json`
- Problem: `core:default` includes:
  - `core:tray:default`: `allow-new`, `set-icon`, `set-menu`, `set-tooltip`, …
  - `core:menu:default`: every menu command, including `set-as-app-menu`
  - `core:path:default`: resolve any base directory
  - `core:webview:default` / `core:window:default` getters

  The frontend uses only `invoke` of app commands and `listen` (bridge.js:9-10, 450). Script in the webview could therefore create tray icons or menus that impersonate the app, or enumerate local paths.
- Recommendation: Replace with `["core:event:allow-listen", "core:event:allow-unlisten"]` (plus anything the smoke test proves necessary). App commands do not need capability entries.
- Effort: S

### TAURI-13 — No navigation or new-window guard on the main webview
- Severity: Low
- Category: security
- Status: SUSPECTED
- Location: `desktop/src-tauri/src/lib.rs:352-372` (no `on_navigation` / `on_new_window` / `on_page_load` handling); `desktop/src/components/Messages.jsx:27-28` (peer links rendered as real `<a href>` with only an `onClick` `preventDefault`)
- Problem: React's `onClick` handles only primary-button clicks. Middle-click, Shift/Ctrl-click, or dragging a link onto the window can navigate the main webview, or open a new WebView2 window, to a peer-supplied remote page. That page is a convincing phishing surface ("enter your password") inside the app chrome. IPC to remote origins is denied by the ACL, so this is not a code-execution path.
- Recommendation:
  - Add a `WebviewWindowBuilder` / plugin `on_navigation` that allows only `tauri://localhost`, `http://tauri.localhost` and the dev URL.
  - Deny new-window requests.
  - Render peer links without `href` (a button that calls `openUrl`), or add an `onAuxClick` handler.
- Effort: S

### TAURI-14 — Poll-loop cost: two full O(total messages) passes, four times a second, forever
- Severity: Low
- Category: performance
- Status: VERIFIED
- Location: `desktop/src-tauri/src/lib.rs:598` (250 ms interval), `:624` (`state_signature(&m)` and `ui_signature(&m)`; the latter calls `state_signature` again at `:464`), `:160` (`messages.iter().filter(delivered).count()` per chat), `:489-541` (party signature hashes every file entry and `format!("{:?}")` status each tick)
- Problem:
  - Each tick, under the manager lock, walks every message of every chat twice to count `delivered`, and hashes every title, contact and file.
  - The doc comment calls the signature "cheap"; it is linear in the total history.
  - The 4 Hz wakeup also runs while the app is locked and idle, which is a battery cost on laptops.
- Recommendation:
  - Compute `state_signature` once per tick and pass it into `ui_signature`.
  - Better, keep a `dirty_generation: u64` in `ChatManager`, bumped by every mutating method, and compare that.
  - Back off the interval while the identity is locked or the window is hidden.
- Effort: M

### TAURI-15 — Desktop notifications block under the manager lock and expose message previews
- Severity: Low
- Category: performance / security (privacy)
- Status: VERIFIED (blocking call site); SUSPECTED (Linux markup rendering)
- Location:
  - `client/src/app/chat_manager/mod.rs:588-599` (`notify_rust::Notification::show()`, synchronous)
  - Reached from `poll_session_events` → `receive_text` (`events.rs:591-599`) inside the poll loop's locked section (`lib.rs:610-625`)
- Problem:
  - `show()` performs a synchronous D-Bus call on Linux (default timeout 25 s if the notification daemon hangs) or a WinRT call on Windows, on a tokio worker while holding `ChatManager`'s mutex.
  - The body is the first 50 characters of the peer's message, and the title is the peer-influenced chat title. Both are shown on the lock screen or in the notification centre. There is no "hide content" option.
  - On Linux the body is passed unescaped. Daemons that support body markup will interpret peer-supplied `<b>`, `<a href>` and `<img>`.
- Recommendation:
  - Queue notifications and dispatch them from `spawn_blocking` after the lock is released.
  - Add a "show message content in notifications" setting, off by default for an E2EE messenger.
  - Escape `& < >` in the body on Linux.
- Effort: S

### TAURI-16 — Release builds log at debug level to stdout only; the log plugin is a dead dependency; diagnostics have no logs
- Severity: Low
- Category: dx / security (privacy)
- Status: VERIFIED
- Location: `desktop/src-tauri/src/lib.rs:330-336` (default filter `info,p2pem_classic=debug,messenger_core=debug`); `Cargo.toml:20` (`tauri-plugin-log` never `.plugin(..)`-registered); `commands/auth.rs:281-282`
- Problem:
  - Debug logs (peer addresses, chat ids, filenames, fingerprints) go to stdout. On Linux that stdout is often captured by journald or `~/.xsession-errors` when launched from a desktop entry.
  - On Windows release (`windows_subsystem = "windows"`) stdout goes nowhere, so support bundles admit "no in-app log buffer".
  - `tauri-plugin-log` is compiled in but unused.
- Recommendation:
  - Default to `info` (`warn` for `messenger_core`) in release.
  - Either register `tauri-plugin-log` with a rotating file target in the data directory, with redaction, and include its tail in `export_diagnostics`, or drop the dependency.
- Effort: S

### TAURI-17 — `change_password` can revert a concurrent display-name change
- Severity: Low
- Category: bug
- Status: VERIFIED
- Location: `desktop/src-tauri/src/commands/auth.rs:166` (clone), `:171` (save the clone), `:176` (swap the clone in) vs `:193-197` (`set_display_name` saves the new name)
- Problem: A `set_display_name` that lands during the roughly 1 s Argon2 window is overwritten in both memory and on disk by the stale clone.
- Recommendation:
  - Hold an async "identity write" mutex across both commands.
  - Or, after the blocking work, re-lock, copy only the key-wrapping fields (`encrypted_private_key`, `salt`, `nonce`, `argon_params`) into the live identity, then save.
- Effort: S

### TAURI-18 — The invite link advertises the settings port, not the live listener port
- Severity: Low
- Category: bug
- Status: VERIFIED
- Location: `desktop/src-tauri/src/commands/contacts.rs:76` (`config.listen_port`) vs `connect.rs:153` (`my_addresses` uses `hosting_port.unwrap_or(listen_port)`, with a comment explaining why)
- Problem: A user hosting on a port typed into the Host pane (or port 0, OS-assigned) shares an invite whose LAN candidate points at a port nothing is listening on.
- Recommendation: Use `mgr.hosting_port.unwrap_or(mgr.config.listen_port)`, as `my_addresses` does.
- Effort: S

### TAURI-19 — `open_file` / `open_url` leave zombie processes on Unix and duplicate the `open` crate
- Severity: Low
- Category: bug / maintainability
- Status: VERIFIED
- Location: `desktop/src-tauri/src/commands/chats.rs:315-348`, `:528-536`
- Problem: Each call does `Command::spawn()` and drops the `Child` without waiting. On Linux and macOS every click leaves a zombie `xdg-open`/`open` until the app exits. `open_data_dir` already uses `open::that`.
- Recommendation: Use `open::that_detached` (or reap the child in `spawn_blocking`). Keep the explorer `/select,` special case only for reveal.
- Effort: S

### TAURI-20 — `unlock` reports every failure as "Wrong password"
- Severity: Low
- Category: ux
- Status: VERIFIED
- Location: `desktop/src-tauri/src/commands/auth.rs:73-74`; `core/src/identity/mod.rs:376-401`
- Problem: `decrypt` also fails for a missing salt or nonce, or for out-of-range or unusable stored Argon2 parameters. These are corruption cases where retyping the password can never help, yet they are all mapped to "Wrong password".
- Recommendation: Distinguish the wrong-password error (a typed error, or matching the message) from structural errors, and surface the latter as "identity file is damaged — restore from backup".
- Effort: S

### TAURI-21 — `panic = "abort"`: any command panic is a whole-app crash that skips the history flush
- Severity: Low
- Category: bug / maintainability
- Status: VERIFIED
- Location: `Cargo.toml` `[profile.release] panic = "abort"`; `desktop/src-tauri/src/commands/contacts.rs:111` (`expect("just inserted")`); `lib.rs:345, 413, 422` (`expect` at startup); `lib.rs:74-87`
- Problem:
  - In release, a panic in any command handler or in the poll loop aborts the process, losing up to 3 s of unsaved history.
  - `lock_or_recover`'s poisoning recovery, carefully documented, is dead code in release builds.
  - The `expect` in `import_invite` holds today, but it is an invariant enforced only by `import_contact`'s current dedup logic.
- Recommendation:
  - Replace `expect` in command paths with `ok_or("…")?`.
  - Install a panic hook that attempts a best-effort synchronous `persist_history` before aborting.
  - Document that the poisoning recovery applies only to debug builds.
- Effort: S

### TAURI-22 — `send_file` awaits the bounded file lane while holding the manager mutex
- Severity: Low
- Category: performance
- Status: SUSPECTED
- Location: `desktop/src-tauri/src/commands/chats.rs:239-245`; `client/src/app/chat_manager/files.rs:85-91` (`file_tx.send(FileMeta).await`)
- Problem: The bridge holds `manager.lock().await` across `send_file(..).await`. If the session's bounded file lane is still full (a stalled peer whose socket is not draining chunks from a previous or cancelled send), `send().await` parks while holding the lock. The poll loop and every command then block until the peer's TCP connection dies.
- Recommendation: Use `try_send` for `FileMeta` (the lane is guaranteed empty when the one-send-per-chat guard passes), or reserve the lane permit before taking the lock.
- Effort: S

### TAURI-23 — Data-directory fallbacks and permissions
- Severity: Low
- Category: security / bug
- Status: VERIFIED
- Location: `desktop/src-tauri/src/lib.rs:387-425`
- Problem:
  - `create_dir_all(&data).ok()` swallows the error and creates the directory with default permissions (0755 under a typical umask). The files inside are 0600, but the directory listing, and the presence of `parties.json` and the diagnostics bundles, are world-visible on multi-user systems.
  - When `ProjectDirs` fails, the app uses the CWD-relative `history.json.enc` / `identity.json`, so the identity depends on where the app was launched from.
  - `P2PEM_DATA_DIR` is honoured in release builds.
- Recommendation:
  - Create the data directory with 0700 on Unix.
  - Fail into the error state (`init_error`) instead of a relative path.
  - Consider honouring `P2PEM_DATA_DIR` only in debug builds or behind a CLI flag.
- Effort: S

### TAURI-32 — No restore or import path, although the UI tells users to "restore it over the file"
- Severity: Low
- Category: ux / unfinished
- Status: VERIFIED
- Location:
  - `desktop/src/components/Onboarding.jsx:49-50` ("If you have a backup (Settings › Identity backup on another install), restore it over the file above and reopen the app.")
  - Command list in `desktop/src-tauri/src/lib.rs:252-320` (no `import_identity`/`restore_*`)
  - `commands/auth.rs:295-304` (`open_data_dir` is `ensure_ready`-gated)
- Problem: Restoring is a manual file operation the app cannot help with.
  - On the error screen the path is at least shown, because the identity error text includes it.
  - On a **new device** (the main reason to have a backup), the app opens on `set_password`. Nothing offers "restore from backup". The data folder cannot be opened from the app until a password is set, and setting one creates and saves a *new* `identity.json` that the user must then overwrite by hand while the app is closed.
  - Combined with TAURI-30, a hand-restored identity that does not match the existing `history.json.enc` silently wipes that history on first unlock.
  - The hint "Settings › Identity backup on another install" is also confusing: it reads as if the backup must be taken from another install.
- Recommendation:
  - Add an ungated (pre-auth) `restore_identity` command. It should pick a file with the native dialog, validate it with `Identity::load` plus a password `decrypt` in `spawn_blocking`, refuse if a *different* readable identity already exists unless explicitly confirmed, and install it with `write_file_atomic`.
  - Offer "Restore from backup" on the set-password and error screens.
  - Reword the Onboarding hint.
- Effort: M

### TAURI-33 — The backup covers only the identity; conversations, contacts and trust states have no backup path, and the UI does not say so
- Severity: Low
- Category: ux / docs
- Status: VERIFIED (the export copies only `identity.json`). The reported claim that the UI "promises identity AND history" is **refuted** as stated.
- Location: `desktop/src-tauri/src/commands/auth.rs:201-236` (copies `state.identity_path` only); `desktop/src/App.jsx:268-278` (a code comment plus the `BackupPrompt` wiring, no history promise); `desktop/src/components/Onboarding.jsx:212-221`; `desktop/src/components/Settings.jsx:184-193`
- Problem:
  - None of the screens claims history is included. The copy is consistently "identity" ("Your identity lives only on this device… the key your contacts have verified"). The gap is one of omission.
  - Contacts, their Verified/Blocked trust states, every conversation and all settings live in `history.json.enc` (`persistence.rs:315-328`). None of that is exported, and the UI never says so.
  - A user who "backed up" and then loses the disk keeps their fingerprint but loses every contact and every verification. They must re-verify everyone.
- Recommendation:
  - Either state plainly in both the BackupPrompt and the Settings row that conversations and contacts are not included,
  - or offer a full export: `identity.json` plus `history.json.enc` (both already encrypted at rest) plus `parties.json`, bundled into one file, with the TAURI-31 safeguards.
- Effort: S (copy) / M (full export)

### TAURI-34 — No in-app way to delete the identity or all local data
- Severity: Low
- Category: security (privacy) / unfinished
- Status: VERIFIED
- Location: Command registration `desktop/src-tauri/src/lib.rs:252-320`. The only destructive commands are `delete_chat` (`commands/chats.rs:557-564`), `remove_contact` (`contacts.rs:127-136`) and `party_leave` (`party.rs:633-652`).
- Problem: A user who wants to stop using the app, hand over the device, or start over has to find the data folder (`open_data_dir`) and delete files by hand. That means `identity.json`, `history.json.enc`, `parties.json`, `diagnostics/` bundles, and any leftover `.history.json.enc.<uuid>.tmp` files.
  - Received files in the download folder are untouched by any delete, including `delete_chat`.
  - Doing this while the app runs is unsafe: the poll loop rewrites `history.json.enc` within `HISTORY_SAVE_MIN_INTERVAL`, and the close handler rewrites it on exit.
- Recommendation:
  - Add a `delete_all_local_data` command. It should require the current password (verified off-thread), stop hosting and sessions, set a flag that disables all further saves (including the close flush), delete the data-directory files, and exit or restart into first-run.
  - Optionally offer to delete received downloads too.
- Effort: M

### TAURI-24 — Stale comments and an unused command
- Severity: Info
- Category: docs / maintainability
- Status: VERIFIED
- Location:
  - `commands/contacts.rs:113-114` ("Contacts are not covered by the poll loop's change signature"), which is false since `lib.rs:176-195`
  - `Cargo.toml` ("pulls in egui/ratatui transitively"; egui is gone)
  - Cargo `description` says "Phase 1 bridge"
  - `lib.rs:147-152` ("cheap signature"; see TAURI-14)
  - `my_identity` is registered (`lib.rs:257`) but never invoked by bridge.js
- Recommendation: Update the comments, and remove `my_identity` or use it.
- Effort: S

### TAURI-25 — The invoke-contract test cannot see Tauri's camelCase mapping or nested struct fields
- Severity: Info
- Category: dx
- Status: VERIFIED
- Location: `desktop/src/lib/invokeContract.js:225-305`
- Problem:
  - The comparison is an exact string match. Tauri 2 expects camelCase JS keys for snake_case Rust parameters by default, so a future `chat_id` param with a JS key `chat_id` would *pass* this check and fail at runtime (the exact footgun it exists to catch).
  - Struct parameters (`settings: SettingsUpdate`, `perms: PermsDto`) are checked only at the top level, so a field added to `SettingsUpdate` but not sent by the UI produces "invalid args" at runtime.
  - All current parameters are single-word, so nothing is broken today.
- Recommendation:
  - Fail the check when a Rust parameter contains `_` (enforcing the single-word convention), or model the camelCase conversion.
  - Extend `parseRustCommands` to resolve `#[derive(Deserialize)]` struct fields.
- Effort: S

### TAURI-26 — No updater and no OS code signing
- Severity: Info
- Category: unfinished / security
- Status: VERIFIED
- Location: `desktop/src-tauri/Cargo.toml` (no `tauri-plugin-updater`); `tauri.conf.json` `bundle` (no `createUpdaterArtifacts`, no Windows certificate or macOS signing identity); `.github/workflows/release.yml:400-414` (Sigstore attestations only, by design)
- Problem: Users get no in-app notice of security releases, which matters for a messenger with a peer-reachable parser surface. The installers trigger SmartScreen and Gatekeeper warnings that train users to click through.
- Recommendation: At minimum, add an update *check* (signed release manifest → toast). Consider `tauri-plugin-updater` with its minisign key, and OS code signing when budget allows.
- Effort: M

### TAURI-27 — Command test coverage gaps
- Severity: Info
- Category: maintainability
- Status: VERIFIED
- Location: `desktop/src-tauri/src/tests.rs`
- Problem: The binding and auth gating tests are strong. There is no coverage for:
  - `open_file`'s blocked outcome on a seeded received `.exe` message (testable without launching anything)
  - `file_preview`'s SVG refusal and 4 MiB cap
  - `set_password` / `unlock` on an already-ready identity (TAURI-02)
  - `party_join`'s two-step flow (TAURI-01; needs the in-process server that `client/tests/party_e2e.rs` already stands up)
  - `state_signature` sensitivity (contacts, read marks), and save coalescing
  - `party_send_file`, `party_send_file_dm`, `party_download_file`, `send_file` beyond argument binding
  - `pick_upload`'s size cap
- Recommendation: Add bridge tests for each; most need only seeded `ChatManager` state.
- Effort: M

### TAURI-28 — The community pin is keyed by the raw address string
- Severity: Info
- Category: ux / security
- Status: VERIFIED
- Location: `desktop/src-tauri/src/commands/party.rs:333-336`, `:603-606`
- Problem: `example.org` and `example.org:12345` (the default port), or a hostname and its IP, are different keys. Joining by an alternate spelling finds no pin and falls back to a fresh SAS verification rather than enforcing the stored pin. This fails safe, but only if TAURI-01 is fixed.
- Recommendation: Normalise to the `parse_host_port` result (lower-cased host plus explicit port) before lookup and storage.
- Effort: S

### TAURI-29 — Optional hardening not enabled
- Severity: Info
- Category: best-practice
- Status: VERIFIED
- Location: `desktop/src-tauri/tauri.conf.json:24-26`
- Problem: `app.security.freezePrototype` is not set, and the isolation pattern is not used. Neither is required with the current strict CSP and no `withGlobalTauri`, but both are cheap layers against prototype-pollution-to-IPC chains.
- Recommendation: Set `"freezePrototype": true`. Evaluate the isolation pattern if IPC payloads ever become richer.
- Effort: S

## Things done well
- **File operations never take a filesystem path from the webview.** `open_file` and `file_preview` resolve (chat id, message id) through history (`chats.rs:289-312`). Dialog-driven commands get their paths from native dialogs parented to the window (`native_file_dialog`), so a dialog cannot open behind the app.
- **`ensure_ready` gates every post-auth command**, and `init_error` gates even `set_password`/`unlock` (`ensure_identity_loaded`). Tests assert both barriers across a broad command list.
- **The CSP is strict and has a single source.**
  - It is generated from `csp.js` and drift-tested against `tauri.conf.json`.
  - Production has `script-src 'self'` with no inline or eval.
  - The dev-only relaxations are never shipped, which the built `dist/index.html` confirms.
  - There is no `withGlobalTauri`, no asset protocol, no fs/shell/opener plugin, and devtools are compiled out of release.
- **`open_file` refuses received executables** unless the user confirms. It also checks the Unix exec bit and treats only the final extension as significant. `file_preview` refuses SVG and caps at 4 MiB. Party download names are sanitised before reaching the save dialog.
- **History persistence handles failure correctly.**
  - Writes are coalesced, with a shared `saved_sig` so a command save and a poll-loop save do not duplicate work.
  - Writes are atomic and fsynced, with unique temp names.
  - A failed save leaves the marker unset, so it retries.
- **The identity is handled carefully.**
  - An unreadable identity is never replaced.
  - `change_password` re-wraps a copy off-thread and swaps it in only after the save succeeds.
  - The password floor is published from the core.
- **The TOFU prompt queue is peeked, not taken**, so a second peer's prompt is not lost. `pending_fingerprint` gives the UI a polling fallback.
- **The JS/Rust argument contract is checked twice**: by Rust mock-IPC tests and by a platform-independent text check. All 66 invoke key sets match today.
- **`parties.json` is written atomically**, and a damaged file refuses the join rather than silently dropping every pin.

## Open questions
1. Is `trust: true` on `party_join` ever sent without a preceding `verify` in the same UI flow (for example, rejoin from the saved list after the user left and the pin was deleted)? If so, TAURI-01 also covers rejoins.
2. Is the intended behaviour that connecting to a contact uses the hosting password (TAURI-03), or was the shared field meant to be TUI-only?
3. On macOS, does the app's Quit path raise `CloseRequested` for the main window in the Tauri version pinned here (2.11.x)? This decides whether TAURI-10's flush gap is real on macOS.
4. Is there a reason not to adopt `tauri-plugin-single-instance` (TAURI-05), such as the `P2PEM_DATA_DIR` multi-peer testing workflow? A per-data-dir lock file would keep that working.
5. Finding IDs TAURI-30 to TAURI-34 were added after the coordinator's follow-up and are placed by severity, not by number.
6. Does each platform's rfd save dialog actually confirm overwrites in the shipped configuration (GTK and the xdg portal in particular)? TAURI-31 recommends not relying on it either way.
