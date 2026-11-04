# User Journey & Machine Footprint (download → daily use → uninstall)

## Scope covered

Read against the code at HEAD `8d626b8` (workspace version 1.16.2):

- Discovery/download: `README.md`, `docs/USER_GUIDE.md`, `SECURITY.md`, `.github/workflows/release.yml` (all 539 lines).
- Install: `desktop/src-tauri/tauri.conf.json`, `desktop/src-tauri/Cargo.toml`, `desktop/src-tauri/capabilities/default.json`, `desktop/src-tauri/src/main.rs`.
- First run / onboarding: `desktop/src-tauri/src/lib.rs` (`run`, `init_bridge`, poll loop, `persist_history`), `desktop/src-tauri/src/commands/auth.rs` (whole file), `desktop/src/App.jsx` (auth gating), `desktop/src/components/Onboarding.jsx`, `desktop/src/components/Settings.jsx` (identity backup rows), `core/src/identity/mod.rs` (`get_or_create`, `save`), TUI startup in `client/src/tui/app/mod.rs` (`new`, `prompt_auth_if_needed`, `handle_password_key`, `try_unlock`, `try_set_password`, `autosave`).
- Daily use: hosting/UPnP (`client/src/app/chat_manager/connect.rs`, `core/src/network/nat.rs`, `core/src/network/session.rs::bind_host_listener`), invites (`core/src/identity/invite.rs`, `commands/contacts.rs::my_invite_link`), notifications (`chat_manager/mod.rs::notify_incoming_message/show_notification`, `events.rs::receive_text`), receipts/typing, communities (`commands/party.rs` saved-parties file, `desktop/src/components/Parties.jsx`, `server/src/main.rs`, `server/src/identity.rs`).
- Failure paths: history load/save (`client/src/app/persistence.rs`), identity load errors, crash/panic handling (`client/src/support.rs`), file spool (`core/src/transfer/receiver.rs`).
- Footprint: every `ProjectDirs`, `temp_dir`, download dir, `File::create`, `localStorage`, log and crash path; network listeners and outbound hosts (`rg` for `reqwest|ureq|hyper|stun|8.8.8.8|http(s)://`).
- Deletion/uninstall: `delete_chat`, `ChatManager::delete_all_data`/`clear_history`, docs' reset/uninstall claims, Tauri bundle config.
- Privacy disclosures: README, USER_GUIDE, THREAT_MODEL vs. what the UI says.

Not run: no build, no test, no installer produced; Tauri/NSIS/tauri-action behaviour is reasoned from configuration and known upstream defaults and is marked SUSPECTED where it depends on them.

## Summary

The happy path is thoughtfully built. It has a password floor enforced in core, a refusal to replace an unreadable identity, a backup prompt at first run, privacy-conservative defaults (mDNS, UPnP, auto-accept and TOFU auto-trust all off), no telemetry and no outbound HTTP, and a release pipeline that only publishes attested assets. The weak points are at the edges of the lifecycle, where data is lost or left behind:

1. **Silent history destruction.** If `history.json.enc` cannot be decrypted or parsed after a successful unlock, the desktop app only logs a warning, starts empty, and its poll loop overwrites the file within ~250 ms (Critical, VERIFIED). Corruption, a downgrade, or one unknown enum variant is enough. On the TUI, pressing Esc on the first-run "set password" prompt runs the session on an identity that is never saved. The next launch generates a new identity, and once a chat exists the old history is overwritten with the new key (High, VERIFIED).
2. **"Delete everything" and uninstall cleanup are documented but do not exist.** `USER_GUIDE.md` describes `Settings → Delete Everything` and a Windows uninstaller that wipes data. Neither exists. `ChatManager::delete_all_data` is dead code, and the data directory sits outside the paths Tauri's uninstaller knows about. After uninstalling, the identity, history, `parties.json`, diagnostics and webview profile stay on every OS.
3. **Backup and migration are incomplete.** The backup copies `identity.json` only, never the history. There is no restore or import command, although the error screen tells users to "restore it over the file above". The "Backed up" badge is set by any export to any path and is not cleared after a password change.
4. **Privacy claims do not match communities.** The README promises "End-to-end encryption on every conversation" and "no server copy of your messages". The Communities UI never says the operator can read every channel message and DM, which THREAT_MODEL.md states plainly.
5. **Release mechanics (SUSPECTED).** `tauri-action` is given `tagName` while the job token is read-only. Depending on tauri-action's behaviour, this either fails every desktop leg or bypasses the "draft-only, attest-then-publish" design. Both macOS legs may also emit the same `P2PEM.app.tar.gz`, which the collision check refuses.
6. **Plaintext leaves the encrypted store.** Smaller spills of plaintext outside the encrypted store:
   - Message text goes into OS notifications.
   - Unaccepted file offers are spooled into `~/Downloads` and never swept after a crash.
   - Info-level logs with peer IPs and filenames go to stdout, which a Linux session may persist in the journal.
   - There is no single-instance guard.
   - A UPnP mapping survives app exit until its lease expires.
   - The desktop release build has no logs or crash reports to give support.

## The journey, step by step

### 1. Discovery & download
- **User:** reads README → "Download P2PEM Desktop" → GitHub Releases latest; picks `P2PEM_<v>_x64-setup.exe` / `_aarch64.dmg` / `_x64.dmg` / `_amd64.AppImage|.deb`.
- **Code:** `release.yml` drafts a release (`create-release`), builds 4 desktop legs (`windows-x64`, `macos-aarch64`, `macos-x64`, `linux-x64` on ubuntu-22.04) + 3 Tools legs (linux-x64, macos (arm64 runner only), windows-x64). It uploads them to the run's artifact store, then the `attest` job:
  - flattens them, refusing name collisions, and requires ≥7 files;
  - writes `SHA256SUMS`;
  - attests everything with Sigstore;
  - uploads the assets to the draft, re-downloads and verifies them, then publishes.
  - Bundle targets: `deb, rpm, appimage, msi, nsis, dmg` (`tauri.conf.json:31`).
  - Actions are pinned to commit SHAs, builds are cold with no cache, and `--locked` is used.
