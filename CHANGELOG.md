# Changelog

All notable changes to this project are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
(`Added` / `Changed` / `Fixed` / `Security` / `Removed`, plus `Performance`,
`Documentation`, and `Dependencies` where they earn their keep), and the
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Versions with a published tag link to a GitHub comparison; earlier versions
predate tagged releases.

## [Unreleased]

### Added

- **Per-file permissions in communities.** A shared file now carries four
  separate rights — see it, download it, remove it, share it on — set for
  everyone who can reach it or for one person. Seeing that a file exists is no
  longer the same as being able to download it. You can only pass on what you
  hold yourself, and you keep full control of anything you shared.
- **Sharing a file on without re-uploading it.** The server stores each file
  once, so posting one you already have into another channel or DM costs
  nothing and takes no time.
- **The app is usable with a keyboard and a screen reader.** A skip link jumps
  straight to the message box instead of making you tab past the toolbar, five
  navigation buttons and every conversation you have. The conversation list is
  announced as a list, with each row saying how many unread messages it has and
  whether the person is verified — until now those were a coloured dot and a
  bare number, invisible to a reader.

### Changed

- **Long conversations no longer redraw constantly.** An open thread was
  re-rendering every message several times a second for as long as the app was
  running. Nothing looks different; it just stops costing battery.

- **Connections are now signed with Ed25519** where both sides support it,
  falling back to the previous RSA signature for older peers. Handshakes carry
  64-byte signatures instead of 256 and verify faster. **Your safety number is
  unchanged** — contacts you have already verified stay verified, and nobody
  needs to re-check anything.

### Fixed

- **A private channel's messages reached every connected member.** They were
  correctly kept out of the channel list and refused its history, but each
  message posted still arrived at their client as it was sent. Restricted
  channels are now delivered only to the people allowed to read them, and the
  fix is covered end to end rather than only in the layer that decides it.
- **A community could quietly stop updating until you reconnected.** If a
  message arrived at the same moment you were sending one, the connection could
  desynchronise and go silent — new channels never appeared, messages stopped
  coming in, and nothing said anything was wrong. The busier the community, the
  more likely it was.
- **Adding someone from an invite link in "New connection" never connected.**
  It saved the contact and then reported "Saved undefined to contacts" instead
  of dialling them. Pasting the same link into the Contacts pane always worked;
  this path did not.
- **A file permission you turned off could turn itself back on.** Removing
  someone's ability to see a file did nothing if they could still share it on —
  the switch moved back and the only way through was to guess the right order.
- **The Contacts pane could come up blank** instead of showing your contacts.

### Security

- **A wrong connection password no longer reveals its length.** The check
  answered a wrong-length guess faster than a wrong-character one, which tells
  an attacker how long the password is before they start guessing it.
- **A peer can no longer grow the app's memory without bound** by connecting
  repeatedly from changing addresses. The list of recent connections is now
  capped, and the addresses knocking hardest are the last to be forgotten.
- **One member can no longer flood a community with channels.** Creating them is
  limited per person, so a single member cannot fill the server's channel list
  and leave everyone else unable to make one.
- **The desktop app declares a content security policy in the page itself**, not
  only in the shell that loads it, so it holds wherever the page is opened.

## [1.16.0] - 2026-08-19

> **Breaking for Communities.** `MemberInfo` gained a role and `ChannelInfo` a
> member list, so community clients and servers must ship together. Anyone
> self-hosting a community server has to restart it on the matching version.

### Added

- **Roles on community servers.** Guest, Member, Admin and Owner, enforced by
  the server. The first identity to join owns the server — the operator starts
  it and then joins it, which is the only bootstrap that does not need an admin
  to already exist. A role can only ever be granted below your own and the owner
  can never be demoted, so an admin cannot take a community from the person
  running it. Admins get an inline role picker in the member list.
- **Channel kinds that actually do something.** Private channels are limited to
  the members you pick and are not even listed to anyone else; locked and
  announce channels are readable by everyone and writable by admins. A new
  dialog creates channels of a chosen kind and changes an existing channel's
  access; admins can delete channels.
- **A Drive panel** listing every file shared in a community you can see, with
  who shared it, where, when, and how large, plus a bar showing how much of your
  storage allowance you have used.
- **Deleting shared files.** Both the person who shared a file and an admin can
  remove it; the storage it occupied is genuinely reclaimed once nothing else
  references it. Deleting a channel releases the files shared in it.
- **Storage allowances.** Each member gets 128 MiB by default alongside the
  existing 1 GiB server-wide ceiling, because a single ceiling let whoever
  filled it first deny file sharing to everyone else. Admins are exempt. Sharing
  one file into several channels counts once.
- **An activity log** for admins, recording role changes, channel changes and
  file deletions.
- **Large files in communities.** Sharing was capped at 4 MiB — the most that
  fits in one message — and anything bigger was simply refused. Files up to
  100 MiB now stream in chunks. The server is told the size first and can refuse
  on space, allowance or permission before a byte moves, so a file that will be
  rejected costs a moment instead of the whole upload. Nothing changes in the
  UI: picking a big file just works now.
- **A Communities pane in the terminal client** (`:party`), showing each
  community's channels and their access rule, its members with roles and
  presence, the files you can see, and your storage use. With commands to match:
  `:party-files`, `:party-role`, `:party-channel-access`,
  `:party-delete-channel`, `:party-delete-file` and `:party-audit`.

### Fixed

- **A private channel's messages were sent to everyone.** Members who were
  correctly denied the channel — it did not appear in their channel list, and
  its history was refused — still received every message posted to it as it
  arrived. Restricted channels are now delivered only to the people allowed to
  read them.
- **A community that predates roles could not be administered.** Upgrading left
  every existing member an ordinary member, and since only an admin can promote
  anyone, the operator was locked out of their own server's governance
  permanently. Such a server now appoints an owner on first load and records it
  in the activity log.
- **Sharing the same file twice in one place leaked storage.** Only one of the
  two shares survived a restart, while the file still counted both, so deleting
  the visible one could never free the space.

- **The member list is live again.** Nothing announced a join or a disconnect,
  so your view of who was in a community was frozen at the moment *you* joined:
  people who arrived later never appeared, and the online dots never moved.
- **Closing one window no longer shows you as offline** when you have the
  community open somewhere else.
- **A direct message sent from one device now appears on your others.** Channel
  messages always did; DMs were delivered only to the recipient, so the copy on
  your second device never arrived.
- **A file the server failed to store is no longer offered as if it worked.**
  The write error was logged and swallowed, so the upload was acknowledged, the
  file appeared in the channel for everyone, and every attempt to download it
  answered "unknown file" — permanently.
- **Presence and role changes reach the screen.** The desktop app decided
  whether the Communities view had changed by counting members and channels, so
  anything that changed a member without changing the count — going offline,
  being promoted — never triggered a refresh.
