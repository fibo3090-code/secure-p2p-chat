# Terminal client (ratatui TUI) and CLI entry point

## Scope covered

Read in full:
- `client/src/main.rs` (CLI args, tracing setup, relay-server mode)
- `client/src/tui/mod.rs`, `client/src/tui/app/mod.rs`, `client/src/tui/app/cmd_connect.rs`, `cmd_contacts.rs`, `cmd_misc.rs`, `cmd_party.rs`
- `client/src/tui/command.rs`, `input.rs`, `overlays.rs`, `ui.rs`, `downloads.rs`
- `client/src/colorgrid.rs`, `client/src/support.rs`, `client/src/logbuf.rs` (logging path), `client/src/lib.rs`
- TUI sections of `docs/USER_GUIDE.md`, `docs/TUTORIAL.md`, `README.md`

Read where the TUI depends on them: `Identity::{get_or_create, save, encrypt, change_password}` (`core/src/identity/mod.rs`), `ChatManager::{connect_to_host*, connect_via_relay, start_host_via_relay, delete_chat, clear_history, reject_fingerprint, add_contact, mark_chat_read}`, `persistence.rs` (`load_history_auto`, `save_history`, `history_snapshot`), `PartyManager::connect_and_join` and `PartyServerConn`, the desktop bridge command list (for parity), and the vendored `ratatui-core 0.1.2` / `ratatui-widgets 0.3.2` / `crossterm 0.29` / `arboard 3.6.1` sources for rendering, input and clipboard behaviour.

No files were modified and nothing was built or run.

## Summary

The TUI is tidy and the recent split into `cmd_*.rs` reads well. Several careful fixes are in place: downloads save off-thread without overwriting, the fingerprint queue is peeked rather than consumed, peer control characters cannot reach the terminal, and usage errors come from one command table. But the audit found **several security-relevant defects in the trust and password flows**, and **one data-loss path on first run**:

1. The TOFU verification overlay opens by itself on `tick()`, and `y` or `Enter` accepts it. A user typing a message ("hey", Enter) accepts an unverified peer by accident.
2. `:party-trust` reconnects with **no pin**, so it trusts whatever key answers the *second* connection, not the one the user checked.
3. `:set-password` on an unlocked identity calls `Identity::encrypt` directly. That bypasses `change_password(current, new)`, whose own doc says skipping the current-password check lets anyone at an unlocked machine lock the owner out.
4. Pressing Esc on the first-run "Set a password" prompt means the identity is **never written to disk** (`Identity::save` refuses plaintext). Every launch becomes a new identity, and the previous history file is overwritten with a new key.
5. There is no bracketed paste, so a pasted multi-line text becomes Enter keystrokes. Each line is sent as a message, and a line starting with `:` is **executed as a command**, for example `:send ~/.ssh/id_rsa` to the selected peer.
6. If `TuiApp::new` fails (an identity file that exists but cannot be read, the case CLAUDE.md treats most carefully), or the app panics, **the terminal stays in raw mode on the alternate screen**. The error message goes only to an in-memory log buffer nobody can see.

Parity with the desktop app is much weaker than the README and tutorial claim ("does everything the desktop app does"). Community channel and DM messages, the audit log, LAN discovery, blocking, community pins, the "auto-host on startup" setting and themes all have no working TUI equivalent. The TUI also runs blocking work on the UI loop: Argon2, synchronous full-history encrypt+fsync every 30 s, reading a whole file into memory for community upload, and a community TCP connect with no timeout.

## Findings — ordered by severity

### TUI-01 — The fingerprint overlay pops up under the user's fingers; `y`/`Enter` accept, `n`/`Esc` reject
- Severity: High
- Category: security
- Status: VERIFIED
- Location: `client/src/tui/app/mod.rs:377-386` (overlay opened from `tick()` with no user action), `client/src/tui/app/mod.rs:720-724` (key handling), `client/src/tui/overlays.rs:300-305`, `:177` (help mentions only y/n)
- Problem: `tick()` sets `self.overlay = FingerprintVerify{..}` whenever a prompt is queued and no overlay is open. Once it is open, every key goes to `handle_overlay_key`: `y` **or `Enter`** calls `resolve_fingerprint(chat_id, true)`, `n` **or `Esc`** rejects (which deletes the conversation), and every other key is swallowed. So someone typing in the composer when a peer connects to their host (an ordinary situation, given auto-rehost) loses their keystrokes. The first `y` in their text ("hey", "yes") or the Enter they press to send **accepts an unverified fingerprint**, which `confirm_fingerprint` then persists as trusted. A stray `n` ("and") or `Esc` rejects a legitimate peer. Help documents only `y / n` and says Esc "closes" overlays.
- Impact: The SAS/TOFU step, the core MITM defence, can be completed by accident with no comparison at all. A MITM who times the connection while the victim is typing gets trusted.
- Recommendation: Do not accept on `Enter`. Require an explicit, deliberate action: a dedicated key such as `A` (capital), or typing the first two SAS digits, or `:verify accept`. Ignore all input for about 1 s after the overlay appears (a debounce). Make `Esc` *defer* (close and keep the prompt queued, re-surfacing later or via a status-bar badge) rather than reject. Never steal focus from the composer silently: show a status-bar alert and open the overlay on a key.
- Effort: S

### TUI-02 — `:party-trust` does not pin the fingerprint the user verified (TOCTOU on community join)
- Severity: High
- Category: security
- Status: VERIFIED
- Location: `client/src/tui/app/mod.rs:21-25` (`PendingPartyJoin` has no fingerprint field), `client/src/tui/app/cmd_party.rs:53-57`, `client/src/tui/app/cmd_party.rs:77-86` (`connect_and_join(..., None, true)`), `client/src/app/party_manager.rs:406-428`
- Problem: `:party-connect` connects, gets `NeedsVerification { fingerprint, sas }`, shows the SAS in a toast and stores only address, username and password. `:party-trust` then opens a **new** TCP+handshake connection and calls `connect_and_join(.., expected_fingerprint: None, trust_new_identity: true)`. In `connect_and_join`, `None` plus `true` falls into `None => {}`, which proceeds and sends the `Join` frame with the username and password to **whatever key answered the second connection**. The fingerprint the user compared out of band is never checked again.
- Impact: An on-path attacker who passes the first connection through (so the honest SAS is shown) and intercepts the second receives the community credentials, and the user believes they verified the server. This defeats exactly what CLAUDE.md's "two-step trust decision" entry exists to prevent.
- Recommendation: Store `fingerprint` in `PendingPartyJoin` and call `connect_and_join(.., Some(&pending.fingerprint), true)` (or `false`, since the pin now decides). Add a TUI test where the second connect presents a different key and must fail before `Join`.
- Effort: S