- **Friction:**
  - Installers are unsigned and un-notarized (deliberate; `release.yml:399-419`, `SECURITY.md:25`). The README says nothing about the SmartScreen / Gatekeeper steps a user must take (JOURNEY-14).
  - There is no auto-updater and no in-app version check (JOURNEY-15).
  - The README platform table omits the `.rpm` that is built.
  - Tools for Intel macOS are not built.
  - The tauri-action/token interaction may break the pipeline (JOURNEY-05).

### 2. Install
- **Code:** Tauri 2 defaults; there is no `bundle.windows`/`nsis`/`macOS`/`linux` section. Identifier `com.chat-p2p.p2pem`, product `P2PEM`. Capabilities are `core:default` only; there is no shell, fs, dialog or notification plugin. Plugins: none registered (`tauri-plugin-log` is a dependency but never `.plugin(...)`-ed; `lib.rs:347-372`). There is no deep-link plugin, so `chat-p2p://` is **not** registered (JOURNEY-11), and no autostart, single-instance or updater plugin.
- **Friction:**
  - Windows: per Tauri's NSIS defaults, a per-user install. WebView2 is bootstrapped from Microsoft if missing, which is an outbound download nobody mentions.
  - Hosting binds `0.0.0.0:<port>` (`core/src/network/session.rs:93-97`), so the first "Host" triggers the Windows Defender / macOS firewall prompt. Nothing in the app explains it (JOURNEY-19).

### 3. First run / onboarding
- **Desktop:**
  - `init_bridge` resolves `ProjectDirs("com","chat-p2p","P2PEM").data_dir()` (or `P2PEM_DATA_DIR`) and `create_dir_all`s it.
  - `Identity::get_or_create` makes an in-memory plaintext identity when none exists; a present-but-unreadable file is a hard error (`core/src/identity/mod.rs:648-679`), surfaced as `auth_status.state == "error"` → `BootScreen fatal`. Good.
  - `SetPasswordScreen` explains that there is no recovery. `set_password` → `Identity::encrypt` enforces `MIN_PASSWORD_LEN`, then `save` (refuses plaintext; `mod.rs:615-621`) → `load_history_auto` → `BackupPrompt`.
- **TUI:**
  - `TuiApp::new` (`client/src/tui/app/mod.rs:130-157`) uses `ProjectDirs("…","EncryptedMessenger")`.
  - With a fresh identity, it **immediately sets the history key from the unsaved plaintext identity** (`:150-156`) and shows the Set-password overlay, which **Esc dismisses** (`:804-809`).
  - The app is then fully usable on an identity that is never written (JOURNEY-02).
- **Friction:**
  - The backup covers the identity only.
  - There is no restore path.
  - The badge semantics are weak (JOURNEY-06).
  - The desktop has no "Delete everything" for a user who wants to start over (JOURNEY-03).

### 4. Daily use
- **Connecting:** direct (host/connect), relay with hole punch (the user must supply a relay; there is no default relay, only `relay.example.com` placeholders), LAN mDNS (off by default), UPnP (off by default). SAS verification is both-sided.
- **Multiple devices:** not supported or documented. The same `identity.json` on two machines gives two peers with the same fingerprint and divergent histories, with no sync.
- **Moving to a new computer:** undocumented. It needs both `identity.json` *and* `history.json.enc` copied by hand into an OS-specific directory the UI only reveals through "open data folder" (`auth.rs:295-305`), and there is no import (JOURNEY-06).
- **Changing password:** `change_password` re-wraps the key on a copy and saves atomically (good). Existing backups stay on the **old** password, and the UI does not say so (JOURNEY-06).
- **Notifications:**
  - Notifications use `notify_rust` directly from core and include the **message preview** (`events.rs:591,599`; `mod.rs:574-598`) (JOURNEY-07).
  - Delivery receipts are always sent (`mod.rs:634`). There is no opt-out; typing indicators can be turned off.
- **Communities:**
  - Two-step server trust is well done.
  - The server stores plaintext (THREAT_MODEL.md:91,103), but the UI and README do not say so (JOURNEY-04).

### 5. Failure paths
- **Peer offline:** no offline delivery (README says so honestly). The composer is disabled offline and sends return `Err`.
- **Corrupted / unsupported history:** silently replaced by an empty one (JOURNEY-01).
- **Corrupted `parties.json`:** joins are refused with an explanation (good, `party.rs:549-575`).
- **Disk full:** the history save logs a warning and retries next tick (`lib.rs:140-144`). There is no user-visible toast, so the user does not know history is not being saved. Low; folded into JOURNEY-10.
- **Crash:**
  - The desktop has no panic hook or log file (JOURNEY-10).
  - Orphan spool files remain in Downloads (JOURNEY-12).
  - The UPnP mapping lingers until its lease expires (JOURNEY-13).
  - Unsaved state is at most ~3 s old (`HISTORY_SAVE_MIN_INTERVAL`), but the macOS Quit path may skip the close flush (JOURNEY-16).

### 6. Footprint inventory (what is written where)

