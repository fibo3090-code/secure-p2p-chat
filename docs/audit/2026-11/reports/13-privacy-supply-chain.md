# Privacy, metadata and supply-chain audit

Auditor 13 of 13. Repository state audited: `main` at `62b952f` (the PR #188 merge), which includes `49a4e6e chore(deps): clear today's RustSec advisories` and `a41ae3a` (`.gitleaksignore`). Nothing in the repository was modified. Every finding is tied to file:line, crate@version or an advisory ID, and is marked VERIFIED (I read the code or ran the tool) or SUSPECTED (inferred, and needs a runtime or upstream check).

## Scope covered

- **Privacy and metadata.** I covered these areas:
  - What each observer can learn, summarised in the matrix below.
  - Invite-link contents and the mDNS advertisement.
  - The relay and hole-punch control protocol.
  - What the community server stores, and what the UI and docs say about it.
  - Typing indicators and delivery receipts.
  - Log statements and their default levels in all three binaries.
  - `client/src/logbuf.rs`, `client/src/support.rs` (diagnostics bundle and panic hook), and the desktop `export_diagnostics`.
  - OS notifications and clipboard writes.
  - Plaintext on disk on the client and on the server.
  - Zeroize coverage of keys and passwords.
- **Supply chain.** I covered these areas:
  - `Cargo.lock`: 749 packages, all from crates.io, no git sources.
  - Duplicate major versions.
  - `deny.toml`, including every accepted advisory. I ran `cargo-deny check` (`~/.cargo/bin/cargo-deny`) at `62b952f`: advisories, bans, licenses and sources all pass.
  - The npm tree, via `npm audit` with and without `--omit=dev` and a check for install scripts.
  - GitHub Actions pinning and third-party actions.
  - Release integrity in `release.yml`.
  - Dependabot coverage.
  - gitleaks configuration, `.gitleaksignore`, and a tree and history grep for keys and tokens.
  - The tracked `desktop/dist/` bundle as an injection vector.
- **Process gap the coordinator raised.** RUSTSEC-2026-0385/-0386/-0370/-0371/-0372 reached PR CI before the scheduled audit dealt with them. See SUPPLY-01.
- **Already reported by other auditors, cited and extended here rather than re-derived:**
  - Community messages and DMs are plaintext on the server, while README.md:8-9/34/41 says otherwise.
  - `party.db` is world-readable.
  - The desktop logs at `messenger_core=debug`.
  - Notification text goes into OS stores.
  - Invites embed LAN IPs.
  - mDNS advertises the display name and hostname.

## Summary

The project's supply-chain controls are well above average:
- Every action is pinned to a commit SHA.
- The release pipeline drafts the release and publishes it only after attesting and re-verifying.
- `SHA256SUMS` is itself attested.
- Release builds are cold and `--locked`.
- `cargo-deny` runs with a stale-acceptance checker.
- gitleaks scans full history.
- CodeQL covers Rust, JavaScript and Actions.
- The committed `dist/` is checked against a fresh rebuild.

**The privacy findings are larger than the supply-chain ones.** The highest-impact items are these:

1. **Community servers.** The operator receives plaintext channel messages and DMs. The operator also receives the user's long-term P2P identity fingerprint, because the same RSA identity key authenticates to every community. That lets every community operator link a user across communities and to their P2P contacts (PRIV-02). The desktop UI never tells the user any of this, and THREAT_MODEL.md claims a badge that does not exist (PRIV-01).
2. **Logs and the TUI diagnostics bundle.**
   - The TUI keeps the CLI arguments, including `--relay-token`, in its log buffer.
   - It also keeps peer IPs, file names and save paths, the UPnP external IP and contact names.
   - "Export diagnostics" writes all of that next to a README that says only "Private keys are not included" (PRIV-04).
   - On Linux, the desktop app's debug stdout plausibly ends up in the systemd journal (PRIV-05).
3. **Secrets in memory.** Zeroize coverage stops at the identity PEM and the AEAD keys. The `history_key` (a `Copy` `[u8; 32]`), every password and the decrypted history JSON are never wiped, and nothing is protected from swap (PRIV-08).

On the supply-chain side, these are the most important items:

1. **Advisories on PRs (SUPPLY-01).** cargo-deny's advisory check runs as a hard gate on PRs, and only on PRs that touch Rust. A new RustSec advisory therefore breaks unrelated Rust PRs while frontend-only PRs merge green. The daily audit files an issue but never prepares a fix.
2. **tauri-action may publish outside attestation (SUPPLY-02, SUSPECTED).** `release.yml` passes `tagName`/`releaseName` to tauri-action. That makes the action try to create or upload to a release, which contradicts the attest-then-publish design. Either the desktop legs fail under `contents: read`, or (with broader permissions) assets go up without attestation.
3. **Dependabot gaps (SUPPLY-03).** There is no `github-actions` ecosystem, so 14 SHA-pinned actions never get update PRs. The `core/fuzz` lockfile is git-ignored, so the fuzz job floats.
4. **Unverified gitleaks in CI (SUPPLY-04).** CI's secret-scan job still pipes the gitleaks download straight into `sudo tar` without a checksum. audit.yml fixed this; ci.yml did not.

## Metadata exposure matrix

| Observer | What it learns | Evidence |
|---|---|---|
| **Peer (P2P contact)** | <ul><li>Your IP address. A direct connection reveals it, and so does the hole punch: the relay hands each side the other's public endpoint *and* LAN candidates **before** any SAS/TOFU decision, to anyone who holds the token.</li><li>Your LAN IP, and with UPnP on your external IP, embedded in every invite.</li><li>Your display name, the invite creation timestamp and your long-term fingerprint.</li><li>Typing indicators: **on by default**.</li><li>Delivery receipts: **always sent, with no setting**. `receive_text` calls `send_ack_for_chat` unconditionally.</li><li>Online presence: auto-host and auto-reconnect.</li><li>Protocol version only, not the app version.</li></ul> | `desktop/src-tauri/src/commands/contacts.rs:72-91`; `core/src/identity/mod.rs:537-552`; `core/src/network/relay.rs:405-409, 696-704`; `core/src/types.rs:468` (`enable_typing_indicators: true`); `client/src/app/chat_manager/events.rs:598`, `mod.rs:634` |
| **Relay operator** | <ul><li>Both peers' public IP:port.</li><li>Each side's self-reported LAN `ip:port` (`PunchCaps.local_addrs`).</li><li>The rendezvous token, in plaintext: the control channel is unencrypted bincode over `send_packet`.</li><li>Pairing time and session duration.</li><li>If bridged: byte volume and timing for the whole session.</li><li>Tokens embedded in v3 invites are static per contact, so they link repeated sessions between the same pair.</li><li>Content is not visible: the v3 handshake is end-to-end.</li></ul> | `core/src/network/relay.rs:1-19, 65-108, 182` (logs each client IP at info) |
| **Community server operator** | <ul><li>All channel messages **and DMs** in plaintext (`messages.envelope` and `dm_messages.envelope` TEXT columns).</li><li>File contents, names, MIME types and uploader.</li><li>Membership and a role audit log.</li><li>Every member's **P2P identity fingerprint**: the user authenticates with the same RSA key that identifies them to P2P contacts.</li><li>The member's IP, logged at info.</li></ul> | `server/src/state/db.rs:360-411`; `desktop/src-tauri/src/commands/party.rs:323-324` (`id.private_key()` passed to `connect_and_join`); `client/src/app/party_manager.rs:395-404`; `server/src/lib.rs:67` |
| **LAN observer** | <ul><li>With mDNS on (off by default): the display name as instance name, the OS **hostname**, the LAN IP and the listening port. The fingerprint is no longer in TXT, but the `types.rs` comment still says it is.</li><li>With UPnP on: SSDP/IGD traffic.</li><li>Always: the relay control channel in clear, including the token and LAN candidates.</li></ul> | `core/src/network/discovery.rs:150-168`; `core/src/types.rs:410-413` (stale comment) |
| **Network observer (on-path, ISP)** | <ul><li>The peer and relay IPs, timing and volume.</li><li>The plaintext relay token, plus a 4-byte plaintext protocol version.</li><li>The community server's address and traffic volume.</li><li>No content.</li></ul> | `core/src/network/relay.rs` (no AEAD on the control channel); `core/src/network/session.rs:392` |
| **Local attacker with disk access** | <ul><li>Received files and their spools in plaintext.</li><li>`parties.json` (0600, unencrypted): every community address, username, server name and pin.</li><li>On the community server host: `party.db` and `blobs/` under the default umask, readable by other users.</li><li>TUI diagnostics bundles (0644) with logs.</li><li>`crashes/panic-*.log`.</li><li>History and identity are encrypted.</li></ul> | `desktop/src-tauri/src/commands/party.rs:531-538, 577-583`; `server/src/state/mod.rs:379-382`; `server/src/state/blobs.rs:164`; `client/src/support.rs:101-125, 150-158` |
| **OS (notifications, clipboard, swap, journald)** | <ul><li>**Notifications:** the first 50 characters of each received message, plus received file names, with the conversation title. On by default, with no "hide content" option. These persist in the Windows notification database, macOS Notification Center and the GNOME/KDE history.</li><li>**Clipboard:** invite links (IPs and relay token) and fingerprints, never cleared. The TUI `:identity` command copies the fingerprint silently as a side effect.</li><li>**Swap:** no `mlock`, so keys, passwords and decrypted history can be paged out.</li><li>**journald (Linux desktop):** debug stdout, SUSPECTED.</li></ul> | `client/src/app/chat_manager/mod.rs:574-599`, `text.rs:171-179`, `files.rs:549-552`; `client/src/tui/app/mod.rs:1050, 1369-1372`; `desktop/src/components/Creator.jsx:24`, `Relays.jsx:27`, `Contacts.jsx:63` |

## Findings

### PRIV-01: The desktop UI never tells users the community server reads their messages and DMs, and THREAT_MODEL.md claims a badge that does not exist
- Severity: High
- Category: privacy
- Status: VERIFIED
- Location:
  - `desktop/src/components/Parties.jsx:119-124` (the only explanatory copy)
  - `desktop/src/components/Settings.jsx:317-322`: "There is no account and no central server holding your messages."
  - `THREAT_MODEL.md:97`: "the operator wears an explicit 'this operator can read messages' badge"
  - `core/src/party/mod.rs:23-29`: `TrustTier` exists in the data model.
  - `server/src/state/db.rs:371-385`: plaintext envelopes. Other auditors also found README.md:8-9/34/41.
- **Problem:**
  - Other auditors found the plaintext storage. What is new here is that **no UI surface discloses it**:
    - `rg -i "operator|tier|administered|can read"` over `desktop/src/**/*.jsx` finds no tier badge and no warning.
    - The community join card says only "administered, multi-channel rooms that keep history".
    - The "How P2PEM works" panel in Settings says flatly that there is no central server holding your messages.
  - The `TrustTier` field travels with every envelope, but nothing renders it.
  - THREAT_MODEL.md lists the badge as a *control* that mitigates the plaintext risk, so the threat model relies on a mitigation that was never built.
  - Only `docs/USER_GUIDE.md:249` (operator notes) tells the truth, and it is addressed to operators, not to members.
  - "Community DMs" sit next to E2E P2P DMs in the same app with the same "DM" vocabulary.
- **Impact:** Users will reasonably assume that a "DM" inside the same E2E messenger is end-to-end encrypted. Anything they send there, including credentials, files and personal content, is readable by the operator and stored indefinitely on the operator's disk.
- **Recommendations:**
  - Render `envelope.tier` and a persistent banner on every community view and every community DM composer: "This community's operator can read all messages and files here, including DMs."
  - Add a one-time interstitial before the first community join.
  - Fix the Settings copy.
  - Either implement the badge or remove it from THREAT_MODEL.md's controls list until it exists.
  - Add a component test asserting the banner is present, so it cannot regress silently.
- Effort: S

### PRIV-02: The same long-term identity key authenticates the user to every community server, so operators can link users across communities and to their P2P identity
- Severity: Medium
- Category: metadata
- Status: VERIFIED
- Location:
  - `desktop/src-tauri/src/commands/party.rs:323-324`: `crate::lock_identity(&state.identity)` → `id.private_key()` is handed to `connect_and_join`.
  - `client/src/app/party_manager.rs:395-404`: `PartyClient::connect(stream, privkey, …)`.
  - `server/src/state/db.rs:360-364`: `members(id, username, fingerprint)`.
- **Problem:** The community handshake reuses the user's P2P RSA identity. Every community server therefore records the user's global fingerprint, the same value that appears in their invite links, in mDNS (older versions) and in every P2P contact's records. Two community operators, or a community operator and any P2P contact, can match records on that fingerprint and learn that the pseudonymous "alice" in community A is "bob" in community B and is identity X on P2P. Nothing tells the user this, and nothing lets them use a per-community key.
- **Impact:** This defeats per-community pseudonymity, and it is invisible to the user. It also widens the impact of the plaintext-server issue: the operator holds plaintext messages bound to a globally stable identifier.
- **Recommendations:**
  - Derive a per-community key: an Ed25519 key from HKDF(identity secret, server fingerprint), or simply a fresh random key per saved community, stored next to its pin in an encrypted store.
  - Use that key for community authentication.
  - If a shared identity is ever wanted (for example for future sealed tiers), make it opt-in and say so.
  - Document the linkage in THREAT_MODEL.md now.
- Effort: M

### PRIV-03: Relay hole punching discloses each peer's public and LAN addresses to whoever holds the token, before verification, and the relay control channel is plaintext
- Severity: Medium
- Category: metadata
- Status: VERIFIED
- Location:
  - `core/src/network/relay.rs:1-19`: design notes.
  - `relay.rs:86-108`: `PunchCaps { local_addrs }` and `PunchStart { peer_public, peer_locals }`.
  - `relay.rs:405-409`: the client self-reports `primary_local_ipv4()`.
  - `relay.rs:696-704`: the server forwards both.
  - `NO_HOLEPUNCH_ENV` (`relay.rs:62`) is the only opt-out, and it is an environment variable.
- **Problem:** Users pick "relay" in the UI (`Relays.jsx:81`: "A relay is a blind broker … It forwards bytes only"), and many will read that as "my peer does not learn my IP". In fact, by default:
  1. The relay sends each side the other's observed public endpoint *and* its LAN IP:port.
  2. This happens after token pairing but **before** the v3 handshake and SAS. Anyone who obtains a token, for example from a forwarded invite link or clipboard history, learns the host's public and private addresses just by joining.
  3. The control frames, including the token and LAN candidates, are unencrypted bincode, so on-path observers see them too.
- **Impact:** This is IP and LAN-topology disclosure to unverified parties. Hosting through a relay to hide your address does not work, and nothing in the UI or settings exposes that.
- **Recommendations:**
  - Add a user setting ("Never reveal my IP to peers: always bridge through the relay") that sets the equivalent of `P2PEM_NO_HOLEPUNCH` and suppresses LAN candidates.
  - Send LAN candidates only after both sides have proven possession of the token. An HMAC over the token-derived tag would do; the code already derives a tag in `punch.rs`.
  - Consider TLS/Noise on the relay control channel.
  - Correct the Relays.jsx copy to say that the relay may connect you directly and reveal addresses.
- Effort: M

### PRIV-04: The TUI log buffer and "Export diagnostics" bundle capture relay tokens, peer IPs, file names and paths, contact names and the external IP, under a README that implies the bundle is safe to share
- Severity: Medium
- Category: logging
- Status: VERIFIED
- Location:
  - `client/src/main.rs:76-83`: filter `info,p2pem_classic=debug` into `LogBuffer`.
  - `client/src/main.rs:86`: `tracing::debug!(?args, "Parsed CLI arguments")`. `Args` contains `relay_token`, `connect`, `host_relay` and `connect_relay` (`main.rs:20-57`).
  - `client/src/support.rs:69-82, 91-126`: `logs.txt`, plus a README saying "Share diagnostics.json and logs.txt when reporting a bug. Private keys are not included."
  - `client/src/tui/app/mod.rs:1387-1420`: export.
  - `client/src/tui/app/mod.rs:1375-1383`: Ctrl+L copies the whole buffer to the clipboard.
- **What a bundle contains at default levels:**
  - The relay token, which is the rendezvous bearer for that host, from the parsed CLI arguments.
  - Peer IPs: `core/src/network/session.rs:138` "Client connected from {}" at info, `:172/176` candidate host:port, `:238` "Connected to {}:{}".
  - The full local path of every file sent: `client/src/app/chat_manager/files.rs:47`.
  - Received file names: `events.rs:605`.
  - Received file save paths: `core/src/transfer/receiver.rs:211, 352` "File saved to: {:?}".
  - The UPnP/NAT-PMP external IP: `core/src/network/nat.rs:122, 154`; `events.rs:756`.
  - Contact names on add, block and unblock: `contacts.rs:36, 198, 259`.
  - Discovered mDNS peers' names and addresses: `discovery.rs:221-226`.
  - From `diagnostics.json`: a 16-hex-char (64-bit) fingerprint prefix, the absolute data paths (which usually contain the OS username), the relay server and the download directory.
- **What it does not contain:** message bodies. `ProtocolMessage`'s custom `Debug` redacts `Text`/`TextChunk` (`core/src/core/protocol.rs:82-117`).
- **Problem:** No redaction pass runs, no preview is shown, and no warning is given. The files are written with `std::fs::write`, so they get the default umask (0644 on Unix).
- **Impact:** A user following the README attaches, publicly to a GitHub issue, a file that maps their contacts, IPs, file names and a live relay token.
- **Recommendations:**
  - Remove `?args` logging, or log only flag presence.
  - Add a redaction layer for the buffer: IPs → `x.x.x.N`, paths → basename, a `relay_token` field → hash prefix.
  - Downgrade the "File saved to"/"Preparing to send file" path logs to debug and log basenames only.
  - Write bundles with `write_file_atomic` (0600).
  - Change the README text to list exactly what is included and tell the user to review it before sharing.
  - Show the contents (or at least their categories) in the UI before export.
- Effort: S

### PRIV-05: Desktop debug logging (filenames, peer addresses, frame metadata) goes to stdout, which on Linux plausibly persists in the systemd journal
- Severity: Medium
- Category: logging
- Status: SUSPECTED (the default filter is VERIFIED; journald capture depends on how the desktop session launches apps)
- Location:
  - `desktop/src-tauri/src/lib.rs:327-336`: default `EnvFilter::new("info,p2pem_classic=debug,messenger_core=debug")` with an fmt layer to stdout.
  - `core/src/network/session.rs:1124, 1210`: `debug!("Sending message: {:?}") / ("Received message: {:?}")`, where `FileMeta` prints its `filename` (`protocol.rs:118+`).
  - `desktop/src-tauri/src/main.rs:2`: `windows_subsystem = "windows"` in release (stdout discarded on Windows).
- **Problem:** Other auditors found the debug default. To go further:
  - On Windows the output vanishes.
  - On macOS a Finder launch sends it to `/dev/null`.
  - On GNOME/KDE, applications launched from a `.desktop` entry, which is what the `.deb`, `.rpm` and AppImage integrations produce, commonly have their stdout and stderr captured by systemd-journald under the user unit. Every frame, file name and peer address then lands in a persistent, unencrypted journal that the user does not know exists and that outlives "Delete all data".
  - `export_diagnostics` on desktop tells the user that logs go to stdout "or check the OS console", which confirms the design.
- **Recommendations:**
  - Default release builds to `info` (or `warn`) for `messenger_core` and `p2pem_classic`; keep debug behind `RUST_LOG` or `cfg(debug_assertions)`.
  - Stop logging per-frame `Debug` at debug level, and move it to `trace`.
  - Remove the unused `tauri-plugin-log` (SUPPLY-07), or use it with an explicit in-memory sink.
- Effort: S

### PRIV-06: Desktop notifications show message text by default with no "hide content" option
- Severity: Medium
- Category: privacy
- Status: VERIFIED (this extends another auditor's finding)
- Location:
  - `client/src/app/chat_manager/mod.rs:574-599`
  - `text.rs:171-179`: the first 50 characters.
  - `files.rs:549-552`: "📎 <filename>".
  - `core/src/types.rs:467`: `enable_notifications: true`.
  - `desktop/src/components/Settings.jsx:231-232`: a single on/off toggle.
- **Problem:** The only choice is all or nothing. Mainstream E2E messengers default to, or at least offer, "show sender only" or "show nothing". The content goes into OS-managed, unencrypted notification stores:
  - the Windows `wpndatabase.db`
  - the macOS NotificationCenter database
  - the GNOME notification history
  It is also shown on lock screens, and it lives on after the encrypted history is deleted. The file name variant leaks received file names the same way.
- **Recommendations:**
  - Add a three-way `notification_preview` setting: full / sender only / generic.
  - Default it to "sender only", or at least make the lock-screen-safe mode easy to find.
  - Never put file names in notifications under the non-full modes.
- Effort: S

### PRIV-07: The invite link silently embeds the LAN IP, the external IP and a creation timestamp, and is pushed through the clipboard, which is never cleared
- Severity: Low
- Category: metadata
- Status: VERIFIED (this extends another auditor's finding)
- Location:
  - `desktop/src-tauri/src/commands/contacts.rs:72-91`: external mapping first, then `primary_local_ipv4()` plus the listen port.
  - `core/src/identity/mod.rs:523-552`: `timestamp`, `nonce`, `name`, `addresses`, relay token.
  - Clipboard writes: `desktop/src/components/Creator.jsx:24`, `Relays.jsx:27` (relay token), `Contacts.jsx:63`; `client/src/tui/app/cmd_connect.rs:55`, `cmd_contacts.rs:103`, `client/src/tui/app/mod.rs:1050` (`:identity` copies the fingerprint as a side effect).
- **Problem:**
  - The `primary_local_ipv4` trick (`core/src/util.rs:537-545`) reveals the internal address even when the user only wants to share a relay contact.
  - The timestamp tells a recipient when the invite was minted.
  - The links, which carry relay bearer tokens, stay on the clipboard indefinitely, where they are visible to clipboard managers, Windows Clipboard History/Cloud Clipboard and Universal Clipboard.
  - The TUI copy on `:identity` is undisclosed and unrequested.
- **Recommendations:**
  - Say in the Share panel that the link contains your IP addresses, and offer "relay-only (no addresses)".
  - Exclude RFC1918 addresses unless the user chooses "LAN".
  - Clear the clipboard after about 60 seconds if it still holds the value the app wrote.
  - On Windows, set the `ExcludeClipboardContentFromMonitorProcessing` / `CanIncludeInClipboardHistory=0` formats (arboard exposes `set().exclude_from_history()` on Windows).
  - Make the TUI `:identity` command display the fingerprint without copying it.
- Effort: S

### PRIV-08: Zeroize coverage stops at the PEM and the AEAD keys: the history key, passwords and decrypted history are never wiped, and there is no swap protection
- Severity: Low
- Category: privacy
- Status: VERIFIED
- Location:
  - `core/src/identity/mod.rs:715-722`: `history_key()` returns a plain `[u8; 32]`, and the `digest` `GenericArray` is not wiped.
  - `client/src/app/chat_manager/mod.rs:175`: `history_key: Option<[u8; 32]>` (`Copy`, so it is duplicated on every call).
  - `client/src/app/persistence.rs:146-150`: the decrypted history `plaintext` `Vec` is dropped unwiped.
  - `client/src/app/persistence.rs:170`: `serde_json::to_string(&self)` is the full plaintext history, built on every save and never wiped.
  - `core/src/identity/mod.rs:363-364`: `String::from_utf8(plaintext)`; on the error path the decrypted PEM bytes are dropped inside the `FromUtf8Error`.
  - `desktop/src-tauri/src/commands/auth.rs:67, 116`: `password: String`.
  - `client/src/app/chat_manager/mod.rs:190`: `connection_password: Option<String>`.
  - Server `--password`/`PARTY_PASSWORD` is a plain `String`, also visible in `ps` when passed as an argument (`server/src/main.rs:40-42`).
  - There is no `mlock`/`VirtualLock` anywhere: `rg mlock` returns nothing.
- **What is covered:** the private-key PEM (`Zeroizing<String>`), Argon2 output, the session AES key and the ed25519 seed (`crypto.rs:230, 553`; `session.rs:475`). `RsaPrivateKey` zeroizes on drop.
- **Problem:** The history key decrypts *all* stored messages and is copied freely. Passwords and the whole decrypted history linger in freed heap and can be paged to swap or captured in crash dumps or hibernation files.
- **Recommendations:**
  - Make `history_key` return `Zeroizing<[u8; 32]>`, and store it as `Option<Zeroizing<[u8; 32]>>` (non-`Copy`).
  - Wrap password parameters in `Zeroizing<String>` immediately on entry: Tauri deserialises into `String`, so convert at the top of the command.
  - Serialise history into a `Zeroizing<Vec<u8>>` (`serde_json::to_writer`).
  - Wipe the decrypt buffer on both paths.
  - Prefer `PARTY_PASSWORD` over `--password` in the docs.
  - Optionally use the `memsec`/`region` crate to `mlock` the key page.
- Effort: M

### PRIV-09: The community server writes plaintext `party.db` and `blobs/` under the default umask, and server logs record member IPs at info
- Severity: Medium
- Category: privacy
- Status: VERIFIED (this extends another auditor's finding)
- Location:
  - `server/src/state/mod.rs:379-382`: `create_dir_all` and `Connection::open`, with no mode.
  - `server/src/state/blobs.rs:164`: `File::create` for staging, then a rename into the store.
  - `server/src/main.rs:46`: default data dir `party-data`, relative to the working directory.
  - `server/src/lib.rs:63-75`: `%addr` at info on connect and disconnect.
  - `server/src/main.rs:54`: default filter `info`.
  - Contrast `server/src/identity.rs:39-90`: the key *is* written 0600 from creation.
- **Problem and impact:** On a shared host, every local user can read all community history, all DMs and all files. The server also builds an indefinite IP log of members by default, which raises the operator's data-retention exposure.
- **Recommendations:**
  - Create the data dir 0700 (`DirBuilderExt::mode`) and set `umask(0o077)` at server start, or open the DB and blobs with explicit modes.
  - On startup, refuse to run (or warn) if the data dir is group- or world-readable.
  - Log connection IPs at debug, or hash them.
  - Document retention.
- Effort: S

### PRIV-10: `parties.json` stores the user's community list in plaintext, outside the encrypted history
- Severity: Low
- Category: privacy
- Status: VERIFIED
- Location: `desktop/src-tauri/src/commands/party.rs:531-538` (`address`, `username`, `name`, `fingerprint`); `desktop/src-tauri/src/lib.rs:428`.
- **Problem:**
  - The file is 0600 and atomic, which is good, but it is not encrypted with the history key. Anyone with disk access can read which communities a user belongs to and under which usernames, even when the app is locked. Everything else of comparable sensitivity (contacts, chat titles) is inside `history.json.enc`.
  - The pins are not integrity-protected either, so a local attacker can replace them, although a local attacker is largely out of scope.
- **Recommendation:** Move the saved-communities list into the encrypted history file. Alternatively, encrypt it with a subkey of the history key and AEAD it, so the pins are also authenticated.
- Effort: S

### PRIV-11: Delivery receipts cannot be disabled, and typing indicators default to on
- Severity: Low
- Category: metadata
- Status: VERIFIED
- Location:
  - `client/src/app/chat_manager/events.rs:598` and `files.rs:541`: `send_ack_for_chat` is unconditional.
  - `client/src/app/chat_manager/mod.rs:634-636`.
  - `core/src/types.rs:468`: `enable_typing_indicators: true`.
  - `text.rs:276`: the gate for typing.
- **Problem:** Every received message tells the sender that the app is online and has processed it, which acts as a presence oracle. Typing activity also leaks by default. Both are normal product choices, but neither is described in the privacy copy, and receipts have no switch at all.
- **Recommendation:** Add an `enable_delivery_receipts` setting, and mention both in the Settings privacy section.
- Effort: S

### PRIV-12: Stale privacy comment in `Config` about mDNS
- Severity: Info
- Category: privacy
- Status: VERIFIED
- Location: `core/src/types.rs:410-413` says enabling mDNS "will advertise a hostname and a fingerprint". `core/src/network/discovery.rs:138-149` documents that the fingerprint was removed from the TXT record.
- **Recommendation:** Update the comment. Also tell users in the toggle hint that the OS hostname is broadcast; it often contains the person's real name, such as "Janes-MacBook-Pro". Consider advertising a random instance or host label instead of `hostname::get()` (`discovery.rs:159`).
- Effort: S

### SUPPLY-01: The process gap: new RustSec advisories break unrelated Rust PRs, frontend PRs merge green past them, and the daily audit only files an issue
- Severity: Medium
- Category: supply-chain
- Status: VERIFIED
- Location:
  - `.github/workflows/ci.yml:776-811`: the `supply-chain` job, gated on `needs.changes.outputs.rust == 'true'`, running `cargo deny --all-features check` as a hard failure.
  - `.github/workflows/audit.yml:15-19, 145-176`: daily at 07:00 UTC; on failure it comments on or creates one issue and fails.
  - The incident: RUSTSEC-2026-0385 (ctutils), -0386 (tokio `AsyncFd`) and -0370/-0371/-0372 (zvariant/zbus_names/zvariant_derive via `notify-rust 4.18.2 → zbus 5.18.0`) turned PR CI red before anyone acted. They were cleared by `49a4e6e`: ctutils 0.4.2→0.4.8, tokio 1.53.2→1.54.1, three new `ignore` entries.
- **Problem:**
  1. **Asymmetric gating.** A PR that touches only Rust *formatting* fails on an advisory it did not introduce, while a frontend-only PR (`rust=false`) skips the job, so `ci-success` stays green on a tree with an open vulnerability. The schedule exists precisely because "did the world change" is not a diff property, but the PR gate is still diff-scoped.
  2. **Detection without remediation.** The scheduled job runs once a day, after most advisories are published, and only produces an issue. No branch or PR with `cargo update -p <crate>` is prepared. The first signal most contributors see is their own red PR.
  3. **The stale-acceptance checker makes pre-staging impossible**, and that is correct, so the window between publication and the fix is entirely manual.
- **Recommendations:**
  - On PRs, split cargo-deny into `cargo deny check bans licenses sources` (hard, diff-relevant) and `cargo deny check advisories`. Report the advisories half without blocking when the advisory is not on a crate changed in the diff; compare `Cargo.lock` against the base, or just use `continue-on-error` plus a PR annotation. This is the mode cargo-deny's own docs recommend.
  - Keep advisories hard on `push` to `main` and on the schedule.
  - Run audit.yml more often (every 6 hours costs little).
  - When it fails on `advisories`, have it try `cargo update -p` for each affected crate and open a PR (or let Dependabot security updates do it: enable "Dependabot security updates" in repository settings; that is not in dependabot.yml and cannot be verified from the repo).
  - Make `ci-success` require `supply-chain` to have run at least once on `main` within N hours. As a simpler option, run advisories unconditionally (ungated) in `ci.yml`, as is already done for secret-scan.
- Effort: S

### SUPPLY-02: tauri-action is given `tagName`/`releaseName`, which makes it try to create or upload to the GitHub release, contradicting the attest-then-publish design
- Severity: Medium
- Category: supply-chain
- Status: SUSPECTED (tauri-action's upload-when-`tagName`-is-set behaviour is upstream; it should be confirmed against tauri-action v0.6.2's `action.yml`)
- Location:
  - `.github/workflows/release.yml:247-257`: `tauri-apps/tauri-action@84b9d35…` with `GITHUB_TOKEN`, `tagName: ${{ github.ref_name }}` and `releaseName`.
  - Workflow-level `permissions: contents: read` (`release.yml:17-18`), not overridden in the `desktop` job.
  - The header comment (`release.yml:13-16`) says "The build matrices no longer upload anything to the release".
- **Problem:** When `tagName` is set, tauri-action looks up a release for that tag, creating it if absent, and uploads the bundles to it.
  - The release is a **draft**. Drafts are not returned by `GET /releases/tags/{tag}` and are listable only with push access.
  - With a `contents: read` token, the action will most likely fail trying to create a release, so all four desktop legs fail and no release can be published.
  - If anyone "fixes" that by granting `contents: write` to the job, the bundles are uploaded straight to the release from the build legs. That bypasses the attestation boundary the workflow was built around, which is exactly the regression its comment warns about.
- **Recommendation:** Remove `tagName`, `releaseName` (and `releaseBody`/`releaseId`) from the tauri-action step so it only builds and emits `artifactPaths`. Add an actionlint or grep check that tauri-action is never given release inputs in this workflow. Dry-run the workflow on a throwaway `v0.0.0-test` tag in a fork.
- Effort: S

### SUPPLY-03: Dependabot does not cover GitHub Actions or the fuzz workspace, and the fuzz lockfile is git-ignored
- Severity: Low
- Category: dependency
- Status: VERIFIED
- Location:
  - `.github/dependabot.yml`: only `cargo` at `/` and `npm` at `/desktop`.
  - `.gitignore:6`: `Cargo.lock`, unanchored. `git check-ignore` reports that it matches `core/fuzz/Cargo.lock`. The root `Cargo.lock` is tracked, so it is unaffected today, but the rule is a trap for any new workspace.
  - `.github/workflows/fuzz.yml:37-72`.
- **Problem:**
  - There are 14 distinct SHA-pinned actions (`actions/checkout@11d5960…` "v4", `Swatinem/rust-cache`, `taiki-e/install-action`, `dtolnay/rust-toolchain`, `tauri-apps/tauri-action`, `actions/attest-build-provenance`, and others). SHA pinning is right, but without the `github-actions` ecosystem those pins never receive update PRs, including security fixes and runtime deprecations such as the Node 20 removal.
  - `core/fuzz` is a separate workspace (per CLAUDE.md) with no tracked lockfile and no Dependabot entry, so the weekly fuzz job resolves fresh, unreviewed dependency versions every run.
- **Recommendations:**
  - Add `package-ecosystem: github-actions, directory: /` (Dependabot updates SHA pins and keeps the version comment).
  - Add `cargo` for `/core/fuzz`.
  - Track `core/fuzz/Cargo.lock` and anchor the ignore rule (`/target/`-style), or remove it.
  - Run the fuzz build with `--locked`.
- Effort: S

### SUPPLY-04: CI's secret-scan job installs gitleaks with no checksum (`curl | sudo tar`)
- Severity: Low
- Category: supply-chain
- Status: VERIFIED
- Location: `.github/workflows/ci.yml:893-900`. Contrast `.github/workflows/audit.yml:70-86`, which downloads, verifies `GITLEAKS_SHA256=551f6fc8…`, and comments that the pipe-to-tar form was the one unverified tool.
- **Problem:** The fix was applied to the scheduled workflow only. The PR-time job, which runs on every PR and is ungated, still extracts an unverified tarball as root. A compromised release asset or a redirect would execute in CI. The token is read-only and `persist-credentials: false`, so the blast radius is limited, but the inconsistency means the guarantee in audit.yml's comment does not hold for the PR job.
- **Recommendation:** Copy the audit.yml block, including `--proto '=https' --tlsv1.2` and `sha256sum -c`, into ci.yml. Better, put it in a shared composite action so the two cannot drift again.
- Effort: S

### SUPPLY-05: The committed `desktop/dist` is effectively unreviewable in PRs, and the staleness check ignores whitespace
- Severity: Low
- Category: supply-chain
- Status: VERIFIED (the exploitability of whitespace-only edits is low)
- Location:
  - `desktop/dist/assets/index-ax_52jWK.js`: 354 KB in 8 lines, containing a NUL byte at offset ~237904.
  - `.gitattributes`: only `* text=auto`; no `linguist-generated`.
  - `.github/workflows/ci.yml:468-512`: `git diff --quiet --ignore-all-space --ignore-blank-lines --ignore-cr-at-eol -- dist`.
  - `desktop/src-tauri/tauri.conf.json`: `frontendDist: ../dist`, which is embedded at `cargo build` time.
- **Problem:**
  - A PR can change the minified bundle in ways no human reviewer will read; GitHub does not render a 354 KB single-line diff. The only control is the rebuild comparison.
  - That comparison:
    - ignores **all** intra-line whitespace, so whitespace inside string literals, template literals and regexes is not compared
    - runs only on `ubuntu-latest`, and only when the classifier marks the PR `frontend`
    - is in the `frontend` job, which is part of `ci-success`, so it is enforced
  - Releases rebuild `dist` (`release.yml:238-242`), so installers are not affected. But every `cargo build --workspace` in CI, and every developer's `cargo run -p p2pem-desktop` without `tauri dev`, embeds the committed bundle. A malicious bundle would therefore run on contributor machines with full IPC access to the bridge (identity unlock, file open, and so on).
- **Recommendations:**
  - Mark `desktop/dist/** linguist-generated=true -diff` so reviewers see "generated" rather than a fake-reviewable diff.
  - Add a CODEOWNERS rule requiring maintainer review for `desktop/dist/**`.
  - Replace the whitespace-insensitive comparison with a byte-exact comparison of the hashed **assets** (`dist/assets/*`, where the hash already proves identity) and keep whitespace tolerance only for `dist/index.html`.
  - Alternatively, have `tauri-build`'s `build.rs` fail when `dist/` is older than `src/`.
- Effort: S

### SUPPLY-06: The accepted-advisory list has no structured reasons or expiry; RUSTSEC-2023-0071's rationale is narrower than the advisory; 55 crates are duplicated, including two RustCrypto generations
- Severity: Low
- Category: dependency
- Status: VERIFIED (the advisory scope point is SUSPECTED)
- Location:
  - `deny.toml:17-58`: `ignore` lists RUSTSEC-2023-0071, -2024-0436, -2025-0141, -2024-0370, -2026-0194, -2026-0195, -2026-0370, -2026-0371 and -2026-0372.
  - `deny.toml` `[bans] multiple-versions = "warn"`.
  - `Cargo.toml` (workspace dependencies).
  - `Cargo.lock`.
- **Each acceptance in turn:**
  - **RUSTSEC-2023-0071 (rsa Marvin).** The justification says "this app performs no RSA decryption". That is VERIFIED: there is no `Pkcs1v15Encrypt`/`Oaep` use anywhere. But the root cause is non-constant-time bignum arithmetic (num-bigint-dig) on *private-key operations*. Both the client (`IdentityProof` RSA-PSS on every handshake, `core/src/network/session.rs`) and the community server (its RSA identity, `server/src/identity.rs`) perform remotely triggerable RSA signatures, mitigated by blinding. The entry should say "signing only; blinding in place; Ed25519 migration is the exit" (`ed25519-dalek` 3.0 is already a dependency and `session.rs:440` negotiates a scheme).
  - **RUSTSEC-2025-0141 (bincode unmaintained).** "Migration pending" has no owner or date. bincode 1 decodes peer-controlled relay control frames and Party frames (about 30 call sites); `util::decode_exact` mitigates trailing bytes.
  - **RUSTSEC-2024-0436 (paste) and RUSTSEC-2024-0370 (proc-macro-error).** Build-time only. Fine.
  - **RUSTSEC-2026-0194/-0195 (quick-xml).** The proc-macro-only path is well argued.
  - **RUSTSEC-2026-0370/-0371/-0372 (zbus family via notify-rust).** Well argued, with a date and a removal condition. It is the template the others should follow.
- **Duplicates.** `Cargo.lock` has 749 packages and 55 names with more than one version. Among them:
  - Two RustCrypto generations: `sha2` 0.10.9/0.11.0, `digest` 0.10/0.11, `aead` 0.5/0.6, `cipher` 0.4/0.5, `chacha20` 0.9.1/0.10.2, `signature` 2/3, `rand` 0.8.7/0.9.5/0.10.2, `rand_core` 0.6/0.9/0.10, `getrandom` 0.2/0.3/0.4.
  - The cause is the direct pins `aes-gcm = "0.11"` alongside `chacha20poly1305 = "0.10.1"`, `sha2 = "0.10"`, `rand = "0.8.6"` and `x25519/ed25519-dalek 3.0` (`Cargo.toml` workspace dependencies).
  - That doubles the crypto code that needs review and patching.
  - Also present: `syn` 1/2/3 and `base64` 0.21/0.22/0.23.
- **Recommendations:**
  - Convert the ignores to the `{ id, reason }` table form, with an issue link and review date for each.
  - Reword RUSTSEC-2023-0071's reason accordingly.
  - Put an owner and target release on the bincode migration.
  - Plan a single-generation crypto upgrade (`chacha20poly1305` 0.11, `sha2` 0.11, `rand` 0.9+).
  - Add `[bans] deny` for crates that should never appear (`openssl`, `openssl-sys`, any `egui`/`eframe`, which CLAUDE.md requires to be absent) so that rule is enforced by tooling.
- Effort: M

### SUPPLY-07: Unused `tauri-plugin-log` and `log` dependencies add build-script and transitive surface
- Severity: Low
- Category: dependency
- Status: VERIFIED
- Location: `desktop/src-tauri/Cargo.toml` (`tauri-plugin-log = "2"`, `log = "0.4"`); `Cargo.lock:6018` (`tauri-plugin-log 2.10.0`, which pulls `fern`, `android_logger`, `swift-rs`, `objc2`, `time`). `rg "tauri_plugin_log|log::" desktop/src-tauri/` finds no use: the plugin is never registered in `lib.rs:347-356`.
- **Problem:** The shipped app compiles a plugin and its build script (`tauri-plugin` build support), and its transitive crates, for nothing. Every unused dependency is supply-chain exposure with no benefit.
- **Recommendation:** Remove both. Consider adding `cargo-machete` or `cargo-udeps` to Fast Checks.
- Effort: S

### SUPPLY-08: Dev-only npm advisories (2 high, 8 moderate), and release builds run npm lifecycle scripts
- Severity: Low
- Category: dependency
- Status: VERIFIED
- Location: `desktop/package-lock.json`; `npm audit --omit=dev` reports 0 vulnerabilities.
- **Advisories in the full tree:**
  - `brace-expansion` (high, GHSA-q2hr-2g5m-vwhr / -qhr7-859c-m2p7 / -6j4f-fj2g-mc7p)
  - `zod` (high, GHSA-mrhx-c7xc-m3w5)
  - `@vitest/mocker` (GHSA-qxg6-89mg-rqv8 code injection via unauthenticated manual mock registration; GHSA-xp26-3454-mjgp)
  - `minimatch` (GHSA-g3hv-75wg-m249)
  - eslint chain
- **The tree itself:** 378 packages, of which only 5 are production dependencies: `@tauri-apps/api ~2.11`, `react`, `react-dom`, `lucide-react` and their dependencies. Only `fsevents` (dev, optional) has an install script.
- **Lifecycle scripts:** `release.yml:235` runs `npm ci` with lifecycle scripts, and a dev dependency executing during `vite build` can write into `dist/`. That output is then embedded, attested and published. Provenance proves "built by this workflow", not "free of a compromised build tool".
- **Problem:** None of these advisories ship, but CI gates only `--omit=dev --audit-level=high`, so the vitest mock-registration issue (a dev-server code-injection vector on a developer machine running `vitest --ui`/browser mode) is never surfaced.
- **Recommendations:**
  - Bump vitest, zod and brace-expansion via overrides.
  - Use `npm ci --ignore-scripts` in `release.yml` and the CI frontend job: Playwright browsers are installed by an explicit step and `fsevents` does not need its script.
  - Have audit.yml report the dev tree as an informational section.
- Effort: S

### SUPPLY-09: Releases carry provenance but no SBOM, and installers are unsigned
- Severity: Info
- Category: supply-chain
- Status: VERIFIED
- Location: `release.yml:411-470`.
- **Problem:**
  - Attestation and `SHA256SUMS` are excellent: built from the run's own artifact store, the published bytes re-verified, and the release left as a draft until the end.
  - There is no SBOM (CycloneDX/SPDX), so users and downstream packagers cannot check a shipped binary against future advisories without rebuilding.
  - No Authenticode or Apple notarisation, a choice documented at `release.yml:398-420`.
  - The release `tools` tarballs (`p2pem`, `p2pem-server`) are plain archives.
- **Recommendation:** Generate SBOMs with `cargo cyclonedx` and `npm sbom --omit=dev`, then attest them with `actions/attest-sbom` (pinned by SHA) alongside the provenance.
- Effort: S

### SECRETS-01: No committed secrets found; the one historical fixture is properly allowlisted
- Severity: Info
- Category: secrets
- Status: VERIFIED
- **Evidence:**
  - `rg` over the tree, excluding `target`/`node_modules`/`.git`, for PEM private-key headers, `ghp_`, `github_pat_`, `AKIA…`, `xox[bp]-` and `sk-…` finds only the comment in `.gitleaksignore`.
  - `git log --all -S "PRIVATE KEY-----"` finds only `19e00af` (a truncated fixture: the header followed by "trunc") and `a41ae3a`, which rewrote it to build the string at runtime.
  - `.gitleaksignore` pins exactly `19e00af…:server/src/identity.rs:private-key:100`.
  - `.gitleaks.toml` allowlists one sequential hex placeholder.
  - No `.pem`, `.key`, `.p12`, `.env`, `identity.json`, `party.db` or `.enc` file has ever been added (`git log --all --diff-filter=A`).
  - The history-key KAT deliberately encodes its RSA test key as components, not as a PEM (`core/src/identity/mod.rs:733-740`).
- Effort: n/a

## Things done well

- **Every GitHub Action is pinned to a full commit SHA** with a version comment, and the release workflow explains why: `attest` holds `id-token: write`. Workflows default to `contents: read`, with per-job grants. `persist-credentials: false` is used almost everywhere. There is no `pull_request_target`.
- **Release integrity design.** These are all properties of `release.yml`:
  - Builds are cold, with no cache.
  - Tauri and tools legs run `cargo build --locked`, and the frontend uses `npm ci`.
  - The tag must match the workspace version.
  - Artifacts go to the run-scoped store, are collision-checked, hashed into `SHA256SUMS` *before* attestation, and attested with Sigstore.
  - The release is uploaded only while still a draft, re-downloaded and re-verified, and published last.
  - SECURITY.md documents `gh attestation verify --signer-workflow`.
- **The tools CI downloads are hardened**, with these exceptions: actionlint and gitleaks are checksum-verified in audit.yml (not yet in ci.yml, see SUPPLY-04). audit.yml uses `npm ci --ignore-scripts`. Secret scanning covers full history and is deliberately ungated by path.
- **cargo-deny with a stale-acceptance checker** (`scripts/check-stale-advisories.sh`) that parses only the `ignore` array and fails on dead entries. Licences are allow-list only and compatible with MIT. Sources are restricted to crates.io, and there are no git dependencies. `cargo-deny check` passes at `62b952f`.
- **The scheduled audit** files a single issue rather than one per day, and runs every check even if an earlier one fails.
- **CodeQL advanced setup** covers Rust, JavaScript/TypeScript and Actions, with a documented exclusion rationale.
- **Workspace hygiene.** `base64` is set to `default-features = false` ahead of 0.23's SIMD `unsafe`. `rusqlite` uses `bundled`. The release profile uses `strip`, `lto` and `panic=abort`.
- **Privacy defaults are mostly conservative.** mDNS, UPnP, `auto_trust_on_first_use`, `auto_accept_files` and `auto_host_on_startup` are all **off** by default, and tests pin those defaults (`core/src/types.rs:465-505, 554-556`). mDNS no longer broadcasts the fingerprint (`discovery.rs:138-149`). The display name defaults to a generic "User", not the OS username (`desktop/src-tauri/src/lib.rs:403`).
- **Message bodies are redacted from logs** by a hand-written `Debug` for `ProtocolMessage` (`core/src/core/protocol.rs:82-117`). The TUI has no stdout log layer; logs live only in a bounded 2,000-event in-memory buffer (`client/src/logbuf.rs`). The desktop diagnostics bundle contains metadata only, with no logs.
- **On-disk secrets are handled carefully.** `identity.json`, the encrypted history and `parties.json` use `write_file_atomic` (0600 from creation, fsync, rename). The server identity is written 0600 from creation. Legacy plaintext history is migrated and then deleted or truncated. The private-key PEM, Argon2 output, session keys and the ed25519 seed are `Zeroizing`.
- **The webview is locked down.** The CSP is strict (`script-src 'self'`, `connect-src` limited to IPC, `object-src 'none'`, `frame-ancestors 'none'`), `withGlobalTauri: false`, and the capabilities are only `core:default`. The bundle contains no third-party network origins: only the w3.org namespace and react.dev error-URL strings.