- **Saving a file from the Drive panel** proposed `undefined` as its name.
- **Returning from the Drive or activity log** left the conversation scrolled up
  instead of at the newest message.
- The role picker offered choices the server refuses (an admin could select
  `admin` for another admin); it now offers only what will be accepted.

### Security

- **`h2` updated for RUSTSEC-2026-0258** — before 0.4.16 it queues empty DATA
  frames without limit, which can exhaust memory or panic. It reaches the app
  through UPnP NAT traversal and through Tauri's HTTP stack.

### Documentation

- `docs/protocol.md` now documents the Party wire contract: framing and replay
  protection, the authorization model, the request/response sets, history
  paging, and the file rules.

## [1.15.0] - 2026-08-01

> **Breaking, and the point of the release: P2PEM ships one desktop app.**
> The egui GUI is gone. Releases now lead with P2PEM Desktop (Tauri + React)
> installers and a clearly-secondary tools archive, instead of two similar-looking
> desktop apps with nothing saying which to install. The desktop app uses its own
> data directory, so it does **not** inherit an identity from the retired GUI —
> export a backup from the old app first if you want to keep that identity.

### Removed

- **The egui/eframe desktop GUI** (`client/src/gui/`), along with the
  `eframe`, `egui`, `egui_commonmark`, and `egui_tracing` dependencies and the
  now-unused `rfd`, `emojis`, and `windows-sys` entries. `cargo tree` shows no
  egui anywhere in the workspace, which also removes the egui-winit/wayland half
  of the Linux dependency and advisory surface.
- The hand-rolled Windows packaging (`setup.iss`, `build-and-package.ps1`),
  which hardcoded the retired binary.

### Added

- **Unread counts survive a restart.** `Chat::read_count` is persisted inside
  the encrypted history, so a message that arrived while the app was closed is
  still unread on the next launch. Previously the desktop app seeded "already
  seen" from the current message totals on first load, quietly marking
  everything you were away for as read.
- **A boot screen that can explain itself.** A slow or failed startup used to
  render a permanently blank window with no message and no recovery. It now
  shows a spinner, then the actual error with a retry button, and retries
  automatically with backoff.
- **First-run identity backup.** After creating an identity the app offers to
  save the (already encrypted) backup file, with the reason attached. Settings
  now states whether a backup has ever been made and when.
- **`P2PEM Tools` release artifact** — the terminal client and community server
  in one archive per OS, for self-hosters.
- **Design-token drift guards** replacing the one lost with the egui theming
  module: `desktop/src/lib/tokens.test.js` checks the shipped CSS against
  `design/tokens.json`, and a Rust test checks the TUI brand accent and the
  frozen safety-grid palette.
- **A get-started panel on first run.** With no conversations, the chat pane
  used to say "Select a conversation" — with nothing to select. It now explains
  why the first connection needs a deliberate step (no accounts, no directory)
  and offers the three real paths, each opening the connection dialog on the
  right tab, plus what the six-digit verification code is for.
- **A supply-chain CI gate** (`cargo deny --all-features check` + a secret
  scan), so the accepted-risk list cannot quietly go stale.
- Dialogs keep Tab inside them and hand focus back to whatever opened them.
  `aria-modal` told assistive tech the page behind was inert but did nothing to
  stop Tab walking into it, so a keyboard user could end up typing into
  something hidden behind the scrim.

### Changed

- **The release page tells you what to download.** The workflow composes a body
  with a per-OS table before any artifact uploads.
- **History writes are halved.** Every mutating command saved immediately, then
  the poll loop saw a signature it had not recorded and wrote the identical
  bytes again — two full, fsynced, O(total-history) rewrites per user action.
  The save marker is now shared between them. On top of that the encrypted
  history is serialized compactly instead of pretty-printed; the indentation was
  never read by anyone (the bytes are encrypted immediately) but was paid for on
  every rewrite. The whole file is still re-encrypted whenever the conversation
  surface changes — that cost is inherent to the current format and is now
  documented as a known limit rather than left implicit.
- **Password floor raised to 12 characters**, enforced in `Identity::encrypt` so
  no front-end can set a weaker one — the screen used to coach "12+" while
  accepting four. `decrypt` is deliberately unaffected, so existing identities
  still open. The set-password screen now reads the floor from the backend and
  names the specific problem instead of silently doing nothing.
- **The client binary is the terminal client.** `--gui` exits with a pointer to
  the desktop download rather than starting a different interface than asked
  for. It is a console-subsystem binary now, so the Windows console juggling is
  gone.
- Desktop notifications are titled with the conversation name, and an incoming
  file now raises one too.
- The identity avatar in the rail opens Settings (it was a button with no
  action).

### Fixed

- **An interrupted write can no longer destroy your identity and all your
  history.** `identity.json` was written in place (truncate, then write), and any
  failure to *read* it back — including the empty file a crash or power loss
  could leave — was treated as a reason to silently generate a brand new
  identity. Because the history key is derived from the private key, that one
  cascade produced: unreadable message history, a changed fingerprint for every
  contact who had verified you, and an app that looked freshly installed. Now
  every identity write is atomic and `fsync`ed (temp file, same directory,
  rename, `0600` from creation), and an identity file that exists but cannot be
  read is a hard, explained error that leaves the file intact for restoration
  from a backup. The encrypted history uses the same atomic write, which
  previously renamed without ever flushing.
- **A failed migration no longer leaves your messages readable on disk.**
  Migrating a legacy plaintext history deleted the old file and, if the delete
  failed, logged a warning and carried on — leaving every message in the clear
  with no indication. It now truncates the file to remove the content when it
  cannot be unlinked, and raises a visible error naming the file if even that
  fails.
- **Loading history replaces state instead of merging into it**, so a deleted
  chat cannot reappear after a reload. Live host placeholders are preserved.
- **The terminal client honours the notification focus gate too.** It reports
  presence based on recent keyboard interaction, since a terminal cannot report
  window focus portably; going idle restores notifications for every
  conversation.
- **Community unread counts survive a restart**, the same defect as the
  direct-message counts and fixed the same way: read marks are remembered rather
  than re-seeded from whatever the server replayed on rejoin.
- Dangerous-path checks on a loaded config compare **path components** instead of
  substrings, so `/usrdata` and a directory named `my..files` are no longer
  rejected, while `/var`, `/root`, `/boot`, `/proc`, `/sys`, `/dev` and
  `ProgramData` now are.
- A panic elsewhere in the process can no longer freeze the desktop app: the
  identity mutex recovers from poisoning instead of panicking on every
  subsequent access, which previously killed the event loop outright.