| Writer | Location | Content | Encrypted? | Removed by uninstall? |
|---|---|---|---|---|
| Desktop | `ProjectDirs(com,chat-p2p,P2PEM).data_dir()`: Linux `~/.local/share/p2pem/`, macOS `~/Library/Application Support/com.chat-p2p.P2PEM/`, Windows `%APPDATA%\chat-p2p\P2PEM\data\` (`lib.rs:387-392`) | `identity.json` (Argon2+ChaCha20 wrapped key, plaintext name/fingerprint/public key) | key: yes | No (JOURNEY-03) |
| Desktop | same dir | `history.json.enc` (chats, contacts, **config**, contact map) | yes (ChaCha20-Poly1305, key from private key) | No |
| Desktop | same dir | `parties.json`: community addresses, usernames, display names, pinned fingerprints (`party.rs:531-538`) | **no** (0600) | No |
| Desktop | same dir `/diagnostics/bundle-<stamp>/` | `diagnostics.json` (paths, identity name, fp prefix, counts, config incl. relay server and download dir) + `logs.txt` placeholder | no | No |
| Desktop fallback | **CWD** `history.json.enc`/`identity.json` when no data dir resolves (`lib.rs:420-426`) | as above | — | No |
| Webview | Tauri/WebView profile keyed by identifier `com.chat-p2p.p2pem` (Windows `%LOCALAPPDATA%\com.chat-p2p.p2pem\EBWebView`, Linux `~/.local/share/com.chat-p2p.p2pem` + `~/.cache/…`, macOS `~/Library/WebKit/…`, `~/Library/Caches/…`) | `localStorage`: `p2pem.theme`, `p2pem.party.read` (server ids, channel/DM-member ids, counts; `desktop/src/lib/partyUnread.js:5-24`) | no | Windows NSIS: only if the user ticks the delete-data box (SUSPECTED); others: no |
| TUI | `ProjectDirs(…,EncryptedMessenger)` (`client/src/tui/app/mod.rs:133`) | identity, history | as desktop | n/a (portable binary) |
| TUI | `<EncryptedMessenger data>/diagnostics/crashes/panic-*.log` (`client/src/support.rs:144-158`) | panic payload + backtrace | no | — |
| Both | user download dir (`default_download_dir`, `core/src/types.rs:445-453`) | received files **and** `tmp_<uuid>_<name>` spools for *unaccepted* offers (`receiver.rs:284-287`) | no | No |
| Both | `Config::temp_dir` = `$TMP/chat-p2p` | **unused** by any non-test code (only displayed in diagnostics) | — | — |
| Both | OS notification store (Windows Action Center DB, macOS NC DB, freedesktop daemon history) | sender title + message preview | no | No |
| Desktop | stdout (`lib.rs:328-335`, default `info,p2pem_classic=debug,messenger_core=debug`) | peer IPs, filenames, contact names (e.g. `session.rs:138`, `events.rs:605`, `contacts.rs:36`) | no | journal on Linux desktop sessions (SUSPECTED) |
| Server | `--data-dir` default **relative** `party-data` (`server/src/main.rs:43-46`) | `party.db` (plaintext channel+DM history), `blobs/`, server private key PEM (0600, atomic) | no (by design) | n/a |
| Router | UPnP/NAT-PMP mapping `P2PEM host` TCP `<port>`, 1 h lease (`nat.rs:17,105-113`) | port forward | — | not removed on exit (JOURNEY-13) |

No registry keys or autostart entries are created by app code; installer-created entries are Tauri defaults only.

**Network:**
- Listeners:
  - the host listener on `0.0.0.0:<listen_port>` (default 12345), only when hosting or with `auto_host_on_startup`;
  - mDNS only with `enable_mdns`;
  - the server on `0.0.0.0:<port>`.
- Outbound:
  - peers, relays and community servers the user names;
  - the UPnP SSDP multicast and NAT-PMP gateway when `enable_upnp` is on;
  - `primary_local_ipv4` "connects" a UDP socket to `8.8.8.8:80`, which sends no packet (`core/src/util.rs:536-545`);
  - `open_url` to the OS browser.
- No telemetry, update check, STUN or IP-lookup service (no HTTP client in the tree). VERIFIED.

### 7. Deletion & uninstall
- **Delete conversation:** removes it from memory and saves immediately (`chats.rs:558-564`). The atomic rewrite leaves no plaintext. Received files stay in Downloads, and the dialog does not say so (JOURNEY-17).
- **Delete contact:** revokes trust (documented, verified elsewhere).
- **Delete all data / reset identity:** **absent** from the desktop and TUI (JOURNEY-03).
- **Uninstall:** data stays on every OS, and no doc lists the paths to delete.

### 8. Privacy disclosures
- **Peers learn:** your IP (direct; or the relay's IP plus your IP in a hole punch), online presence while connected, typing (opt-out), delivery receipts (no opt-out), and display name.
- **Invites embed:** LAN IP, external IP when UPnP is on, name and timestamp (`contacts.rs:72-91`, `invite.rs:40-55`). The UI does not mention the embedded IPs.
- **Relays learn:** both endpoints' IPs and timing.
- **Community servers learn everything,** in plaintext (JOURNEY-04).

## Findings — ordered by severity

### JOURNEY-01 — An unreadable history file is silently replaced with an empty one right after unlock
- Severity: Critical
- Category: bug
- Status: VERIFIED
- Location: `desktop/src-tauri/src/commands/auth.rs:79-83` (unlock), `:135-139` (set_password); `desktop/src-tauri/src/lib.rs:128-145` (`persist_history`), `:602, 626-628, 728-735` (`save_history_if_changed`); `client/src/app/persistence.rs:131-160, 205-215`
- Problem: the failure path runs like this:
  - `unlock` calls `load_history_auto`, and on `Err` only does `tracing::warn!` and returns `Ok(())`.
  - `history_key` is already set, so the manager is unlocked with empty state.
  - The poll loop starts with `saved_sig = None` and `last_save` pre-aged, so on the first 250 ms tick `already_saved` is false and `persist_history` writes the empty snapshot over `history.json.enc` with `write_file_atomic`.
  - `load_encrypted` fails on any AEAD failure (a bit flip, a truncated sync), on any `serde_json` error (for example a `MessageContent`/`ChatKind` variant written by a newer build), and on `history_version_supported` returning false (`"1.0"|"1.1"` only).
- Impact:
  - A user who downgrades, restores an old app with a newer history, or suffers one corrupted byte permanently loses every conversation, contact, trust state and setting (config lives in the history file).
  - It happens silently and within a quarter of a second. Nothing on screen says it happened.
  - The identity path has exactly the opposite, careful behaviour (`get_or_create` refuses to replace an unreadable `identity.json`), so the asymmetry is clearly unintended.
- Recommendation:
  - Make a history load failure (other than `NotFound`) a hard state, `auth_status.state = "history_error"`. Do not set `history_key` on the manager, so `history_snapshot` fails and nothing is saved.
  - Before any rewrite, rename the unreadable file aside (`history.json.enc.unreadable-<ts>`).
  - Offer "Start with empty history (the old file is kept at …)" as an explicit choice.
  - Add a bridge test that writes garbage / a future version to `history.json.enc`, unlocks, ticks the poll loop, and asserts the bytes are unchanged.
  - Apply the same treatment to the TUI (`client/src/tui/app/mod.rs:153-155, 1159-1162`), where `autosave` overwrites as soon as the first chat exists (`:431-442`).
- Effort: M

### JOURNEY-02 — TUI first run: Esc on "set password" runs the whole session on an identity that is never saved
- Severity: High
- Category: bug
- Status: VERIFIED
- Location: `client/src/tui/app/mod.rs:135-156` (new identity, history key set from the unsaved plaintext key), `:288-297` (`prompt_auth_if_needed`), `:804-809` (Esc dismisses the Set overlay), `:309-311` (`is_unlocked` true for a plaintext key); `core/src/identity/mod.rs:651-653, 615-621`
- Problem:
  - On first launch `get_or_create` returns a fresh plaintext identity that is not written to disk (`save` refuses unencrypted identities).
  - `TuiApp::new` immediately derives the history key from it, and the user may Esc past the password overlay.
  - The user can then host, connect, verify contacts and chat. `autosave` writes `history.json.enc` under this ephemeral key.
  - On the next launch there is still no `identity.json`, so a **new** identity with a new fingerprint and key is generated.
  - The history fails to decrypt (warn only), and the first new chat triggers `autosave`, which overwrites it.
- Impact:
  - Every launch is a different peer, so every contact sees a changed fingerprint, which looks exactly like a MITM.
  - All previous history is destroyed.
  - Nothing tells the user.
- Recommendation:
  - Make the Set-password overlay non-dismissable while `!identity.is_encrypted()`. Esc should quit or explain, not continue.
  - Alternatively, do not set `history_key` and refuse all session commands until the identity is persisted, mirroring the desktop's `ensure_ready`.
  - Add a regression test: new identity → Esc → `is_unlocked()` must be false or commands must be refused.
- Effort: S

### JOURNEY-03 — No way to delete local data; documented "Delete Everything" and uninstaller cleanup do not exist; uninstall leaves identity and history behind
- Severity: High
- Category: privacy / docs / unfinished
- Status: VERIFIED (code and docs); SUSPECTED for the exact NSIS template paths
- Location:
  - `docs/USER_GUIDE.md:445-448`
  - `client/src/app/chat_manager/mod.rs:798-831` (`delete_all_data`, only referenced from `tests.rs:1906-1939`)
  - the desktop command list in `desktop/src-tauri/src/lib.rs:250-319` (no delete/reset command)
  - `desktop/src-tauri/tauri.conf.json:29-39` (no `bundle.windows.nsis` config)
  - `desktop/src-tauri/src/lib.rs:387-392` (`ProjectDirs("com","chat-p2p","P2PEM")`)
- Problem:
  - The guide says "`Settings -> Delete Everything` removes the messenger app data directory…" and "On Windows, the uninstaller also offers the same full-data cleanup". Neither exists in `Settings.jsx`, the bridge or the TUI.
  - The only implementation, `delete_all_data`, is dead code.
  - Tauri's stock NSIS uninstaller "delete application data" option targets directories named after the bundle identifier (`%APPDATA%\com.chat-p2p.p2pem`, `%LOCALAPPDATA%\com.chat-p2p.p2pem`). The real data lives at `%APPDATA%\chat-p2p\P2PEM\data`, so even a user who ticks it keeps their identity and history.
  - The MSI, DMG, deb, rpm and AppImage remove nothing.
  - No doc lists the per-OS paths.
- Impact:
  - Someone giving away or decommissioning a machine has no in-app way, and no documented way, to remove their keystore, encrypted history and plaintext `parties.json`, the webview profile, or the Downloads files.
  - The docs promise a privacy property the product does not have.
- Recommendation:
  - Add a `delete_all_data` command. It should be guarded by password re-entry plus a typed confirmation, stop sessions and the UPnP mapping, and call `ChatManager::delete_all_data` (extended to also remove `parties.json` and `diagnostics/`) then exit. Expose it in Settings, and in the TUI as `:wipe`.
  - Either configure NSIS (`bundle.windows.nsis.installerHooks` / a custom uninstall section that asks before removing `%APPDATA%\chat-p2p\P2PEM`), or move the data dir under the bundle identifier.
  - Add a "Where P2PEM keeps data / how to remove it" table for all three OSes, including the webview profile directories, to USER_GUIDE. Until the feature exists, fix lines 445-448.
- Effort: M

### JOURNEY-04 — Users are told everything is end-to-end encrypted with no server copy, but community operators read all community traffic in plaintext
- Severity: High
- Category: privacy / docs
- Status: VERIFIED
- Location:
  - `README.md` ("End-to-end encryption on every conversation"; "Nothing stored anywhere but your device… no server copy of your messages"; "Communities: a self-hosted server…")
  - `desktop/src/components/Parties.jsx:119-123` (join screen: "administered, multi-channel rooms that keep history")
  - vs. `THREAT_MODEL.md:91,97,103`, `docs/USER_GUIDE.md:249`
- Problem:
  - THREAT_MODEL states the Administered tier stores **plaintext** channel messages, DMs and files on the operator's disk. It also says the operator "wears an explicit 'this operator can read messages' badge".
  - No such badge or disclosure exists in the desktop UI. The join screen and verify card talk about codes and history, never about the operator reading content.
  - The README's headline claims contradict it outright.
- Impact:
  - Users will treat Community DMs as private between members when the server owner, or anyone who compromises that server or its backups, can read them.
  - It is the most consequential privacy misunderstanding the product can create.
- Recommendation:
  - Add a persistent "Operator can read messages" badge on every community header and DM composer.
  - Add one sentence on the join and verify card: "Messages and files you post here are stored readable by the server operator."
  - Reword the README bullets to "every direct conversation" and add Communities to "Honest limitations".
- Effort: S

### JOURNEY-05 — Release: `tauri-action` is given `tagName` with a read-only token (pipeline break or attestation bypass)
- Severity: High
- Category: packaging
- Status: SUSPECTED (depends on tauri-action v0.6.2 behaviour)
- Location: `.github/workflows/release.yml:17-18` (workflow `contents: read`), `:154-157` (desktop job has no `permissions:` override), `:247-257` (`tagName`, `releaseName`, `GITHUB_TOKEN`)
- Problem:
  - When `tagName` is set, tauri-action looks up or creates the GitHub release and uploads the bundles to it.
  - The header comment (`:12-16`, `:46-51`) says build legs "no longer upload anything to the release… they hold no write token at all".
  - With a read-only token, the lookup cannot see the draft (drafts are invisible to non-writers via `getReleaseByTag`) and the create/upload calls 403, which fails all four desktop legs. If the token did have write, the legs would upload un-attested bytes directly, the exact thing the redesign forbids.
  - The `d4af540` redesign has not been exercised by any tag in this clone (`git tag` is empty).
- Impact: the next release either fails to produce desktop installers or publishes them outside the attest-then-publish guarantee.
- Recommendation:
  - Drop `tagName`/`releaseName` (and the `GITHUB_TOKEN` env) from the tauri-action step so it only builds, or replace it with `npx tauri build --locked ${{ matrix.args }}` and the existing staging step.
  - Do a dry run with a `v0.0.0-rc` tag on a fork.
- Effort: S

### JOURNEY-06 — Backup covers the identity only; no restore/import; the "Backed up" badge is weak and goes stale after a password change
- Severity: Medium
- Category: ux / unfinished
- Status: VERIFIED
- Location:
  - `desktop/src-tauri/src/commands/auth.rs:192-228` (`export_identity` copies `identity_path` only, then sets `identity_backed_up_at` unconditionally)
  - `:153-176` (`change_password` does not reset `identity_backed_up_at`)
  - `desktop/src/components/Onboarding.jsx:47-51` ("restore it over the file above") and `:217-226`
  - `desktop/src/components/Settings.jsx:181-205`
  - command list `lib.rs:250-319` (no import/restore)
- Problem:
  - The backup excludes `history.json.enc`, `parties.json` (community pins) and settings, so restoring it yields an identity with an empty app: no conversations, no contacts, no trust states, no community pins.
  - There is no import or restore command. The fatal screen tells users to copy the file "over the file above", and the only way to find the directory is the error text itself or "open data folder" (which needs a ready identity).
  - `identity_backed_up_at` is stamped after any successful copy to any destination, including the data dir itself or a temp folder, and is never invalidated.
  - After `change_password`, every existing backup still needs the **old** password, but the badge keeps saying "Backed up".
  - There is no documented "move to a new computer" procedure, and multi-device use is not addressed (the same key on two machines diverges silently).
- Impact: users believe they are protected against disk loss and are not, for everything except the key. They may also find their backup unusable because they no longer remember the old password.
- Recommendation:
  - Offer "Export full backup": an encrypted archive of identity + history + `parties.json`. The history is already encrypted under the identity-derived key, so the archive needs no new crypto.
  - Add "Restore from backup" on the first-run and fatal screens. Validate by decrypting with the entered password before replacing anything, and move the current files aside.
  - Clear `identity_backed_up_at` in `change_password`, and say "Your previous backups still use your old password".
  - Document migration and state that one identity per device is the supported model.
- Effort: M

### JOURNEY-07 — Desktop notifications put message plaintext into OS notification stores and on the lock screen
- Severity: Medium
- Category: privacy
- Status: VERIFIED (content); SUSPECTED (Windows app attribution)
- Location: `client/src/app/chat_manager/events.rs:591,599`; `client/src/app/chat_manager/mod.rs:574-598`; `core/src/types.rs:467` (`enable_notifications: true` by default)
- Problem:
  - `notify_incoming_message(chat_id, &preview)` sends the conversation title plus a text preview to `notify_rust`.
  - The OS persists these: Windows `wpndatabase.db`, macOS Notification Center DB, freedesktop daemons. Previews also show on lock screens.
  - There is no "show sender only / hide content" option. This is the main place message plaintext escapes the encrypted history.
  - `notify_rust` is called without an app id. On Windows that typically attributes the toast to PowerShell, and on macOS to a default bundle, rather than P2PEM.
- Impact: a privacy-focused messenger leaks message content to OS-level stores outside its encryption, persisting after the chat is deleted.
- Recommendation:
  - Add a notification-content setting (Full / Sender only / Generic, defaulting to Sender only).
  - Set `.appname("P2PEM")` and the Windows `app_id`, or move notifications to `tauri-plugin-notification` in the desktop shell.
  - Document it in the privacy section.
- Effort: S

### JOURNEY-08 — No single-instance guard: two desktop instances share one identity and race on the history file
- Severity: Medium
- Category: bug
- Status: VERIFIED (absence); impact reasoned
- Location: `desktop/src-tauri/Cargo.toml` (no `tauri-plugin-single-instance`); `lib.rs:347-372`
- Problem: double-launching (very common from a taskbar or dock) starts a second process with the same data dir. Both unlock the same identity and both rewrite `history.json.enc` from their own in-memory state every ≥3 s, so the last writer wins. Both compete for the host port, and `parties.json` upserts race too.
- Impact: messages received in one instance are erased by the other's save. The two copies are the same peer, so connecting them is a self-connection. Errors surface as confusing port-in-use toasts.
- Recommendation: register `tauri-plugin-single-instance` to focus the existing window. In the TUI, take an advisory lock file in the data dir.
- Effort: S

### JOURNEY-09 — Release: both macOS legs may stage an identically named `P2PEM.app.tar.gz`, which the collision check rejects
- Severity: Medium
- Category: packaging
- Status: SUSPECTED
- Location: `.github/workflows/release.yml:266-301` (a directory is archived as `$(basename).tar.gz`), `:446-471` (refuse any basename collision)
- Problem: if tauri-action lists the `.app` bundle in `artifactPaths` for both `aarch64` and `x64` (the comment at `:263-265`, `:285-288` anticipates it), both produce `staged/P2PEM.app.tar.gz`. `attest` then fails with "two build legs produced a file named P2PEM.app.tar.gz", so nothing is published.
- Impact: the macOS-inclusive release is blocked.
- Recommendation: name directory archives with the leg label (`P2PEM_${VERSION}_${label}.app.tar.gz`), or skip `.app` directories (the DMG already contains them).
- Effort: S

### JOURNEY-10 — The desktop release build has no logs or crash reports, and the diagnostics bundle is almost empty
- Severity: Medium
- Category: ux / best-practice
- Status: VERIFIED
- Location: `desktop/src-tauri/src/main.rs:2` (`windows_subsystem = "windows"`); `lib.rs:326-335` (stdout-only `tracing_subscriber`); `client/src/support.rs:128-160` (`install_panic_hook`, called only from `client/src/main.rs:72`); `auth.rs:279-282` (bundle says logs go to stdout); `docs/USER_GUIDE.md` ("diagnostics and crash logs under the app data area")
- Problem:
  - On Windows release builds stdout goes nowhere.
  - There is no panic hook in the desktop app.
  - `tauri-plugin-log` is a dependency but never registered.
  - A user hit by any failure (load warnings like JOURNEY-01, a failed save with the disk full at `lib.rs:142`, connection errors) has nothing to send.
  - Conversely, on Linux GNOME/KDE sessions the app's stdout is commonly captured by the systemd journal at the default `debug` level. Peer IPs (`session.rs:138`), filenames (`events.rs:605`, `files.rs:73`) and contact names (`contacts.rs:36`) then persist outside the app's control.
- Impact: support is blind, and a quiet metadata log spill happens on Linux.
- Recommendation:
  - Install the panic hook in `run()` with a P2PEM-aware path (`default_diagnostics_dir` is hard-coded to `EncryptedMessenger`).
  - Keep an in-memory `LogBuffer` (as the TUI does) for the diagnostics bundle.
  - Default the release filter to `warn`, and redact or omit filenames and addresses below debug.
  - Surface history-save failures as a persistent toast.
- Effort: M

### JOURNEY-11 — `chat-p2p://` invite links are not registered with the OS, and the IPs they embed are not disclosed
- Severity: Medium
- Category: ux / privacy
- Status: VERIFIED
- Location: `core/src/identity/invite.rs:23-25, 40-55`; `desktop/src-tauri/src/commands/contacts.rs:72-91`; `tauri.conf.json` (no `plugins.deep-link`); `Cargo.toml` (no `tauri-plugin-deep-link`); `desktop/src/components/Creator.jsx:329` (paste-only import)
- Problem:
  - Links look clickable (`chat-p2p://invite/v2/...`), but clicking one in a browser, mail client or chat app produces "no application can open this link". Users must copy and paste into Creator.
  - `my_invite_link` embeds the LAN IP from `primary_local_ipv4()` and, when UPnP is on, the router's public IP, alongside the name and timestamp. Invites get pasted into email and social apps, so this hands a home IP to anyone who sees the link. The UI does not mention it.