### TUI-03 — `:set-password` changes the password without the current one (bypasses `change_password`); reports success on partial failure
- Severity: High
- Category: security
- Status: VERIFIED
- Location: `client/src/tui/app/mod.rs:1184-1190` (`self.identity.encrypt(password)` when encrypted and unlocked), `client/src/tui/app/mod.rs:1195-1211`, `core/src/identity/mod.rs` `change_password` doc ("without it anyone who reaches an unlocked machine could rotate the password and lock the owner out")
- Problem: For an encrypted, unlocked identity, `try_set_password` re-wraps the key under the new password with no proof of the old one, then saves. The core API added `change_password(current, new)` for this exact reason, and the desktop bridge uses a `change_password` command. There are failure-path problems too. `encrypt` clears the plaintext key *before* `save` runs, so if `save` fails, memory holds a locked identity whose wrapper does not match the disk. If `Identity::load`/`decrypt` of the reloaded file fails (lines 1198-1207), the code still closes the overlay and toasts **"Password set"** while `self.identity` stays locked.
- Impact: Anyone with brief access to an unlocked terminal can lock the owner out of their identity, and so out of their whole history (`history_key` derives from it). Users can also be told the password was set when it was not.
- Recommendation: Split the command into `:set-password` (only when `!is_encrypted()`) and `:change-password` (prompts for current then new, in masked overlays). Call `Identity::change_password` on a clone, `save` it, and swap it in only after the save succeeds, as the core doc instructs. Toast success only on the full success path.
- Effort: M

### TUI-04 — Dismissing the first-run password prompt means the identity is never saved, so the user gets a new identity every launch and earlier history is overwritten
- Severity: High
- Category: bug
- Status: VERIFIED
- Location: `client/src/tui/app/mod.rs:135-157` (`get_or_create`, `_is_new_identity` ignored, no `save`), `client/src/tui/app/mod.rs:802-809` (Esc closes the Set prompt), `core/src/identity/mod.rs:615-621` (`save` refuses an unencrypted identity), `core/src/identity/mod.rs:651-653` (new identity is in memory only), `client/src/tui/app/mod.rs:431-443` (autosave)
- Problem: On a fresh install `get_or_create` returns an in-memory plaintext identity. Nothing persists it until a password is set. Esc dismisses the Set prompt (a comment in `cmd_backup` says this state is reachable). The identity is unlocked, so `history_key` is set and autosave writes `history.json.enc` under that ephemeral key. On the next launch there is no `identity.json`, so a *different* identity is generated, `load_history_auto` fails with only a `tracing::warn!`, and the first autosave after any chat exists **overwrites the old history file** with a new key. The user's fingerprint also changes on every launch, so every contact sees a key change. `:name` in this state changes the name in memory, then fails to save with "SECURITY ERROR", leaving `identity_name` out of sync.
- Impact: Silent, unrecoverable loss of history, and broken TOFU with everyone. This is the outcome CLAUDE.md's identity rules exist to prevent. The desktop app blocks all UI until a password is set; the TUI does not, which contradicts CLAUDE.md's "The app blocks all UI until password unlock/set-password completes".
- Recommendation: Make the Set overlay non-dismissable (or have Esc quit), as the desktop does. Do not set `history_key` or autosave until the identity has been persisted. Surface a failed `load_history_auto` as an error overlay and **never autosave over a history file that failed to decrypt**.
- Effort: S