- **The fingerprint prompt can no longer be dismissed without answering it.**
  Escape or a click outside cleared it while a live session sat waiting on the
  decision — the peer hung with no explanation and the prompt reappeared on the
  next poll, reading as a bug. Verify and Reject are now the only ways out, with
  a quiet escape hatch that appears only if confirming actually fails (the
  session went away), and closing that trusts nothing. Reject is also no longer
  styled as an afterthought: refusing a connection you could not verify is the
  correct action, not an edge case.
- **Desktop notifications no longer fire for the conversation you are reading.**
  The setting promises "notify when a message arrives in the background", but
  every received message raised a popup regardless of focus. The shell now
  reports focus + open conversation to `ChatManager` via `set_ui_presence`, and
  `should_notify_for` gates on it.

### Documentation

- **Every accepted advisory now names the path that reaches it**, verified with
  `cargo tree --target all -i` rather than asserted. The `quick-xml` entry in
  particular was wrong: the vulnerable version is reached only through
  `rfd → wayland-scanner`, a build-time proc macro parsing XML vendored inside
  the crate, never linked into the shipped binary — not "transitively via
  Tauri", whose own path already resolves the fixed release. Stale entries left
  over from the egui stack (`epaint` fonts, `ttf-parser`, the `image` AVIF
  chain) were removed rather than left to suppress future findings.
- `SECURITY.md` no longer claims the desktop crate has no automated tests (it
  has 16, run by CI on Linux and macOS), states plainly that the "medium"
  posture is a **self-assessment** rather than an audit, and lists the lack of
  offline delivery for direct P2P as a real limitation.
- `docs/platform_spec.md` no longer says the Tauri binary is absent from the
  release pipeline; phases E and F are marked done, and §14 now lists the honest
  remaining gaps (no mobile client, no offline P2P delivery, self-hosting
  required for WAN, no third-party audit) in order of cost.
- README rewritten around one download, with an explicit "honest limitations"
  section. User guide, tutorial, architecture, developer guide, and `CLAUDE.md`
  updated to the single-app world.

## [1.14.0] - 2026-07-24

### Added

- **File messages are no longer dead ends (desktop).** A finished file card
  can be clicked to open the file with its default app, and a folder button
  reveals it in Explorer/Finder/file manager. The webview only ever passes
  (chat id, message id) across the bridge — never filesystem paths.
- **Inline image previews (desktop).** Received and sent images (PNG, JPEG,
  GIF, WebP, BMP up to 4 MiB) render as a thumbnail right in the chat; click
  the thumbnail to open the full image. SVG is deliberately excluded (it can
  carry scripts).
- **Links in messages are clickable (desktop).** http(s) URLs in message
  text open in the system browser. Only web URLs launch — the scheme is
  whitelisted at the bridge, so a peer cannot make a message open `file:` or
  app-scheme links.
- **Time you can actually see (desktop).** Threads now show day separators
  ("Today", "Yesterday", full date), file cards show their time, and the
  conversation list shows when the last message arrived (time today, weekday
  within a week, date otherwise).

### Security

- **Unlock no longer reveals how the identity file is protected.**
  `Identity::decrypt` used to try up to three Argon2 configurations in turn —
  the parameters recorded in the file, then two older defaults — so an unlock
  cost one, two or three ~1 s derivations depending on how the file happened to
  be written. Anyone able to time the unlock locally learned which
  configuration protects the private key, which narrows an offline attack on a
  stolen identity file. Recorded parameters are now the only ones tried: one
  derivation, and a failure is a failure. Files written before the parameters
  were recorded still open (the two historical schemes are tried for them
  alone), and that costs nothing in secrecy — the absence of the field is
  already visible in the file. Closes #33.

- **Identity files can no longer demand unbounded work at unlock.** Argon2
  cost parameters read from `identity.json` are now checked against
  application limits (1 GiB memory, t≤16, p≤16) before use. The RFC's own
  limits run to terabytes, so a corrupted or hostile file could previously
  make unlock allocate gigabytes — and with recorded parameters now
  authoritative, nothing else would have caught it. The bounds sit far above
  what the app writes (64 MiB, t=3, p=4).

## [1.13.0] - 2026-07-21

### Added

- **Real TCP hole punching over the relay.** The relay rendezvous now
  coordinates a genuine direct connection between two peers instead of only
  forwarding their traffic. When both sides are punch-capable, the server
  hands each the other's observed public endpoint plus LAN candidates and the
  peers perform a TCP simultaneous open (reused source ports, token-tag
  validated, host-led socket selection — `core/src/network/punch.rs`); the
  relay then carries no session bytes at all. It falls back to bridged
  forwarding only when punching fails (symmetric NAT, CGNAT, filtered
  networks). UIs label a punched session `p2p:<addr>` (Direct badge) vs
  `relay:<server>`. The control protocol is append-only, so new clients and
  servers stay wire-compatible with pre-punch peers in both directions;
  `P2PEM_NO_HOLEPUNCH=1` forces the bridged path.

- **Short Authentication String (SAS) verification.** Peer verification now
  leads with a six-digit + three-emoji code derived from the handshake
  transcript (`derive_sas`), which both peers compute identically — an active
  MITM's two handshakes yield two different codes. Users read the short code
  aloud instead of comparing a 64-character fingerprint; the full
  fingerprint / safety grid is demoted to an "advanced" section. Surfaced in
  the egui dialog, the TUI overlay, and the desktop Verify panel.

- **Delivery receipts.** New backward-compatible `Ack` protocol frame:
  receiving a text (or finalizing a file) acknowledges the sender, whose
  message gains a ✓ in all three UIs (`Message.delivered`, persisted). Peers
  that predate the frame drop it harmlessly. See `docs/protocol.md`
  § Delivery receipts.

- **File-transfer cancellation.** Either side can abort an in-flight transfer
  via a new replay-protected `FileCancel` wire frame. Sends stream from a
  background task (a large send no longer freezes the app) over a bounded
  outbound lane, so a slow peer paces the disk reader with real backpressure
  instead of buffering the whole file in memory; the send stops promptly on
  cancel and the receiver discards its partial file. Cancellable from the
  egui transfer bar, the TUI Transfers overlay (↑/↓ select, `c` to cancel),
  and the desktop transfer cards, in both directions.

- **Incoming files now require acceptance.** The `auto_accept_files` setting
  (default off) is finally enforced: an incoming file is held (spooled to a
  temp file) until accepted — Accept/Decline in the egui transfer row, the
  desktop transfer bar, and new TUI `:accept`/`:decline` commands. Declining
  deletes the spool. The toggle is now exposed in the desktop Settings too.

- **Contact trust lifecycle.** Accepting a TOFU prompt promotes the matching
  contact to Verified, and contacts can now be blocked/unblocked (egui
  contacts window, desktop contact cards): a blocked contact's connections
  are auto-refused, its live sessions torn down, and outbound dialing
  refused. The desktop app also gains contact deletion.