- Impact: friction in the core onboarding loop, and an undisclosed IP leak.
- Recommendation:
  - Register the scheme via `tauri-plugin-deep-link` (desktop `schemes: ["chat-p2p"]`) and route it to the import dialog with the SAS still required.
  - Add a line under "Copy invite": "This link contains your current IP address(es)", plus an option to mint a link without addresses (relay-only).
- Effort: M

### JOURNEY-12 — Unaccepted file offers are spooled into the user's Downloads folder and orphaned after a crash
- Severity: Medium
- Category: privacy / bug
- Status: VERIFIED
- Location: `core/src/transfer/receiver.rs:284-287` (spool `tmp_<uuid>_<name>` in `dest_dir`); `client/src/app/chat_manager/events.rs:628` (`dest_dir = config.download_dir`); `core/src/types.rs:457-459, 465` (`temp_dir` defined but unused outside tests and diagnostics); no startup sweep (`rg tmp_` finds no cleanup in non-test code)
- Problem:
  - Before the user accepts a file, up to `MAX_UNACCEPTED_SPOOL_BYTES` (64 MiB) per conversation of peer-chosen bytes is written in plaintext into `~/Downloads`. Indexers, cloud-sync clients (OneDrive/Dropbox "Downloads" sync) and antivirus can all pick it up.
  - The spool is deleted on decline, cancel or disconnect, but a crash or kill leaves `tmp_<uuid>_<name>` files forever.
  - Nothing sweeps them at startup.