### TUI-05 — A multi-line paste is executed as keystrokes: each line is sent, and lines starting with `:` run as commands
- Severity: High
- Category: security
- Status: VERIFIED (code); terminal behaviour per `crossterm-0.29.0/src/event/sys/unix/parse.rs:92` (`\r` maps to Enter)
- Location: `client/src/tui/mod.rs:33-38` (no `EnableBracketedPaste`), `client/src/tui/mod.rs:168-175` (only `Event::Key` handled; `Event::Paste` never produced or handled), `client/src/tui/app/mod.rs:683-692` (`:` on an empty input enters command mode; Enter sends), `client/src/tui/app/mod.rs:725-739` (Enter confirms quit / clear-history)
- Problem: Without bracketed paste, the terminal delivers pasted text as raw keystrokes. Each newline (`\r`) is an `Enter` key. Each pasted line is therefore sent as its own message. After a send the input is empty, so a following line beginning with `:` switches to command mode, and the rest of that line plus the next Enter **executes it**. Pasting text received from someone (for example a peer's "copy this config") containing `\n:send /home/u/.ssh/id_rsa\n` sends that file to the selected chat. `\n:clear-history\n\n` erases everything, because Enter confirms. `\n:connect attacker:1\n` opens a session, and the auto-popping verify overlay plus the next Enter accepts it (TUI-01).
- Impact: Paste-injection of arbitrary commands, including file exfiltration to the current peer. At minimum, a pasted paragraph becomes N separate messages.
- Recommendation: Enable `EnableBracketedPaste` (and disable it on teardown). Handle `Event::Paste(s)` by inserting `s` literally into the focused field, keeping newlines in the composer and stripping them in the command line, and never treat pasted text as commands. Defence in depth: require explicit confirmation for `:send` and `:clear-history`, and do not accept `Enter` as "yes" in destructive confirm overlays.
- Effort: S

### TUI-06 — The terminal is left in raw mode on the alternate screen on startup errors and panics, and the error message is invisible
- Severity: High
- Category: ux
- Status: VERIFIED
- Location: `client/src/tui/mod.rs:33-48` (`TuiApp::new(logs)?` at :42 returns before `disable_raw_mode` / `LeaveAlternateScreen`), `client/src/main.rs:77-83` (tracing has only the `LogBuffer` layer, no stderr/file sink), `client/src/main.rs:114-117`, `client/src/support.rs` `install_panic_hook` (writes a crash file, then calls the default hook; no terminal restore), `client/src/tui/app/cmd_party.rs:97` (`unreachable!()` in the UI path)
- Problem: `get_or_create` deliberately returns a detailed `Err` for an identity file that exists but cannot be read. `run()` propagates it with `?` *after* `enable_raw_mode()` and `EnterAlternateScreen`, so neither is undone. `main` then logs the error with `tracing::error!`, which goes only to the in-memory `LogBuffer` that dies with the process, and calls `exit(1)`. The user's shell comes back in raw mode on the alternate screen (no echo, no line discipline) with **no explanation**. The careful error text in `get_or_create` ("Restore the file from a backup…") is never shown. Any panic in the main task (for example the `unreachable!()` at `cmd_party.rs:97`, or a ratatui layout panic) also leaves the terminal broken. The panic hook prints to the alternate screen, which disappears.
- Impact: The most safety-critical startup failure looks like a hung, corrupted terminal. Users are likely to "fix" it by deleting files.
- Recommendation: Use a guard type, or ratatui 0.30's `ratatui::init()`/`restore()`, which installs a restoring panic hook. Construct `TuiApp` **before** entering raw mode. Restore the terminal in the panic hook before printing. In `main`, `eprintln!` the error chain on failure (stderr is safe once the screen is restored). Replace `unreachable!()` with an error toast.
- Effort: S

### TUI-07 — Community (Party) messages, DMs, the audit log and notices are never displayed; the TUI can post but never read
- Severity: High
- Category: unfinished
- Status: VERIFIED
- Location: `client/src/tui/overlays.rs:543-734` (Party pane renders channels, members, files and quota only), `client/src/tui/app/cmd_party.rs:225` (the only reader of `conn.messages`, used to find a file to download), `client/src/tui/app/cmd_party.rs:304-314` (`:party-audit` requests `conn.audit`, which nothing renders), `client/src/app/party_manager.rs:103-128` (`messages`, `audit`, `last_notice` exist)
- Problem: `rg` over `client/src/tui` finds no rendering of `PartyServerConn.messages`, DM history, `audit` or `last_notice`. A user can `:party-post`, `:party-dm` and `:party-audit`, and will never see a reply, the channel history, or the audit log. `last_error` appears only inside the Party pane, so a refused post or DM is removed silently with no toast.
- Impact: The community feature is effectively write-only in the TUI, while README/TUTORIAL say the TUI "does everything the desktop app does".
- Recommendation: Render the selected channel's or DM's `messages` in the main message view (for example as pseudo-chats in the chat list, or a channel list in the Party pane with Enter to open). Show `audit` in the Party pane when non-empty. Toast `last_error`/`last_notice` when they change, as the desktop does via `party_clear_error`/`party_clear_notice`.
- Effort: L

### TUI-08 — Community connect and trust block the whole UI on a TCP connect with no timeout
- Severity: High
- Category: performance
- Status: VERIFIED
- Location: `client/src/tui/mod.rs:156-158` (`execute_command(cmd).await` inside the frame loop), `client/src/tui/app/cmd_party.rs:27`, `:77-87`, `client/src/app/party_manager.rs:403-404` (`TcpStream::connect(..).await?` then `PartyClient::connect(..).await?`, no `timeout`)
- Problem: Unlike P2P connects (which `spawn_session` and return immediately), `connect_and_join` awaits the TCP connect and the Party handshake inline, and the TUI awaits it inside the render/input loop. A mistyped or firewalled address (SYN black-holed) stalls for the OS connect timeout, typically 75–130 s on Linux and about 21 s on Windows. During that time nothing redraws and no key is processed, including quitting.
- Impact: The UI appears frozen with no progress indication.
- Recommendation: Wrap the connect and handshake in `tokio::time::timeout` (for example 10–15 s) in `PartyManager`, and in the TUI spawn the join as a task that reports back through a oneshot polled in `tick()`, the same way `pending_party_downloads` already works. Show "Connecting to …" in the status bar meanwhile.
- Effort: M

### TUI-09 — Multi-line messages have their newlines removed when rendered, and scroll height is miscomputed
- Severity: Medium
- Category: bug
- Status: VERIFIED
- Location: `client/src/tui/ui.rs:214-232` (each message is one `Line` whose content is a single `Span::raw(content)`), `ratatui-core-0.1.2/src/text/span.rs:311-315` (`styled_graphemes` filters `char::is_control`, which includes `\n`), `client/src/tui/ui.rs:270-275`, `:380-387` (`wrapped_rows` splits on `\n`)
- Problem: The composer explicitly supports newlines (Ctrl+J), and peers send them, but a `Span` is never split on `\n`. ratatui drops control characters inside spans, so "line one\nline two" renders as "line oneline two". `wrapped_rows` counts the newline as an extra row, so `max_scroll` is too large and the bottom of the view shows blank rows under the newest message.
- Impact: Multi-line messages (code, addresses, lists) are mangled on screen, and auto-scroll sits slightly off.
- Recommendation: Build one `Line` per `\n`-separated segment, putting the timestamp and sender prefix on the first line and indenting continuation lines. Keep `wrapped_rows` consistent with that. Add a render test with a multi-line message using `TestBackend`.
- Effort: S

### TUI-10 — CLI launch flags are dropped for a password-protected identity; `--port` is ignored by `--connect`; no flag validation
- Severity: Medium
- Category: bug
- Status: VERIFIED
- Location: `client/src/tui/mod.rs:42-44` (`prompt_auth_if_needed` then `apply_launch_config` runs immediately), `client/src/tui/mod.rs:63-113`, `client/src/tui/app/mod.rs:925-948` (`unlocked_private_key` toasts and returns), `client/src/tui/mod.rs:70` (`:connect {target}` ignores `launch.port`), `client/src/main.rs:20-56`
- Problem:
  - (a) After the first run the identity is encrypted, so it is locked at launch. `apply_launch_config` runs `:host`/`:connect`/`:host-relay`/`:connect-relay` *before* the unlock overlay is answered. Each command hits `require_unlocked`, toasts "Identity is locked", and is discarded, not deferred. So `--host`, `--connect`, `--host-relay` and `--connect-relay` do nothing for any normal user, and the documented examples (`USER_GUIDE.md:103-107`) silently fail.
  - (b) `--connect 1.2.3.4 --port 9000` connects to port 12345: `--port`'s help ("Port to use") implies otherwise.
  - (c) There are no `conflicts_with`/`requires` constraints. `--relay-server --host` silently ignores `--host`; `--relay-token` without a relay flag is ignored; `--host --connect` does both.
  - (d) `--tui` is accepted and does nothing; `-H` is an unusual short form for host.
- Impact: Scripted and automated launches (the README pitch "or by an automated client") do not work once a password exists.
- Recommendation: Queue launch commands and run them after a successful unlock or set-password (store a `Vec<TuiCommand>` and drain it in `try_unlock`/`try_set_password`). Pass `launch.port` as the default port for `--connect`. Add clap `conflicts_with_all` for `relay_server`, and `requires` so `relay_token` needs a relay flag. Hide or deprecate `--tui`.
- Effort: S

### TUI-11 — Passwords are typed in clear on the command line, kept in command history, and truncated at the first space
- Severity: Medium
- Category: security
- Status: VERIFIED
- Location: `client/src/tui/app/mod.rs:584-586` (`push_history(&raw)` before parsing), `client/src/tui/input.rs:185-193`, `client/src/tui/ui.rs:289-295` (command box shows text unmasked), `client/src/tui/command.rs:411`, `:441-445`, `:486-487` (`opt_string`/`word()` take one whitespace-delimited word)
- Problem: `:unlock <pw>`, `:set-password <pw>`, `:connection-password <pw>` and `:party-connect host user <pw>` all show the secret in clear while typed, and the raw line goes into `command_field.history`. Up-arrow recalls the identity password for anyone at the keyboard for the life of the process. Because each password argument is a single `word()`:
  - `:set-password correct horse battery staple` fails the length check, using only "correct";
  - `:set-password correcthorse battery` **sets "correcthorse"** with no warning;
  - `:unlock` with a spaced passphrase always reports "Wrong password".

  The masked overlay path takes the whole text, so the two entry paths disagree. `EditableField`/`String` buffers are not zeroized either (low on its own).
- Impact: Shoulder-surfing and recall of the identity password; silently wrong passwords that lock the user out.
- Recommendation: Make `:unlock`, `:set-password`/`:change-password` and the connection or community passwords open the masked overlay instead of taking an inline argument. If the inline form is kept, take the rest of the line (`a.text()`), and never `push_history` a line whose command is password-bearing. Consider `zeroize` for `password_field`.
- Effort: S

### TUI-12 — Setting a password has no confirmation field and no length hint
- Severity: Medium
- Category: ux
- Status: VERIFIED
- Location: `client/src/tui/overlays.rs:756-800` (single field), `client/src/tui/app/mod.rs:810-816`, `desktop/src/components/Onboarding.jsx:133`, `desktop/src/components/Settings.jsx:45` (desktop requires a match)
- Problem: The password is entered once, masked as `•`. A typo becomes the permanent password for the only key to the history. The prompt does not state the 12-character minimum (`MIN_PASSWORD_LEN`); the user only finds out from a toast afterwards.
- Impact: Lockout from identity and history caused by one typo.
- Recommendation: Use a two-step overlay (new, then confirm, which must match). Show "at least {MIN_PASSWORD_LEN} characters" and a live counter in the prompt.
- Effort: S

### TUI-13 — Blocking work runs on the UI loop: Argon2, full-history save every 30 s, whole-file read for community upload; failures are silent
- Severity: Medium
- Category: performance
- Status: VERIFIED
- Location: `client/src/tui/app/mod.rs:1154-1168` (`decrypt` = one Argon2id pass, 64 MiB, t=3, p=4), `:1170-1214` (`encrypt` plus a reload `decrypt` = two passes), `:431-452` (autosave every 30 s regardless of changes; `save_history` re-serialises, encrypts and fsyncs the whole history synchronously; errors only `tracing::warn!`), `client/src/tui/app/cmd_party.rs:192-210` (`std::fs::read(p)` of an arbitrary path with no size check), `client/src/tui/app/mod.rs:1369-1385` (clipboard)
- Problem:
  - Unlock freezes the UI for about 1 s, and set-password for about 2 s, with no "Unlocking…" frame, because the KDF runs before the next draw.
  - The 30 s autosave rewrites the entire history synchronously even when nothing changed. With large histories (single messages may reach about 24 MiB) this produces periodic stalls and disk churn. The desktop bridge already uses `history_snapshot` plus a background write with change detection (`state_signature`) and `HISTORY_SAVE_MIN_INTERVAL`.
  - `:party-send-file` reads the entire file into RAM on the UI thread before `PartyManager` can refuse it as too large. A 10 GB file means a 10 GB allocation and a long freeze.
  - An autosave failure (disk full, permissions) is never surfaced, so the user believes history is being saved.
- Impact: Periodic UI stalls; possible out-of-memory; silent loss of persistence.
- Recommendation: Run the KDF and the history write via `spawn_blocking` (with `history_snapshot()`), saving only when a state signature changes, as the desktop does. `stat` the file and reject it against `MAX_PARTY_FILE_BYTES` before reading, then read off-thread. Toast persistent save failures, at most once per failure streak.
- Effort: M

### TUI-14 — Each frame does O(all messages) work, and the loop handles one input event per frame
- Severity: Medium
- Category: performance
- Status: VERIFIED (code); magnitude SUSPECTED
- Location: `client/src/tui/mod.rs:149-182` (`tick` + `draw` + `poll(100ms)` + exactly one `event::read()` per iteration), `client/src/tui/ui.rs:205-233` (clones every message's text twice per frame), `:270-274` (re-wraps all of it), `client/src/tui/app/mod.rs:366-395` (`tick` sorts all chats and does other housekeeping every iteration), `client/src/tui/input.rs:91` (`Vec::insert` per char)
- Problem: Every iteration runs a full `tick()` and a full redraw for a single key event. A paste of N characters therefore costs N full ticks and N full redraws, and each redraw clones and wraps the entire selected conversation. A peer can send one ~24 MiB text message (the documented maximum); after that every frame of that chat clones and wraps 24 MiB.
- Impact: Sluggish typing in long chats; pasting large text is very slow; one large incoming message can make the TUI unusable for that chat.
- Recommendation: Drain all pending events per frame (`while event::poll(Duration::ZERO)?`) and redraw once. Cache the rendered lines per chat, invalidated on message count or width change, or render only the visible window of messages. Cap the displayed length of very long messages with a "(truncated — N KB)" marker.
- Effort: M

### TUI-15 — `:disconnect` deletes the conversation and its history; deletions have no confirmation
- Severity: Medium
- Category: ux
- Status: VERIFIED
- Location: `client/src/tui/app/mod.rs:992` and `:1046` (both `Disconnect` and `DeleteChat` call `disconnect_selected`), `client/src/tui/app/mod.rs:1072-1089` (`delete_chat` + save), `client/src/tui/command.rs:150`, `client/src/tui/app/cmd_contacts.rs:51-62` (`:contact-remove`)
- Problem: A user who wants to drop a connection (`:disconnect`, "Disconnect / remove the selected chat") permanently deletes the conversation and saves immediately. There is no command to close a session and keep its history. Neither `:delete` nor `:contact-remove` asks for confirmation. `:contact-remove` also silently revokes the fingerprint's trust and lifts any block; the desktop dialog warns about both via `deleting_contact_would_unblock`. `:clear-history` is the only destructive command with a confirmation.
- Impact: Accidental, irreversible loss of conversation history.
- Recommendation: Make `:disconnect` tear down only the session (ChatManager has the teardown pieces). Add confirm overlays for `:delete` and `:contact-remove`, with the trust/unblock wording from `deleting_contact_would_unblock`.
- Effort: S

### TUI-16 — The Party pane's community selection does not change which community commands act on; posts always go to the first channel
- Severity: Medium
- Category: ux
- Status: VERIFIED
- Location: `client/src/tui/app/mod.rs:785-797` (←/→ changes only `party_sel`), `client/src/tui/app/cmd_party.rs:30-31`, `:89-90` (`current_party` is set only on connect), `client/src/tui/app/cmd_party.rs:110-113`, `:184-187` (`channels.first()`)
- Problem: With two communities joined, selecting one in the pane has no effect on `:party-post`, `:party-dm`, `:party-role` and the rest. They go to whichever community was joined last, while the pane highlights another. `:party-post` and `:party-send-file` always target `channels.first()`; there is no "current channel" concept, despite help saying "current Party channel". Posting into a specific channel or DMing a file is impossible.
- Impact: Messages and admin actions go to the wrong community or channel.
- Recommendation: Make `party_sel` drive `current_party`. Add `:party-channel-select #name` (or a selectable channel list in the pane) and use that channel for post and send-file. Show the target community and channel in the status line.
- Effort: M

### TUI-17 — The TUI never pins community identities, so every join is first contact; docs say the app pins
- Severity: Medium
- Category: security
- Status: VERIFIED
- Location: `client/src/tui/app/cmd_party.rs:20-27` (comment: "the TUI has no saved-communities file"), `docs/USER_GUIDE.md:233-235` ("their app pins the fingerprint on first join and warns them if it ever changes"), `docs/USER_GUIDE.md:242`
- Problem: Without `parties.json`, each `:party-connect` (every launch, every reconnect) is a first contact that asks the user to re-verify by eye. Users quickly learn to type `:party-trust` reflexively, so a changed server key is never flagged as a change. The guide's TUI line also omits `:party-trust`, the mandatory second step.
- Impact: The TOFU guarantee degrades to "trust whatever answered" over time. The documentation overstates the protection.
- Recommendation: Reuse `load_saved_parties`/`write_file_atomic` from the desktop path (it lives in `client/src/app`) to persist pins in the TUI data dir, and pass the pin as `expected_fingerprint`. Until then, correct the guide and document `:party-trust`.
- Effort: M

### TUI-18 — The selected chat is marked read every tick, even while the user is away
- Severity: Medium
- Category: ux
- Status: VERIFIED
- Location: `client/src/tui/app/mod.rs:400-404` (`mark_chat_read(selected)` every tick, unconditionally), `:423-429` (presence already computed from `last_interaction`)
- Problem: Messages that arrive in the selected conversation while the user is idle (beyond `PRESENCE_IDLE`, or the terminal is backgrounded) are marked read immediately and persisted as read (`read_count`). When the user returns, nothing indicates new messages. The desktop marks read only on focus (the `mark_read` command).
- Impact: Missed messages, the exact bug the persisted-unread design was meant to fix.
- Recommendation: Gate `mark_chat_read` on the same "recent interaction" presence used for notifications, and mark read on the next keypress.
- Effort: S

### TUI-19 — Settings that do nothing in the TUI, and features missing compared with the desktop app
- Severity: Medium
- Category: unfinished
- Status: VERIFIED
- Location: `client/src/tui/app/mod.rs:1268`, `:1317-1320` (`auto-host` toggle; only `desktop/src-tauri/src/commands/auth.rs:101` honours `auto_host_on_startup`), `client/src/tui/app/mod.rs:1271-1279`, `:1329-1351` (`theme` is stored; no TUI code reads `cfg.theme`), `:1269`, `:1321` (`mdns`; nothing in `client/src/tui` lists discovered peers), `:1404` (`discovered_peers: 0` hard-coded), the desktop `invoke_handler` command list
- Problem: The TUI settings panel offers `auto-host`, `theme` and `mdns`, but the TUI never auto-hosts on startup, never applies a theme, and never surfaces LAN discovery. CLAUDE.md says discovery is "Surfaced by the TUI and the desktop app"; it is not. Other desktop features with no TUI command: `block_contact`/`unblock_contact`, `change_password` (with current-password check), `list_discovered_peers`, `my_addresses`, `open_file`, `party_leave`, saved communities, `party_send_file_dm`, `party_share_file`, `party_set_file_permissions`, `party_create_channel_kind`, `auto_connect` reconnect, and accepting a *specific* file offer.
- Impact: Users toggle options that silently do nothing. The README/TUTORIAL parity claims ("does everything the desktop app does", "fully featured") are inaccurate.
- Recommendation: Either implement them (auto-host after unlock, `:nearby` for discovery, `:block`/`:unblock`, `:party-leave`) or hide the non-functional settings in the TUI, and correct the docs and CLAUDE.md.
- Effort: M

### TUI-20 — The composer and command box do not scroll, and cursor placement ignores soft wrap
- Severity: Medium
- Category: ux
- Status: VERIFIED
- Location: `client/src/tui/ui.rs:37` (input box fixed at 5 rows, 3 inner), `client/src/tui/ui.rs:315-324` (no `.scroll`), `client/src/tui/mod.rs:136-141` (`cursor_display()` returns the logical row and column; the column is clamped to the box width and the row ignores wrapped rows), `client/src/tui/input.rs:167-180`
- Problem: Once the text exceeds three visual rows (a long line, or several Ctrl+J newlines), the rest is clipped, so the user types blind. On a soft-wrapped line the hardware cursor sticks to the right border of the first row while the text continues below. A pasted invite link in `:import …` (long) shows the same problem in the command box.
- Impact: Composing and checking anything longer than a sentence is error-prone.
- Recommendation: Compute wrapped (row, column) with the same width-aware wrapper, and set `Paragraph::scroll` so the cursor row stays visible. Optionally let the input grow to about 1/3 of the screen height.
- Effort: M

### TUI-21 — Colours are hard-coded: security text is white (invisible on light themes), no `NO_COLOR`, RGB safety grid
- Severity: Medium
- Category: ux
- Status: VERIFIED (code); terminal rendering SUSPECTED
- Location: `client/src/tui/overlays.rs:296` and `:452` (fingerprint hex `fg(Color::White)`), `:809` (confirm text white), `client/src/tui/ui.rs:329` (status bar `bg(Color::Black)`), `client/src/tui/overlays.rs:85` (`BRAND_ACCENT` RGB), `:286`, `:443` (`Color::Rgb` grid)
- Problem: On a light-background terminal the 64-character fingerprint, the backstop for the SAS, and the quit/clear confirmations render white on white. There is no `NO_COLOR` or monochrome mode, and the `theme` setting is unused (TUI-19). On terminals without truecolor (macOS Terminal.app before Tahoe, legacy Windows conhost, `TERM=screen`), the RGB palette is quantised, so two peers on different terminals may see different grid colours, which undermines a cue users are asked to compare.
- Impact: Accessibility, and reliability of the verification UI.
- Recommendation: Use `Color::Reset`/default foreground for body text. Honour `NO_COLOR` (and `COLORTERM`) and fall back to a 16-colour palette, or label cells with palette indices, when truecolor is unavailable. Map the existing `theme` values to a light/dark palette.
- Effort: M

### TUI-22 — Unit tests touch the developer's real data directory; the TUI has no data-dir override
- Severity: Medium
- Category: dx
- Status: VERIFIED
- Location: `client/src/tui/app/mod.rs:133-157` (`ProjectDirs("com","chat-p2p","EncryptedMessenger")` with no override), `client/src/tui/app/mod.rs:1437-1439` (tests call `TuiApp::new`), `:1561` (`app.tick()` triggers autosave), `:1507-1524`, `:1535-1548`, `:1615-1623` (`save_history_now` via `:set`, contact add, quit), `client/src/tui/ui.rs:484`, `:492`, `:501`
- Problem: Every TUI unit test creates the real `~/.local/share/EncryptedMessenger` and reads the real `identity.json`. When none exists (CI, a fresh dev box) the identity is unlocked, so the tests **write `history.json.enc`** there under a throwaway key. Run later, the real TUI generates a new identity, cannot decrypt that file, and overwrites it (see TUI-04). On a developer machine with a real encrypted identity, the tests run "locked", so their behaviour depends on the machine. Unlike the desktop app (`P2PEM_DATA_DIR`), there is no way to run two TUI peers on one machine, which the USER_GUIDE uses as its local test recipe.
- Impact: Tests that are not hermetic and can pollute user data; no multi-instance local testing.
- Recommendation: Add `TuiApp::new_in(data_dir, logs)` and use a `tempdir` in tests. Honour `P2PEM_DATA_DIR` (or a `--data-dir` flag) in the TUI as the desktop does.
- Effort: S

### TUI-23 — `:invite` advertises the configured listen port, not the port actually being hosted
- Severity: Medium
- Category: bug
- Status: VERIFIED
- Location: `client/src/tui/app/cmd_contacts.rs:90` (`self.chat_manager.config.listen_port`), contrast `client/src/tui/app/mod.rs:474-481` (auto-rehost deliberately uses `hosting_port`)
- Problem: After `:host 9000`, `:invite` embeds `<lan-ip>:12345`. The rehost code even comments that rehosting elsewhere "silently invalidates the address they already shared".
- Impact: Invite links that cannot connect.
- Recommendation: Use `self.chat_manager.hosting_port.unwrap_or(config.listen_port)`.
- Effort: S

### TUI-24 — No log output anywhere (relay server is silent; `RUST_LOG` has no visible effect); docs reference a log view that does not exist
- Severity: Medium
- Category: dx
- Status: VERIFIED
- Location: `client/src/main.rs:77-87` (registry with `EnvFilter` + `LogBuffer` only), `client/src/main.rs:94-97` (relay-server mode), `client/src/main.rs:73` ("log overlay (Ctrl+L)"), `client/src/logbuf.rs:3` ("The TUI's log overlay"), `docs/USER_GUIDE.md:478` ("inspect the log terminal")
- Problem: Not writing tracing to stdout in TUI mode is correct, since it would corrupt the screen. But no layer writes anywhere: no file, no stderr. `--relay-server`, a long-running daemon, prints nothing at all (not even "listening", or errors before `?` returns), and its 2000-event `LogBuffer` cannot be read. In the TUI the only access to logs is copying them to the clipboard (Ctrl+L), which fails on headless/SSH machines (see TUI-29), or `:diagnostics`. There is no "log overlay", despite the comments and docs. `tracing::debug!(?args)` also records `relay_token` into the buffer, which then lands in clipboard copies and diagnostics bundles.
- Impact: Operators cannot see what the relay is doing; TUI users on servers cannot read logs.
- Recommendation: Add a `fmt` layer to stderr when not in TUI mode (relay server), and a rolling file layer in the data dir for the TUI. Add a real `:logs` overlay (scrollable view of `LogBuffer`), keeping clipboard copy as a secondary action. Redact `relay_token` from the args debug log.
- Effort: M

### TUI-25 — Ctrl+C does nothing, and Esc has a destructive meaning in one overlay
- Severity: Low
- Category: ux
- Status: VERIFIED
- Location: `client/src/tui/input.rs:240` (`Char(c) if !ctrl`, so Ctrl+C returns false), `client/src/tui/app/mod.rs:664-715` (no Ctrl+C/Ctrl+D/Ctrl+Q handling), `client/src/tui/app/mod.rs:722`
- Problem: Raw mode swallows SIGINT, and no key handler maps Ctrl+C to quit, so the universal escape hatch silently does nothing. In the input focus, `q` types a letter, so the only exits are `Esc`, then `q`, then `y`, or `:q`. In the verify overlay `Esc` rejects the peer (and deletes the chat), whereas the help says Esc "closes overlay".
- Recommendation: Map Ctrl+C (and Ctrl+D on an empty composer) to the quit confirmation, and a second Ctrl+C to quit. Make Esc in the verify overlay defer (see TUI-01).
- Effort: S

### TUI-26 — Message timestamps show UTC hours with no date; all incoming messages are labelled "Peer"
- Severity: Low
- Category: ux
- Status: VERIFIED
- Location: `client/src/tui/ui.rs:208` (`msg.timestamp.format("%H:%M ")` on a `DateTime<Utc>`; `core/src/types.rs:165`), `client/src/tui/ui.rs:209-213`
- Problem: A user in UTC+2 sees their own 14:00 message stamped 12:00. Messages from different days are indistinguishable. The sender is always "Peer", so in a reconnected or renamed chat there is no indication of who is speaking.
- Recommendation: Use `with_timezone(&chrono::Local)`, insert date separators between days, and use the chat title or peer name in place of "Peer".
- Effort: S

### TUI-27 — Auto-rehost can overwrite a command the user just submitted
- Severity: Low
- Category: bug
- Status: VERIFIED
- Location: `client/src/tui/app/mod.rs:584-595` (`submit_command` sets `pending_command`), `client/src/tui/mod.rs:154-158` (next iteration runs `tick()` before `take_pending_command()`), `client/src/tui/app/mod.rs:481` (`self.pending_command = Some(Host(..))` unconditionally)
- Problem: `pending_command` is a single slot. If a rehost becomes due on the tick right after the user presses Enter on a command (or Enter in the Contacts overlay, `mod.rs:1245`), the user's command is replaced and silently lost.
- Recommendation: Use a `VecDeque<TuiCommand>`, or have `auto_rehost` skip when `pending_command.is_some()`.
- Effort: S

### TUI-28 — In-app help and documentation are inaccurate or incomplete
- Severity: Low
- Category: docs
- Status: VERIFIED
- Location:
  - `client/src/tui/overlays.rs:213` ("Esc or :help to close"; `:` is swallowed by the Help overlay at `mod.rs:755-762`)
  - `client/src/tui/app/cmd_misc.rs:78` (tells the user to run `:password`, which does not exist; the command is `:set-password`)
  - `client/src/tui/command.rs:278`, `:508-519` (`:set` help and `settings_keys()` omit `upnp`, although `apply_setting` accepts it and `docs/USER_GUIDE.md:170` tells users to `:set upnp on`)
  - `client/src/tui/overlays.rs:169-179` (no mention of `q`, `?`, or that Enter accepts in verify/confirm)
  - `docs/USER_GUIDE.md:371-418` (the command reference omits `:accept`, `:decline`, `:connection-password`, `:lock`, `:name`, `:backup`, `:party-trust`, `:party`, `:party-send-file`, `:party-download`, `:party-files`, `:party-delete-file`, `:party-role`, `:party-channel-access`, `:party-delete-channel`, `:party-audit`; the verify section does not mention the SAS the overlay leads with)
  - `docs/USER_GUIDE.md:242` (TUI join line omits `:party-trust`)
  - `docs/TUTORIAL.md:53`, `:226`, `README.md:93-97` ("does everything the desktop app does" / "fully featured"; see TUI-07 and TUI-19)
  - CLAUDE.md ("The TUI holds `Arc<Mutex<ChatManager>>` directly"; `TuiApp` owns `ChatManager` by value, `client/src/tui/app/mod.rs:3-4`, `:71`; and the discovery claim)
- Recommendation: Generate the USER_GUIDE command table from `COMMANDS`, or add a test that every `COMMANDS` name appears in the guide. Fix the strings above. Add `upnp` to `settings_keys()`.
- Effort: S

### TUI-29 — Clipboard: contents may vanish on Linux, the Identity overlay overwrites the clipboard without asking, and failures are hidden
- Severity: Low
- Category: ux
- Status: SUSPECTED (Linux content loss, per arboard docs); VERIFIED (silent failure, unconditional overwrite)
- Location: `client/src/tui/app/mod.rs:1369-1373` (`Clipboard::new()` dropped immediately; errors discarded), `:1049-1052` (`:identity` always copies the fingerprint), `client/src/tui/overlays.rs:395`, `:420` (titles claim "copied to clipboard" / "fingerprint copied" regardless), `arboard-3.6.1/src/lib.rs:55-59`
- Problem: arboard documents that on X11/Wayland the content is hosted by the process and "when the last `Clipboard` instance is dropped, the contents may become unavailable", unless a clipboard manager takes over. The TUI drops the instance right after `set_text`. On SSH or headless machines `Clipboard::new()` fails silently, yet the overlay still says the link or fingerprint was copied. Opening `:identity` always replaces the user's clipboard.
- Recommendation: Keep one long-lived `Clipboard` in `TuiApp`, or use `SetExtLinux::wait()` on a background thread. Report failures in the overlay title ("could not copy — select the text manually"). Offer OSC 52 as a fallback for SSH. Copy on an explicit key (`c`) in the Identity overlay rather than on open.
- Effort: S

### TUI-30 — Windows: AltGr characters are probably dropped from all text fields
- Severity: Low
- Category: bug
- Status: SUSPECTED
- Location: `client/src/tui/input.rs:234-240` (`KeyCode::Char(c) if !ctrl`)
- Problem: On Windows, crossterm reports AltGr as `CONTROL | ALT`. Characters typed with AltGr on non-US layouts (`@`, `{`, `[`, `\`, `€`, `|` on German/French keyboards) carry `CONTROL`, so they are ignored. A German-layout user could not type `user@host` or a Windows path.
- Recommendation: Treat `CONTROL|ALT` together as AltGr (insert the character), as many crossterm apps do. Verify on a Windows box.
- Effort: S

### TUI-31 — Duplicate toasts on connect
- Severity: Low
- Category: ux
- Status: VERIFIED
- Location: `client/src/app/chat_manager/connect.rs:359` and `client/src/tui/app/cmd_connect.rs:30-33`; `connect.rs:392` and `cmd_connect.rs:78`
- Problem: ChatManager already toasts "Connecting to h:p" / "Connecting via relay …", and the TUI adds an almost identical toast. Two of the four toast slots (`ui.rs:339`) are consumed.
- Recommendation: Drop the TUI-side toasts.
- Effort: S

### TUI-32 — Leftover patterns from the `cmd_*.rs` split worth factoring
- Severity: Low
- Category: maintainability
- Status: VERIFIED
- Location:
  - `client/src/tui/app/cmd_party.rs:103-109`, `:128-134`, `:163-169`, `:177-183`, `:214-220` (five copies of the "No Party server joined" block, although `require_party()` at `mod.rs:209-220` exists and the newer commands use it)
  - `cmd_party.rs:358-363`, `:398-414`, `:435-440`, `:136-148` (member and channel lookup by name repeated four times)
  - `client/src/tui/app/mod.rs:1246` (`let _ = id;`)
  - `client/src/tui/overlays.rs:382` (`let _ = settings_keys();`)
  - `client/src/tui/ui.rs:425` and `client/src/tui/overlays.rs:748` (two `truncate` functions; the latter's doc says "display columns" but it counts chars, so CJK/emoji names overflow)
  - `client/src/tui/app/mod.rs:1253-1289` + `:1291-1365` + `overlays.rs:361-371` (three hand-synchronised settings tables: index-based toggle with hard-coded `len = 9`, key-based `apply_setting`, and render rows)
  - `client/src/tui/app/cmd_contacts.rs:68-69` (`:contact-rename` mutates `chat_manager.contacts` directly, bypassing ChatManager, unlike every other contact mutation)
  - `client/src/tui/app/mod.rs:18-26` (`PendingPartyJoin` declared between `use` lines)
  - `client/src/tui/app/mod.rs:1142`, `:1145` (`let _ =` on `confirm_fingerprint`/`reject_fingerprint`, then an unconditional "Fingerprint accepted" toast)
  - `client/src/tui/app/mod.rs:300-303` (`parse_command` "compatibility shim")
- Recommendation: Route all Party commands through `require_party()` plus `find_member(&str)` and `find_channel(&str)` helpers. Describe settings in one table (`key`, `kind`, getter/setter) that drives toggle, `:set`, render and `settings_keys()`. Add a `ChatManager::rename_contact`. Surface `confirm_fingerprint` errors. Delete the dead lines.
- Effort: M

### TUI-33 — File-path arguments: no `~` expansion, relative to CWD, backup overwrites silently
- Severity: Low
- Category: ux
- Status: VERIFIED
- Location: `client/src/tui/app/cmd_misc.rs:87` (`std::fs::copy(&source, &dest)`), `cmd_misc.rs:9-22` (`:send`), `cmd_party.rs:192`, `client/src/tui/app/mod.rs:1294-1297` (`download-dir` stored verbatim, not validated)
- Problem: `:backup ~/id.json`, `:send ~/x` and `:set download-dir ~/Downloads` treat `~` literally. Relative paths resolve against wherever the binary was started. `:backup` overwrites an existing file (including a previous backup) without warning, and does not create the parent directory.
- Recommendation: Expand a leading `~`. Resolve and validate `download-dir` (create it, check writability). Refuse to overwrite on `:backup` unless forced (`:backup! path`).
- Effort: S

### TUI-34 — Ambiguous name matching picks an arbitrary item; `#index` refers to an order that is never shown
- Severity: Low
- Category: ux
- Status: VERIFIED
- Location: `client/src/tui/app/cmd_party.rs:224-236` (first substring match over `HashMap` values, so the order is nondeterministic), `cmd_party.rs:322-327`, `cmd_party.rs:137-141` (`#N` into `members`; the Party pane `overlays.rs:644-665` shows no numbers), `client/src/tui/app/mod.rs:970-982` (`resolve_contact` by name with HashMap `find`; duplicate contact names are allowed)
- Problem: `:party-download a` or `:party-delete-file report` can act on a different file than the user meant. `:party-delete-file` may delete the wrong file. `:party-dm #2` targets a member order that is not displayed anywhere.
- Recommendation: Prefer an exact match. On more than one match, list the candidates and refuse. Show indices in the pane.
- Effort: S

### TUI-35 — Transfers overlay cannot accept or decline a specific offer; `:accept` picks one heuristically
- Severity: Low
- Category: ux
- Status: VERIFIED
- Location: `client/src/tui/app/mod.rs:770-783` (only `c` cancel), `client/src/tui/app/cmd_misc.rs:24-50` (sorts offers by "in selected chat" and takes the first)
- Problem: With two held offers, `:accept` accepts whichever sorts first, and the user cannot choose from the overlay where they can see them. `c` (cancel) is also refused for `AwaitingAcceptance` (`mod.rs:1116-1119`), so declining from the overlay is impossible.
- Recommendation: Add `a`/`d` keys in the Transfers overlay acting on the highlighted row, and allow `c` on held offers (map it to `reject_incoming_file`).
- Effort: S

### TUI-36 — Bidi overrides in peer-supplied text are not neutralised in the TUI
- Severity: Low
- Category: security
- Status: SUSPECTED
- Location: `client/src/tui/ui.rs:215`, `:142`, `client/src/tui/overlays.rs:242` (peer message text, chat titles and `peer_name` rendered raw)
- Problem: ratatui strips C0/C1 control characters (`ratatui-core-0.1.2/src/buffer/buffer.rs:351`), so ANSI injection is not possible, which is good. But U+202E and other bidi controls are `Cf`, not `Cc`, and pass through. Terminals that apply bidi (some do) can make a peer name or a message read differently than it is ("Connecting to …" in the verify overlay is peer-controlled). Filenames are already sanitised by core; message text and display names are not, as far as the TUI is concerned.
- Recommendation: Strip or escape `util::has_hidden_characters`-class code points in the TUI render path for names and titles, and visibly mark them in message text.
- Effort: S

### TUI-37 — Diagnostics bundle details
- Severity: Info
- Category: best-practice
- Status: VERIFIED
- Location: `client/src/support.rs` `export_diagnostics_bundle` (`fs::write`, default 0644 permissions, bundle name to the second), `DiagnosticsConfig` (missing `enable_upnp`), `client/src/tui/app/mod.rs:1404` (`discovered_peers: 0`)
- Problem: `logs.txt` contains peer addresses, chat titles and fingerprints and is world-readable on multi-user systems. Two exports within the same second write into the same directory. The report omits UPnP state and always reports zero discovered peers.
- Recommendation: Write with 0600 like the rest of the data dir, add a collision suffix, and include `enable_upnp` and a real peer count.
- Effort: S

## Things done well

- **No terminal injection from peers.** All peer text goes through ratatui `Span`s, and ratatui filters control characters (`ratatui-core` `set_stringn` and `styled_graphemes`), so escape sequences in messages, titles or filenames cannot reach the terminal.
- **Community downloads** (`client/src/tui/downloads.rs`) are a model fix: the name is sanitised, `create_new` closes the check-then-use (TOCTOU) window, partial files are removed on error, the suffix loop is bounded, the write runs off the UI thread, and there are tests.
- **The fingerprint queue is peeked, not taken** (`mod.rs:372-386`), so closing or redrawing never discards a pending verification (the key handling itself is the problem; see TUI-01).
- The **two-step community join** keeps credentials local until the user accepts (`PendingPartyJoin`). Only the missing pin on the second step (TUI-02) breaks the promise.
- `:backup` refuses to export a plaintext identity (`cmd_misc.rs:71-81`).
- **One command table**: usage errors come from `COMMANDS`, so `:help` and parse errors cannot drift (`command.rs:317-359`, tested at `:736-750`). Autocomplete and history interact sensibly.
- **Input handling**: the `EditableField` is UTF-8 safe with a char-index cursor and width-aware cursor column; key-release events are filtered for Windows (`mod.rs:169-175`); mouse capture is deliberately off so native selection works.
- **Logging never goes to stdout** in TUI mode, so it cannot corrupt the screen (the opposite problem is TUI-24).
- **Presence approximated by recent interaction**, falling back to "notify" (`mod.rs:412-429`), is a sensible portable choice.
- The **auto-rehost** logic correctly reuses `hosting_port` and is gated on `check_rehost_needed()` and on unlock.
- **Drift tests**: the brand accent is checked against `design/tokens.json`, and the safety-grid palette is frozen with a known-answer test.

## Open questions

1. Does the core sanitise a peer's identity display name (`peer_name`) before it reaches `PendingFingerprint`? If not, TUI-36 also affects the verify overlay heading.
2. Is the TUI meant to reach full parity (as README/TUTORIAL claim), or should the docs scope it to a "lightweight / server" client? That decides whether TUI-07 and TUI-19 are bugs or documentation fixes.
3. Should `--relay-server` stay in the TUI binary at all, or move to its own binary with a real stderr logger (TUI-24)?
4. The Windows AltGr behaviour (TUI-30) and the Linux clipboard ownership (TUI-29) need confirming on real systems; they are reasoned from crossterm and arboard behaviour, not observed.