- **Editable display name and identity backup.** Your display name can be
  changed (Settings in egui and desktop, `:name` in the TUI) — invite links
  stop advertising everyone as "User" — and every UI can export a backup copy
  of the encrypted `identity.json` (`:backup <path>` in the TUI).

- **Multi-address invites (payload v4).** Signed invites can carry every
  reachable candidate address in priority order — the UPnP external address
  first, the LAN address second. A connecting peer tries each in turn with a
  bounded 10-second per-attempt timeout (a warning toast marks each
  fallback), so the same invite works from both the internet and the local
  network. Fully backward compatible in both directions: pre-v4 clients read
  the primary `address` field, and v4 clients keep verifying old invites (the
  new field is omitted from the signed bytes unless it carries ≥ 2 entries,
  mirrored on both the signing and verifying side). Contacts store the
  candidate list (`Contact.addresses`; old history files load unchanged).

- **NAT traversal (opt-in).** With the new "UPnP port mapping" setting
  enabled, hosting asks the router to forward the listening port and
  discovers the external IP; generated invites then carry the
  internet-reachable address instead of the LAN one. UPnP/IGD is tried first,
  then NAT-PMP (RFC 6886). The mapping is renewed automatically and removed
  when hosting stops; carrier-grade / double NAT (a private "external" IP) is
  detected and reported so the user falls back to a relay. Off by default —
  enabling it opens a router port and embeds the public IP in shared invites.
  Exposed in all three UIs (egui Settings, desktop Settings, TUI
  `:set upnp on|off`).

- **Signed invites now expire.** An invite older than 30 days is rejected at
  import with a clear error; the timestamp is covered by the signature, so it
  cannot be back- or forward-dated without breaking verification
  (future-dated invites beyond a 1-hour clock-skew allowance are rejected
  too). Legacy v1 unsigned invites carry no timestamp and still import with
  the existing warning.

- **Desktop P2P parity.** The Tauri app catches up with the egui app on every
  networking front: optional connection passwords on direct Host/Connect,
  auto-rehost after an accepted connection consumes the listener,
  auto-reconnect of saved contacts on unlock, the address to share shown
  right after "Start hosting" (LAN + UPnP external once resolved), and LAN
  peer discovery (mDNS browse + advertise, gated behind the new
  `enable_mdns` toggle, off by default) with a "Nearby peers" list in the
  connect pane.

- **Desktop conversation lock, diagnostics, and in-app help.** A titlebar
  toggle refuses new incoming peers (stops the listener, unregisters the
  mDNS service, pauses auto-rehost) like the egui menu toggle; Settings gains
  a Support section (diagnostics bundle export + open data folder) and a
  "How P2PEM works" explainer covering connecting, the SAS verification
  ritual, and delivery checkmarks.

- **File-transfer progress in every UI.** The egui GUI shows a live progress
  bar above the chat input and the TUI shows a percentage in the message-view
  title, matching the transfer bar the desktop app already had.

- **New logo and brand identity.** A speech-bubble + linked-dots mark in a
  teal-to-indigo gradient replaces the RSA-era icon everywhere: the Tauri
  icon trees (Windows/macOS/iOS/Android), the installer icon
  (`encodeur_rsa_icon.ico` → `app-icon.ico`), the egui window icon (new — the
  window previously had none), the web favicon, and the GitHub social
  preview. macOS bundles now declare `CFBundleIconFile` and ship a proper
  `.icns`.

- **Rose theme** joins Light/Dark/Midnight/Forest in the egui theme picker
  and the TUI (`:set theme rose`), and **canonical design tokens**
  (`design/tokens.json`) become the source of record for brand and theme
  colors, enforced by a test so the token file and the UI cannot drift apart.

- **First automated tests for the desktop frontend.** Vitest covers the pure
  logic modules (`colorgrid`, `partyUnread`, `themes`); `npm test` runs in
  CI's new Frontend Build job.

- **Bridge integration tests for the desktop app.** A new suite
  (`desktop/src-tauri/src/tests.rs`) drives the real Tauri command handlers
  over the mock runtime's IPC — the same path the webview uses — covering the
  auth barrier (no command runs before unlock/set-password), the invoke-key
  contract with `bridge.js` (an argument-name mismatch now fails CI instead
  of silently becoming a no-op in production), settings round-trips,
  display-name validation, conversation-lock semantics, TOFU confirmation
  guards, and the
  invite-link round-trip. The command registration moved into a shared
  `invoke_handler()` so tests and the shipping app can never register
  different command lists.

### Changed

- **Client crate renamed `encodeur_rsa_rust` → `p2pem-classic`.** The last
  RSA-era name is gone: the package, library, and binary are now
  `p2pem-classic` (matching the release artifacts), so a task manager shows
  `p2pem-classic` instead of `encodeur_rsa_rust`. Build commands change
  accordingly (`cargo build -p p2pem-classic`); data directories are
  unchanged, and `RUST_LOG` filters targeting `encodeur_rsa_rust=` must
  switch to `p2pem_classic=`.

- **Release assets renamed to one consistent scheme.** The classic egui
  artifacts drop the mixed `Messenger-Setup-v*` / `messenger-*` naming for
  `P2PEM-Classic_<version>_<platform>-<arch>.<ext>`, matching the Tauri app's
  `P2PEM_<version>_*` convention on the same release page. The versionless
  `P2PEM_*.app.tar.gz` archives are no longer published (the `.dmg` covers
  macOS).

- **The desktop bridge only refreshes the webview on real changes.** The poll
  loop computes signatures of the conversation and Communities surfaces and
  emits `state-updated` / `party-updated` only when they change, instead of
  four times a second unconditionally; transfer progress still ticks live and
  a TOFU prompt always forces a refresh.

- **CI now builds the React frontend** (`npm ci` + `npm run build` in
  `desktop/`); previously a broken frontend could pass CI because the
  committed `desktop/dist/` masked it.

- **Documentation restructured.** Numbered docs renamed
  (`docs/03_architecture.md` → `docs/architecture.md`, `04_protocol` →
  `protocol`, `05_platform_spec` → `platform_spec`); the superseded
  UI-redesign spec under `docs/superpowers/` deleted (its content lives in
  `docs/platform_spec.md` §10); CONTRIBUTING.md owns the contribution process
  and DEVELOPER_GUIDE.md the technical guide without duplication; SECURITY.md
  gains a Supported Versions table and points to GitHub private vulnerability
  reporting; `docs/README.md` documents the product-naming map.

- **Accent colors converged across the three UIs.** egui's Dark and Light
  themes used two different blues by accident; both now use the brand accent,
  Midnight/Forest were realigned to the desktop app's exact hues, and the
  TUI's theme-neutral chrome uses the brand accent instead of terminal cyan
  (semantic colors untouched).