- Impact: content the user never agreed to receive lands, and may persist, in a synced and indexed user folder. The `Downloads` clutter looks like a bug.
- Recommendation:
  - Spool into a hidden app-owned subdirectory on the same filesystem (e.g. `<download_dir>/.p2pem-incoming/`, keeping rename atomicity), created 0700.
  - Sweep it at startup.
  - Delete the dead `Config::temp_dir`, or use it consistently.
- Effort: S

### JOURNEY-13 — The UPnP / NAT-PMP mapping is not removed when the app exits
- Severity: Low
- Category: privacy / best-practice
- Status: VERIFIED (no exit path); SUSPECTED for permanent-lease routers
- Location: `client/src/app/chat_manager/connect.rs:210-262` (unmap only when `upnp_cancel` fires while the runtime lives); `desktop/src-tauri/src/lib.rs:345-346` (`Box::leak(rt)`), `:360-369` (close handler only flushes history); `core/src/network/nat.rs:17, 105-113`
- Problem:
  - Quitting the app ends the process before the spawned unmap task runs, so the router keeps forwarding `<port>` to the machine for up to the 1 h lease.
  - Some IGDv1 routers only accept permanent leases (`OnlyPermanentLeasesSupported`), and the code does not handle that. On such routers the mapping either fails or, if the stack falls back to 0, becomes permanent.
- Impact: an inbound port stays forwarded after the user has stopped hosting. Any later process bound to that port is exposed.
- Recommendation:
  - On `CloseRequested` / `RunEvent::Exit`, call `stop_hosting()` and synchronously await `nat::unmap_port` (bounded at ~2 s).
  - Handle the permanent-lease error explicitly: refuse, and tell the user.
- Effort: S

### JOURNEY-14 — Unsigned installers ship with no install walkthrough
- Severity: Medium
- Category: packaging / ux
- Status: VERIFIED
- Location: `release.yml:399-407`; `SECURITY.md:25-29`; `README.md` (Install section); `docs/USER_GUIDE.md:524`
- Problem:
  - Not signing is a documented, reasoned choice. However, macOS 15+ removed the right-click "Open" bypass for un-notarized apps, so users must go to System Settings › Privacy & Security › "Open Anyway". Quarantined unsigned apps can also show "is damaged".
  - Windows SmartScreen needs "More info › Run anyway".
  - None of this is in README or USER_GUIDE. USER_GUIDE only says "Gatekeeper… prompts may still apply". The verification instructions also lack a Windows `certutil -hashfile` equivalent.
- Impact: the first contact with the product looks like malware, and users may abandon it or disable protections more broadly than needed.
- Recommendation:
  - Add per-OS "First install" steps with screenshots, and a Windows `Get-FileHash` / `certutil` line in the release notes.
  - Longer term, consider a free notarization path (an Apple Developer account is a cost) or Windows signing via a free OSS programme (e.g. SignPath for OSS).