- **Big-file internals split for maintainability** (pure refactors, no API or
  behavior change): the 3,200-line `client/src/app/chat_manager.rs` became
  `client/src/app/chat_manager/` with one module per concern (`connect`,
  `contacts`, `events`, `files`, `invites`, `text`, `tests`), and the desktop
  bridge's command handlers moved from `lib.rs` into `src/commands/` grouped
  by concern (`auth`, `chats`, `connect`, `contacts`, `party`).

### Fixed

- **"File sent" is now confirmed at the wire, not at the queue.** Success was
  reported as soon as a file's frames were queued on the session — the
  transfer could still be in flight (or die with the connection) while the
  sender saw "File sent". The session now reports when the final frame is
  actually written to the socket; only then does the success toast appear,
  and a disconnect with sends still pending shows an honest "File may not
  have been delivered" error.

- **Stuck outgoing transfer on a local file error.** If the file was deleted,
  moved, or became unreadable after a send started, the streaming task told
  the peer to cancel but never updated local state — the transfer row sat at
  "in progress" forever and its handle leaked. It is now marked `Failed`
  (with a toast) and cleaned up.

- **Relay joiner no longer misreads a lost host as a legacy server.** A
  punch-capable joiner whose connection dropped before the first response
  (e.g. the host vanished at the rendezvous) was mistaken for a pre-punch
  relay and pointlessly retried in legacy mode. New relays acknowledge a join
  immediately, so only a genuine legacy server (which stays silent) triggers
  the fallback.

- **Workspace build restored on the declared toolchain.** Pinned `rusqlite`
  to 0.39 — 0.40 pulls `libsqlite3-sys` 0.38, whose build script uses the
  unstable `cfg_select` macro and broke `messenger-server` (and any
  `--workspace` build) on the supported Rust version. Also refreshed
  dependencies in-semver, dropping the yanked `spin`/`core2` pins and picking
  up the `memmap2` unsoundness fix.

### Security

- **X25519 key-exchange hardening.** The handshake rejects an all-zero peer
  public key on parse and a non-contributory (low-order-point) shared secret
  at agreement, so a peer cannot force a predictable session key
  (RFC 7748 §6.1).

- **Remote out-of-memory crash in large-text reassembly fixed.** A peer could
  send a single chunked-text frame declaring a huge `total_chunks`, making
  the receiver pre-allocate gigabytes and abort. The chunk count is now
  capped before any allocation (symmetrically on send), with a bound on
  concurrent partial messages per chat.

- **Session rekey no longer drops active conversations.** A key rotation
  could desync the two peers and tear the connection down — either because
  both sides rotated in the same round trip, or because frames sent under the
  old key were still in flight when the initiator retired it. Rekeying is now
  initiated by a single deterministic side (the host), and the receiver keeps
  the previous key for a bounded window (dropped as soon as a frame decrypts
  under the new key). The host also rekeys on its keep-alive tick, so it
  rotates on schedule even when only receiving.

- **RSA decryption removed from the codebase.** The product never decrypted
  with RSA on the wire (X25519 does key agreement; RSA is signatures only),
  so the unused RSA-OAEP encrypt/decrypt functions were deleted — keeping the
  operation targeted by the `rsa` timing advisory (`RUSTSEC-2023-0071`) out
  of the product entirely. Known cryptographic-design limits (no
  post-compromise security / double ratchet; TOFU without key transparency)
  are now documented explicitly in `SECURITY.md`.

### Removed

- **Group chats.** The feature was an illusion (local-only fan-out over 1:1
  sessions; recipients saw a plain DM, offline members lost messages) and is
  removed until a real wire-level design lands. Old histories containing
  legacy group chats still load and display.

- **Dead code:** the never-constructed `MessageContent::Edited` variant and
  the no-op `NotificationSound` setting.

## [1.12.1] - 2026-07-07

### Fixed

- **Idle sessions no longer disconnect after 5 minutes.** Nothing ever sent
  keep-alives, so both peers' receive-idle timers (300 s) tore down any
  healthy-but-quiet conversation. The transport now sends an encrypted
  keep-alive ping every 120 s (consumed silently on receipt, sharing the
  replay-protected sequence space). Regression-tested with shrunken test
  windows.
- **Received files now land in your real Downloads folder.** The default
  download directory was the *relative* path `Downloads`, resolved against
  the process working directory — files were saved next to wherever the app
  was launched from (or failed where that wasn't writable). The default now
  resolves the OS Downloads folder, the temp-dir default moved under the OS
  temp dir, and configs saved by older builds are upgraded on load.
- **Honest error after a peer disconnects.** Sending a message in an
  established conversation whose session dropped showed "Connecting... please
  wait" and silently dropped the message; it now says the message was not
  delivered because the peer is disconnected.
- **v1.12.0 release: desktop installers failed to build** — tauri-action
  invokes `npm run tauri build`, and `package.json` had no `tauri` script.
  The four classic binaries published fine; the installers ship with the next
  release.

## [1.12.0] - 2026-07-05

### Added

- **Community file sharing in the desktop app.** Share a file into a channel
  or DM (paperclip button) and download files others shared (click a file
  message → native save dialog). This wires up the client half of the Party
  file feature that had a complete server but no client: `PartyManager`
  gained `send_file` / `send_file_dm` (optimistic append + inline upload,
  size-checked against the 4 MiB `MAX_INLINE_FILE_BYTES`) and
  `request_download` (correlates the async `FileData` response by content
  hash); the bridge added the matching commands. Downloads remain
  access-checked server-side.
- **Community file sharing from the TUI.** `:party-send-file <path>` shares
  into the current channel and `:party-download <name|hash>` saves a shared
  file into the download folder; downloads never overwrite existing files
  (`name (2).ext`).
- **Community lifecycle in the desktop app.** Joined communities are
  remembered across restarts (`parties.json` — address, username, server
  name; never the password) and offered as one-click rejoin cards; you can
  leave a community (with a confirm step), join more than one (a `+` tab in
  the switcher), and recover from a lost connection via a Rejoin / Remove
  banner. Rejoining replaces the old entry (deduplicated by address).
- **Community server identity pinning (TOFU).** The first join pins the
  server's fingerprint; a later join to the same address presenting a
  different identity is refused with a clear security warning. Leaving a
  community clears its pin (the documented way to accept a legitimately
  redeployed server).
- **Unread badges in the desktop app** — for conversations (list + Chats
  rail) and for Communities (channels, DM threads, switcher, rail icon),
  including messages that arrive while another tab is open. Pre-existing
  history is never counted as unread. The Communities rail label is now
  "Communities" everywhere (was mixed "Parties"/"Party").
- **Real settings in the desktop app.** The Settings pane exposes the
  settings the runtime actually honors: desktop notifications,
  typing-indicator privacy, auto-host on startup with a configurable
  listening port, and the download folder (native picker). Changes save
  immediately into the encrypted history file. The bridge honors auto-host
  on unlock, like the egui/TUI apps.
- **Live file-transfer progress in the desktop app.** In-flight sends and
  receives show a progress bar (filename, live percentage) above the
  composer, and failures surface their reason inline (new `list_transfers`
  bridge command).
- **A real server CLI.** `messenger-server` takes `--name`, `--port`,
  `--password`, and `--data-dir` (with `--help`/`--version`), so hosting a
  community no longer requires environment variables, the display name is
  configurable (was hardcoded), and the port is no longer fixed at 12345.
  The old `PARTY_*` environment variables still work as fallbacks; hosting
  is documented in the user guide.
- **Desktop installers in releases.** Tagged releases build and attach the
  Tauri app's native installers (Windows MSI + NSIS, macOS DMGs for Intel and
  Apple Silicon, Linux deb/AppImage) alongside the classic egui binaries. The
  Tauri CLI is a devDependency (`npx tauri dev` works on a fresh clone), and
  the bundle version tracks the workspace version.