- Effort: S

### JOURNEY-15 — No update mechanism or update notification
- Severity: Low
- Category: unfinished / security
- Status: VERIFIED
- Location: `desktop/src-tauri/Cargo.toml` (no `tauri-plugin-updater`); no HTTP client anywhere; `SECURITY.md:20-21` ("Older releases: No — upgrade to the latest release")
- Problem: the support policy requires users to be on the latest release, but nothing tells them a new one exists. A security fix (e.g. GHSA-6q3g-734c-22jm-class issues) reaches only users who check GitHub on their own.
- Impact: the installed base stays on vulnerable versions indefinitely.
- Recommendation:
  - Add an opt-in "check for updates" that fetches the GitHub latest-release JSON on user request, with a disclosure of the host contacted.
  - Or use `tauri-plugin-updater` with signed update manifests generated in `attest`.
  - Either way, document the privacy implication.
- Effort: M

### JOURNEY-16 — The history flush on close may be skipped on macOS Quit and on session logout
- Severity: Low
- Category: bug
- Status: SUSPECTED
- Location: `desktop/src-tauri/src/lib.rs:360-369`
- Problem: the synchronous flush runs only on `WindowEvent::CloseRequested`. Cmd+Q / the app-menu Quit on macOS, and SIGTERM at logout or shutdown, end the app via `RunEvent::ExitRequested`/`Exit` without a window close request.
- Impact: up to ~3 s (`HISTORY_SAVE_MIN_INTERVAL`) of messages, receipts or read marks can be lost on quit.
- Recommendation: move the flush, plus the UPnP unmap and session teardown, into `.run(|app, event| match event { RunEvent::ExitRequested{..} | RunEvent::Exit => … })`.
- Effort: S

### JOURNEY-17 — Deleting a conversation does not mention received files, which stay on disk
- Severity: Low
- Category: ux / privacy
- Status: VERIFIED
- Location: `desktop/src/components/ChatDialogs.jsx:38`; `desktop/src-tauri/src/commands/chats.rs:558-564`
- Problem: the dialog says "Delete … and its local history? This can't be undone." Files received in that conversation remain in the download folder. Their notification copies (JOURNEY-07) also remain.
- Recommendation: add "Files you received remain in <download dir>" with an optional "also delete received files" checkbox. Received paths are already tracked in `MessageContent::File`.
- Effort: S

### JOURNEY-18 — Metadata stored in plaintext outside the encrypted history is not documented
- Severity: Low
- Category: privacy / docs
- Status: VERIFIED
- Location: `desktop/src-tauri/src/commands/party.rs:531-538, 580-589` (`parties.json`: address, username, name, fingerprint); `desktop/src/lib/partyUnread.js:5-24` (`localStorage` server, channel and member ids); `identity.json` (plaintext name and fingerprint); webview profile directories (see the table above)
- Problem: the README's "Identity and history are encrypted at rest" is true, but which communities you belong to, your username there, and their addresses are readable by anyone with file access. The webview profile is in a separate, undocumented location.
- Recommendation: document these in USER_GUIDE "What is stored locally", or move `parties.json` into the encrypted history and keep only the pins needed pre-unlock (none are needed: joins require unlock).
- Effort: S/M

### JOURNEY-19 — The first host triggers an OS firewall prompt the app never explains; the listener is IPv4-only on every interface
- Severity: Low
- Category: ux
- Status: VERIFIED
- Location: `core/src/network/session.rs:93-97` (`bind(("0.0.0.0", port))`); no "firewall" string in `desktop/src`
- Problem:
  - Windows Defender and the macOS application firewall prompt when the app first binds. The app gives no warning or explanation, and denying it (or choosing "Private networks only" on a public Wi-Fi profile) silently breaks inbound connections.
  - IPv6-only or dual-stack peers cannot reach a host that binds only `0.0.0.0`.
- Recommendation:
  - Before the first host, show "Your OS may ask to allow P2PEM to accept connections — allow it on networks where you want to be reachable".
  - Add a troubleshooting entry.
  - Consider binding `[::]` with dual-stack.