### Security

- **Community file names are sanitized server-side.** A member-chosen name
  like `..\..\Startup\evil.exe` is reduced to a safe filename at upload (the
  single choke point for channel and DM files), so no client can be handed a
  name that escapes its download directory. P2P transfers already had this at
  protocol decode; TUI downloads re-sanitize on save (defense in depth).
- **Party input length caps.** The server bounds member usernames (≤ 32
  chars), channel names (≤ 64 chars), and channel/DM text (≤ 64 KiB, matching
  the P2P transport cap) instead of accepting any string up to the 8 MiB
  packet limit. The desktop UI mirrors the caps for immediate feedback; the
  server remains authoritative.

### Performance

- **Desktop app bundle cut by 70%** (886 KB → 266 KB, gzip 231 KB → 81 KB):
  the icon component's namespace import defeated tree-shaking and shipped the
  entire lucide icon library (~1500 icons) for the ~45 actually used.
- **Long threads render a bounded window.** Chats and community threads mount
  only the most recent 150 messages ("Show earlier messages" widens the
  window), so a multi-thousand-message history no longer re-renders thousands
  of nodes on every poll tick.

### Documentation

- Brought the docs back in sync with the code: the four-crate workspace, the
  shipped Tauri + React desktop app (the plan docs still described an
  unshipped SolidJS rewrite), automatic session-key rotation and text
  chunking in the protocol reference, mDNS LAN discovery, Party file download
  access control, corrected pre-workspace `src/` paths, and the current test
  count. Added a webview/IPC/CSP section to `SECURITY.md`, Party-operator and
  desktop surfaces to `THREAT_MODEL.md`, and a 2026 findings entry to
  `docs/AUDITS.md`.

## [1.11.1] - 2026-06-29

### Fixed

- **Release pipeline produces binaries again.** The `v1.11.0` tag shipped
  with a stale `Cargo.lock`, so every platform job failed at
  `cargo build --locked` and no installers/archives were attached. The
  lockfile is now kept in sync with the workspace version.

### Dependencies

- Rolled up the outstanding Dependabot updates: the grouped `rust-minor`
  updates, `rfd` 0.14 → 0.17, `rusqlite` 0.32 → 0.40, `mdns-sd` 0.11 → 0.20,
  `emojis` 0.6 → 0.9, and the desktop frontend's `react`/`react-dom` 18 → 19,
  `vite` 6 → 8, `@vitejs/plugin-react` 4 → 6, and `lucide-react`.

## [1.11.0] - 2026-06-29

### Added

- **File sharing in Party servers (Phase 2, slice 1).** Members can share
  files (up to 4 MiB inline) in channels and direct messages. The server
  stores each file once, content-addressed by SHA-256 and reference-counted,
  with bytes on disk and metadata in the SQLite store; a file appears in
  history like a message and can be fetched by hash. Larger-file chunking and
  the Drive UI are still to come.

### Changed

- **TUI command polish.** `:help <command>` opens focused command help, the
  Party channel command is canonicalized as `:party-create-channel` (old
  spelling kept as an alias), bare IPv6 connect targets are no longer
  misparsed as `host:port`, and `--tui --connect-relay` reports a missing
  `--relay-token` instead of silently doing nothing.

## [1.10.1] - 2026-06-28

### Changed

- **Party server durability moved to embedded SQLite.** The server mirrors
  its state (members, channels, message + DM history) to a `party.db`
  database under the operator's data dir, writing each change incrementally
  instead of rewriting a whole JSON snapshot on every message. An existing
  `party_state.json` snapshot is imported once on first start. No operator
  configuration change; runtime behavior unchanged.

### Fixed

- CI now builds the whole workspace: installs the WebKitGTK system
  dependencies the Tauri crate needs, and applies `rustfmt` to that crate
  (both were missing after it joined the workspace).

## [1.10.0] - 2026-06-28

### Added

- **Tauri 2 desktop app (new `p2pem-desktop` crate).** A native desktop shell
  wrapping a React/Vite web UI (`desktop/src/`), driving the same
  `ChatManager` core as the egui/TUI front-ends through a `#[tauri::command]`
  bridge. Includes onboarding, conversations, contacts, invite
  import/export, fingerprint verification with the safety-color grid, a
  relays pane, settings, and toasts. Run with `cd desktop && npx tauri dev`.
- **Conversation model.** Every `Chat` carries `kind: ChatKind`
  (`Dm`/`Group`/`Channel`) and `transport: Transport`
  (`Direct`/`Relay`/`Server`), both `#[serde(default)]` for back-compat with
  existing history files; front-ends use them for badges.
- **Party server-routed direct messages (DMs)**, routed and persisted by the
  hub; surfaced in the GUI Party window and the TUI.
- **Party channel creation and management** from the client.

### Changed

- **The workspace now has four members** — `core`, `client`, `server`, and
  `desktop/src-tauri`. Bare `cargo` commands still target the client.
- Consolidated the planning/spec docs into a single canonical platform spec
  and refreshed architecture, protocol, README, and contributor guides.

## [1.9.0] - 2026-06-01

### Added

- **Party server (new `messenger-server` binary).** A self-hosted, multi-user
  server (the "Administered" trust tier): join with an address + optional
  password + a username — no port-forwarding required. Members appear in a
  directory with presence, chat in channels with live broadcast, and the
  server stores history so offline members catch up on reconnect. The
  encrypted, authenticated transport reuses the Protocol v3 handshake (the
  server has its own TOFU-verified identity), with durable state and a stable
  server identity across restarts.
- **Party client UI** in the GUI (join form, server/channel/member lists,
  message view, post box) and the TUI (`:party-connect`, `:party-post`,
  `:party-status`).
- **P2P connection password + conversation lock.** Hosts can require an
  optional shared password (verified inside the encrypted v3 tunnel, after
  identity verification, with a constant-time comparison) and can lock a
  conversation to refuse new connections. GUI (Host/Connect dialogs + a
  menu-bar lock toggle) and TUI (`:connection-password`, `:lock`).
- **TUI overhaul**: a typed command language exposing every action
  (connections, contacts, invites, file send, settings, identity,
  diagnostics) with live autocomplete; modal overlays for fingerprint
  verification, password unlock/setup, contacts, settings, identity (with
  the safety-color grid), transfers, and help; plus auto-scrolling,
  cursor-aware editing, command history, toasts, unread/typing indicators,
  and graceful encrypted-history save on a timer and on exit.

### Changed

- **Workspace restructure.** The single crate became a three-crate workspace
  — `core` (crypto, protocol, identity, transport, shared types), `client`
  (the GUI/TUI app), `server` — so client and server share `core`. No
  behavior change; the v3 handshake was extracted into a reusable form so
  P2P sessions and the Party server run the exact same audited code.
- Test coverage expanded substantially (250+ workspace tests), including
  end-to-end session, relay, and Party server tests, full protocol
  round-trip/symmetry coverage, and the connection-password gate.
- Added structured GitHub issue forms and a pull request template.

### Fixed

- The TUI could not complete a default handshake (it never surfaced
  fingerprint verification) and never persisted chat history; both fixed.
- The TUI no longer registers each keystroke twice on Windows (acts on
  key-press only, not key-release).
- `ChatManager::rename_chat` truncated titles by bytes and could panic on a
  multi-byte/emoji boundary; it now truncates by characters.
- Hardened HKDF session-key derivation to use explicit zero-initialized
  output buffers; prevented AES-GCM nonce-counter wraparound (the transport
  fails closed instead of risking nonce reuse); rejected malformed legacy
  `EPHEMERAL_KEY:` payloads before allocation.
- Full data wipe and Windows uninstall cleanup now actually remove the saved
  identity, encrypted history, diagnostics, and password-protected state.

## [1.8.1] - 2026-04-22

### Fixed

- Restored `cargo clippy --all-targets -- -D warnings` by removing the GUI/TUI
  lint regressions blocking CI.
- Reworked the release workflow to use idempotent
  `gh release create/upload --clobber` steps, so rerunning a tag no longer
  fails on an existing release.
- Locked macOS release builds to `Cargo.lock`; kept separate Intel and Apple
  Silicon DMGs.
- Synced shipped version metadata and README packaging claims with the
  automated release behavior; added a proper onboarding tutorial and
  refreshed the user, developer, security, protocol, and audit docs.

## [1.8.0] - 2026-04-22

### Added

- **Automated packaging & distribution.** A GitHub Actions release workflow
  builds Windows (`Inno Setup` installer), macOS (proper `Messenger.app`
  bundles with `Info.plist`, `.dmg` images for Intel and Apple Silicon), and
  Linux (tarball) artifacts, then creates the GitHub Release and uploads
  everything on each version tag.
- **Signed invite links (v2).** Invites are now signed with RSA-PSS-SHA256
  over the full payload (name, address, fingerprint, public key, timestamp,
  nonce, version), so tampering — including fingerprint-swap attacks — breaks
  verification. URL-safe base64 (RFC 4648); v1 unsigned invites still parse
  with a deprecation warning. Format documented in the protocol reference.
- **Automatic session key rotation.** The session key rotates every
  100 messages or 5 minutes (whichever first) via a new `Rekey` protocol
  message: an HKDF-SHA256 derivation over a fresh random nonce, resolved
  transparently at the protocol layer (never surfaced to the app), with
  sequence validation and simultaneous-rekey resolution. Overhead is
  negligible (~0.03 ms per operation).
- **Command-driven TUI shell** with a command palette and status line
  (`:host`, `:connect`, `:disconnect`, `:rename`, `:help`, `:quit`) and
  better rendering on small terminals.
- Diagnostics bundle export and on-disk panic logs for support and crash
  triage.

### Changed

- History format advanced to `1.1` (with `1.0` load compatibility); periodic
  GUI autosave moved off the hot UI path into a background task; repository
  metadata synced to `v1.8.0`; test keys/nonces replaced with `OsRng`-random
  values and magic numbers replaced with named constants
  (`AES_NONCE_SIZE`, `AES_GCM_TAG_SIZE`).

### Performance

- Enabled LTO with `codegen-units = 1`, symbol stripping, and panic-abort for
  release builds; optimized CI caching (up to 60% faster subsequent builds).

### Security

- Fixed handshake signature negotiation so the runtime truthfully advertises
  and accepts only RSA-PSS identity proofs.
- Bound encrypted identity proofs and transport packets to the handshake
  transcript with AAD.
- Disabled password-removal for persisted identities (encrypted-at-rest is
  enforced, and the GUI reflects it).
- Fixed the destructive local-data reset so it deletes the encrypted history
  and identity files it claims to remove.
- Centralized endpoint parsing (hostnames, IPv4, bracketed IPv6) across the
  app and invite handling.

## [1.7.5] - 2026-02-04

### Fixed

- Refactored the dialog system around an `ActiveDialog` enum, preventing
  multiple dialogs from opening simultaneously and ensuring state reset on
  close; fixed unused-variable warnings in `dialogs.rs`.

### Documentation

- Merged the compatibility audit into `README.md` (System Requirements) and
  the security audit into `SECURITY.md` (Audit History); roadmap updated with
  planned pure-Rust mDNS and QR scanning.

## [1.7.4] - 2026-02-01

### Changed

- Fixed the local CodeQL script (`run-codeql-local.ps1`) to handle virtual
  drive mounting correctly (removed the redundant copy that failed with
  "Cannot overwrite file with itself").

## [1.7.3] - 2026-01-24

### Security

- **Signature scheme negotiation with Ed25519 support.** Ed25519 identity key
  generation and signing behind a `SignatureScheme` handshake negotiation,
  backward compatible with RSA-2048.
- **Transport-layer replay protection.** Per-session sequence tracking
  validates every incoming message before it is emitted; out-of-order,
  duplicate, and old messages are rejected.

## [1.7.2] - 2026-01-24

### Security

- **AAD support in AES-256-GCM.** `encrypt()`/`decrypt()` accept optional
  Additional Authenticated Data for context binding (backward compatible when
  not provided).
- **Payload size validation verified and strengthened**, with a regression
  test for oversized frame headers (TCP framing already read in bounded
  chunks).
- **CI/CD security pipeline.** `security.yml` with six parallel jobs
  (rustfmt, clippy, tests, cargo-audit, release build, cross-platform) plus
  `deny.toml` for supply-chain auditing.

### Documentation

- New `THREAT_MODEL.md` (threat-actor profiles, attack scenarios with
  mitigations, asset analysis across the attack surfaces, known limits) and
  an expanded `SECURITY.md` with a responsible-disclosure process (severity
  SLAs, 30–90-day coordinated-disclosure embargo).