- Effort: S

### JOURNEY-20 — Server and tools defaults: relative data dir, password visible in process list, no Intel macOS tools
- Severity: Low
- Category: best-practice / packaging
- Status: VERIFIED
- Location: `server/src/main.rs:39-46` (`--password` CLI arg; `--data-dir` default `party-data` relative to the CWD); `release.yml:325-331` (tools only on `macos-latest`, which is arm64)
- Problem:
  - Launching the server from different directories silently creates separate communities with new server keys. Every member then hits a changed-pin refusal.
  - `--password` shows in `ps` and in shell history (`PARTY_PASSWORD` env is better and supported, but not recommended in the help text).
  - Intel Mac users get no tools binary, although the release notes imply every OS has one.
- Recommendation:
  - Default `--data-dir` to `ProjectDirs` (or require it) and log its absolute path.
  - Prefer `PARTY_PASSWORD` / `--password-file` in the docs.
  - Add `macos-13` (x64) or a universal tools build.
- Effort: S

### JOURNEY-21 — Relative-path fallback when no data directory resolves
- Severity: Low
- Category: bug
- Status: VERIFIED
- Location: `desktop/src-tauri/src/lib.rs:420-426`; `client/src/tui/app/mod.rs:141-144`; `client/src/support.rs:90`
- Problem: with no resolvable home directory (some sandboxed or service contexts), the identity, history and diagnostics are written relative to the CWD. That can be `C:\Windows\System32`, `/`, or the app bundle.
- Recommendation: fail closed with a clear error instead of writing to the CWD.
- Effort: S

### JOURNEY-22 — Delivery receipts cannot be turned off; typing indicators are on by default
- Severity: Info
- Category: privacy
- Status: VERIFIED
- Location: `client/src/app/chat_manager/mod.rs:634-636` (`send_ack_for_chat`, unconditional); `core/src/types.rs:468` (`enable_typing_indicators: true`)
- Problem:
  - Every received message sends an `Ack`, which tells the peer you were online and the app was running at that moment. There is no setting.
  - Typing is opt-out.
  - Neither is described as metadata disclosure.
- Recommendation: add a "Send delivery receipts" toggle, and list both in a privacy section of USER_GUIDE.
- Effort: S

### JOURNEY-23 — Stale or unused packaging bits
- Severity: Info
- Category: best-practice
- Status: VERIFIED
- Location:
  - `desktop/src-tauri/Cargo.toml` (`tauri-plugin-log = "2"` is never registered; a comment says the bridge "pulls in egui/ratatui transitively", while CLAUDE.md says egui is gone; the package description says "Phase 1 bridge")
  - `README.md` platform table omits the `.rpm` that `tauri.conf.json:31` builds
  - `USER_GUIDE.md:113` mentions `messenger-server` build output, while the release ships `p2pem-server`
- Recommendation: remove the unused dependency, refresh the comments, and align the docs with the artifacts.
- Effort: S

## Things done well

- **Identity is never silently replaced.** `get_or_create` errors on an unreadable file. The bridge exposes `state: "error"` and refuses `set_password`/`unlock`/`ensure_ready`. `BootScreen fatal` explains the situation without offering a destructive retry (`core/src/identity/mod.rs:648-679`, `auth.rs:18-62`, `Onboarding.jsx:34-55`).
- **Atomic, 0600, fsynced writes** for `identity.json`, history, `parties.json` and the server key (`write_file_atomic`). `Identity::save` refuses to write an unencrypted key.
- **The password floor is enforced in core**, published to the UI, and paired with honest "no recovery" copy and a first-run backup prompt with the action attached.
- **`change_password` re-wraps a copy** and swaps it in only after the file is safely written.
- **Privacy-conservative defaults:** mDNS, UPnP, auto-accept, TOFU auto-trust, auto-host and auto-connect are all off (`core/src/types.rs:461-481`, with a test).
- **No telemetry, no update pings, no third-party lookups.** There is no HTTP client in the tree. `primary_local_ipv4` sends no packet.
- **The UPnP mapping is leased (1 h)** and renewed, and the renewal is cancellable, so a crash cannot leave a permanent forward on a well-behaved router.
- **Legacy plaintext history migration** deletes, or at minimum truncates, the plaintext file and toasts the user if it cannot (`persistence.rs:240-295`).
- **Release pipeline:**
  - SHA-pinned actions and cold `--locked` builds;
  - a draft-until-verified release;
  - attested `SHA256SUMS`;
  - a tag/version match check;
  - honest documentation of the unsigned-installer trade-off.
- **Received-file safety:** filename sanitising, `create_new` path reservation, the acceptance gate with a spool cap, and the executable-open confirmation.
- **Community trust:** no credential leaves before the server's fingerprint is pinned or verified, and a damaged `parties.json` blocks joins rather than dropping the pins.

## Open questions

1. Does tauri-action v0.6.2 skip release upload when the token lacks `contents: write`, or fail? (JOURNEY-05). A dry-run tag on a fork would settle it.
2. Does tauri-action list `P2PEM.app` in `artifactPaths` for macOS without the updater enabled? (JOURNEY-09)
3. Exact paths removed by the Tauri 2 NSIS "delete app data" checkbox for this identifier, and whether it is shown at all with the default config (JOURNEY-03).
4. Is multi-device use (the same identity on a laptop and a desktop) intended to be supported? If not, the docs should say "one identity per device" and the backup or restore flow should warn about running both.
5. Should the history format carry a "minimum reader version" so an older build can refuse to open, and refuse to save, a newer file (the downgrade case of JOURNEY-01)?
6. Coordinator note (1): I could not find copy in `App.jsx`/`Onboarding.jsx` that explicitly promises "identity + history" in the backup. `BackupPrompt` speaks only of the identity, and `LockScreen` says "unlock your identity and history". The functional gap the note implies is real either way (JOURNEY-06): the export excludes history and there is no restore.