## [1.7.1] - 2026-01-22

### Security

- Upgraded the `rsa` crate to 0.9.10 to mitigate the "Marvin Attack" timing
  side channel (CVE-2026-21895); remediated CodeQL warnings about hard-coded
  cryptographic values in tests; updated dependencies for upstream security
  fixes.

## [1.7.0] - 2026-01-04

### Fixed

- **Removed the 2 GB file-transfer limit.** Chunking makes a hard-coded cap
  unnecessary; large files can be sent again.

## [1.6.0] - 2026-01-04

### Security

- **Protocol v3: encrypted identity exchange.** Identity proofs are exchanged
  *inside* the encrypted tunnel, so observers can no longer see public keys
  or fingerprints (metadata protection).
- **DoS protection:** packet headers validated before allocation (streaming
  reads), strict handshake timeouts, and per-IP connection rate limiting.
- **Robustness & hygiene:** removed 100+ `unwrap()` calls from critical
  network paths; sensitive keys are now `Zeroize`d.

### Documentation

- `SECURITY.md`, the roadmap, and the protocol reference updated for the new
  posture.

## [1.5.0] - 2025-12-20

### Security

- Replaced hard-coded cryptographic keys in the test suites with securely
  generated random keys (resolving five CodeQL warnings); hardened
  `AesCipher::new` to return a `Result` instead of crashing on an invalid key
  length.

### Fixed

- Rewrote `test_full_handshake` to exercise the production ECDH handshake
  (the old test diverged from the implementation) and fixed a malformed
  payload in `test_file_meta_parsing_robustness` so it actually validates
  input sanitization.

### Changed

- Fixed all clippy warnings; synced `DEVELOPER_GUIDE.md` with the production
  codebase (nonce generation, full `ProtocolMessage` definition).

## [1.4.0] - 2025-12-19

### Added

- **QR code invites.** Generated invite links can be displayed as QR codes
  for easy scanning and contact addition.

### Security

- **Version-downgrade protection:** peers exchange digitally signed protocol
  versions, verified against RSA public keys.
- **Replay protection:** a `seq: u64` on every `ProtocolMessage` variant with
  per-chat `send_seq`/`recv_seq` tracking; invalid or duplicate sequence
  numbers are discarded. Covers text, file, ping, and typing messages.
- **Encrypted chat history at rest:** ChaCha20-Poly1305 with a random nonce
  per save, authenticated against tampering, 0600 permissions on Unix.
- **Counter-based AES-GCM nonces** (`session_id (4) || counter (8)`)
  guarantee uniqueness and eliminate birthday-collision risk.
- Overall assessed risk moved from CRITICAL to MEDIUM: all critical and
  high-priority findings of the era's audit were fixed.

### Fixed

- Rust 2021 compatibility: replaced let-chains with nested `if let`, migrated
  deprecated ChaCha20-Poly1305 and RSA-PSS APIs.

## [1.3.1] - 2025-11-16

### Changed

- Auto-rehost shows a success toast after a listener restarts, and a guard
  prevents multiple concurrent listeners on the same port (with a unit test
  for the placeholder-host detection it relies on).

## [1.3.0] - 2025-11-12

### Fixed

- **Chat creation now syncs to the peer.** Creating a chat from the contacts
  list created it locally but never propagated it, causing "all recipients
  offline" errors. `SessionEvent::NewConnection` now notifies the receiving
  peer, the handshake exchanges chat IDs, and the UI creates the chat locally
  for responsiveness before connecting in the background
  (`connect_to_host`/`connect_to_contact` accept an optional
  `existing_chat_id`).

## [1.2.0] - 2025-10-31

### Added

- **Emoji picker** (32 common emojis), **drag & drop file transfer**,
  **desktop notifications** (configurable), **typing indicators**,
  **auto-save every 30 s**, **per-conversation deletion** (with confirm
  dialog), **keyboard shortcuts** (`Ctrl+Enter` send, `Escape` clear), and
  **connection status indicators**.

### Changed

- Improved chat header, typing feedback, Settings toggles, clickable chat
  rows, and error toasts. New dependencies: `notify-rust`, `emojis`; protocol
  extended with `TypingStart`/`TypingStop`; `Config` gains
  `enable_notifications` and `enable_typing_indicators`.

## [1.1.0] - 2025-10-31

### Security

- **Forward secrecy.** X25519 ECDH with ephemeral per-session keys (discarded
  after use) and HKDF-SHA256 key derivation: past messages stay secure even
  if long-term RSA keys are compromised. Protocol version 2 introduces
  version negotiation against downgrade attacks.

### Changed

- Added `x25519-dalek` and `hkdf`; extended the protocol with `Version` and
  `EphemeralKey` messages; updated the handshake sequence.

## [1.0.2] - 2025-10-23

### Fixed

- **Messages were sent but never appeared on the receiving side:** session
  events were logged but never processed. The UI update loop now polls and
  handles all session events (`Listening`, `Connected`, `MessageReceived`,
  …), with comprehensive tracing through the network layer.

## [1.0.0] - 2025-10-23

### Added

- Complete UI/UX overhaul turning the prototype into a polished app: welcome
  screen, settings panel (download folder, limits), multiline input, colorful
  avatars, smart timestamps, smart send button, tooltips, consistent layout.

### Changed

- Consolidated documentation; fixed assorted borrow-checker issues and
  warnings.

## [0.9.0] - Initial version (undated, pre-tagging)

- Basic chat functionality, end-to-end encryption (RSA + AES-GCM), file
  transfer, a simple GUI, and message history persistence.

[Unreleased]: https://github.com/fibo3090-code/secure-p2p-chat/compare/v1.16.0...HEAD
[1.16.0]: https://github.com/fibo3090-code/secure-p2p-chat/compare/v1.15.0...v1.16.0
[1.15.0]: https://github.com/fibo3090-code/secure-p2p-chat/compare/v1.14.0...v1.15.0
[1.14.0]: https://github.com/fibo3090-code/secure-p2p-chat/compare/v1.13.0...v1.14.0
[1.13.0]: https://github.com/fibo3090-code/secure-p2p-chat/compare/v1.12.1...v1.13.0
[1.12.1]: https://github.com/fibo3090-code/secure-p2p-chat/compare/v1.12.0...v1.12.1
[1.12.0]: https://github.com/fibo3090-code/secure-p2p-chat/compare/v1.11.1...v1.12.0
[1.11.1]: https://github.com/fibo3090-code/secure-p2p-chat/compare/v1.11.0...v1.11.1
[1.11.0]: https://github.com/fibo3090-code/secure-p2p-chat/compare/v1.9.0...v1.11.0
[1.9.0]: https://github.com/fibo3090-code/secure-p2p-chat/compare/v1.8.1...v1.9.0
