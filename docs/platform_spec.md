# Encrypted P2P Messenger — Platform Plan & Roadmap

This is the **single canonical forward-looking document** for the project. It owns
the product vision, architecture, trust model, the planned UI rewrite, the phased
roadmap, and the backlog. It supersedes and absorbs the former separate plan docs
(platform spec, Phase 1 plan, UI refront plan, design system, development plan).

Scope boundaries:

- [architecture.md](architecture.md) describes the code as it is **today**.
- [protocol.md](protocol.md) describes the **shipped wire behavior** only.
- **This document** owns everything *forward-looking*: where the product is going
  and how. When a planned item ships, move its description into `architecture.md`
  or `protocol.md` and trim it here.

---

## 1. Context & Goals

The original app is a 1:1 peer-to-peer encrypted messenger. Its blocker for
non-technical users (the motivating case: classmates) is that pure P2P needs port
forwarding, which they can't do. The goal is to evolve it into a **hybrid
platform**: keep the strong direct-P2P story, and add a **self-hosted central
"Party" server** that friends join with a simple flow — address + optional
password + pick a username — then see a member directory, chat in channels and
DMs, and (later) share files, with messages delivered even to people who were
offline.

The relay is *not* the answer to this: it only pairs two peers and cannot host a
multi-user room. The Party server is the answer.

Guiding decisions:

- **Spec-first.** Design before building; this doc is the design of record.
- **Two trust tiers.** An **Administered** (server-trusted) tier as the default,
  and an optional **E2EE** tier later.
- **One cohesive product**, one identity, one tabbed UI — not glued-together
  pieces.

---

## 2. Current State

An accurate audit of what exists today (verified against the codebase).

### P2P — mature

Protocol v3 handshake (X25519 ECDH → HKDF-SHA256 → AES-256-GCM), TOFU fingerprint
verification, forward secrecy, replay protection, rekeying, 1:1 text + file
transfer up to 10 GiB, typing indicators, encrypted local history, and signed
invite links. "Group chat" today is only client-side fan-out over multiple 1:1
sessions — there is no server-backed group.

### Independent P2P hardening — done

An optional **connection password** (verified inside the established v3 tunnel,
after identity verification and before TOFU, with a constant-time compare) and a
**conversation lock** (the host refuses new connections) ship today. Both are
reachable from the GUI (Host/Connect dialogs + a lock toggle) and the TUI
(`:connection-password`, `:lock`).

### Relay — stateless 1:1 rendezvous

`core/src/network/relay.rs`. Pairs one host and one joiner by token and forwards
ciphertext via `copy_bidirectional`; it never terminates chat encryption. No auth,
rooms, or storage. The `--relay-server` mode ships (`client/src/main.rs`,
`Args.relay_server` → `network::run_relay_server`). It cannot host a multi-user
room.

### Party server — built (Administered MVP)

The Party server exists and works for the Administered tier. See §7 for detail.
In short: `core::party` defines the application protocol; `messenger-server` runs a
real TCP listener with a persistent owner-only identity, applies requests to an
in-memory `PartyState`, fans out live broadcasts across connections, serves
channels and server-routed DMs with offline history catch-up, and persists state
to an embedded SQLite database. A client can join, chat in channels, DM, and create
channels via TUI commands and a GUI Party window.

### Current UI — egui + ratatui, plus the new Tauri/React desktop app

The app ships an **egui/eframe** desktop GUI and a **ratatui** TUI, both driving
the same `ChatManager` (and `PartyManager`). The egui GUI is a top menu bar +
status bar + a left "Chats" sidebar + a central panel + a stack of modeless
dialogs, with the Party experience in a separate floating window.

The **§10 rewrite has shipped** as a fourth crate, `p2pem-desktop`
(`desktop/src-tauri`), wrapping a **React/Vite** web UI (`desktop/src/`) that
realizes the tab-rail / list / content shell. It drives the same managers through a
`#[tauri::command]` bridge and reaches feature parity for onboarding, P2P
conversations, contacts, invites, fingerprint verification, relays, the
Party/Communities surface, settings, and toasts. It is meant to **replace egui**,
but the release pipeline still builds the egui binary — so today both GUIs coexist
and the desktop app is run from source (`cd desktop && npx tauri dev`). The stack
landed as React rather than the originally-planned SolidJS (see §10).

### Crate layout — Cargo workspace (done, four crates)

Four crates: `core` (`messenger-core`), `client` (`p2pem-classic`, the
unified app + binary), `server` (`messenger-server`), and `p2pem-desktop`
(`desktop/src-tauri`, the Tauri shell). The client re-exports core via
`pub use messenger_core::*`; the desktop crate depends on `client` for the
managers. The client crate is `p2pem-classic` (renamed from `encodeur_rsa_rust`) and packaging paths
are unchanged. Bare `cargo` commands still target the client.

---

## 3. Product Shape

One app, one identity, a left **tab rail** (Discord-like):

- **P2P** — direct encrypted DMs between peers (today's strength).
- **Relay** — NAT-traversal helper for 1:1 P2P when neither side can port-forward.
- **Party** — the servers you've joined; each has channels, server-DMs, members,
  and (later) files.

```text
┌──────┬─────────────────────────────────────────┐
│ P2P  │                                          │
│ Relay│   (active tab's content)                 │
│ Party│   Party: servers → channels · DMs ·      │
│ ───  │          members · files · (governance)  │
│ ⚙    │                                          │
└──────┴─────────────────────────────────────────┘
```

Identity/keys/local vault fold into **settings** rather than being a headline tab.
Advanced surfaces sit behind an "advanced" affordance.

---

## 4. Architecture

The workspace (already in place) lets the client and server share code yet ship as
one product:

```text
core/             crypto, protocol/wire types, framing, identity, transport, shared
                  types, and the Party application protocol (core::party). Reused everywhere.
client/           the unified app: egui GUI + ratatui TUI, ChatManager, PartyManager, persistence.
server/           the Party server: TCP listener, PartyState, dispatcher, broadcast hub,
                  persistent identity, SQLite store.
desktop/src-tauri the Tauri 2 shell (p2pem-desktop): #[tauri::command] bridge + event
                  pump over client's managers, wrapping the React/Vite UI in desktop/src/.
```

Relay stays a thin mode (`--relay-server`) alongside `core`.

**Binaries** ship from one repo/release: a default `client` binary (named
`p2pem-classic`) and the `messenger-server` binary.
The `p2pem-desktop` binary builds via `cargo tauri build`/`dev` but is not yet in
the tagged release pipeline (which still ships the egui `client` binary).

### Transport matrix

- **P2P / Relay** — the existing v3 E2EE session, unchanged.
- **Client ↔ Party server** — the **same v3 handshake** establishes an
  authenticated, encrypted channel to the *server* (which has its own
  identity/fingerprint, TOFU-verified once). The **Party application protocol**
  (join, directory, channel ops, messages, DMs) rides on top of that channel.

```text
client ──v3 handshake (core)──▶ encrypted+authenticated tunnel ──▶ core::party
                                                                     │
                                                  messenger-server applies it to PartyState
```

---

## 5. Identity Model

- **Global private identity** (one per user): the existing RSA-2048 key material +
  fingerprint, password-encrypted at rest (`core/src/identity`). Never exposed
  wholesale.
- **Per-server identity** (Phase 5): a server-scoped profile (display name,
  visibility, and for E2EE servers a server-scoped keypair) bound to the global
  identity by a signature, so one person can present context-appropriate faces.
  The MVP uses the global identity with a per-server **display name**; the data
  model reserves room for distinct per-server keys from the start.

---

## 6. Security & Trust Tiers

Designed in from day one. Every stored/transported message and file carries an
**envelope**:

```text
{ tier, sender, channel, seq, timestamp, payload }
```

`payload` is plaintext (Administered) or ciphertext (E2EE). One data model, two
tiers:

- **Administered (default)** — client↔server is encrypted, but the server stores
  **plaintext**. This enables offline buffering, admin-read (clearly labeled),
  search, and simple groups. The server wears an honest "this operator can read
  messages" badge.
- **Private / E2EE (Phase 4)** — a per-channel **group key**; the server stores
  only ciphertext and never sees plaintext; admins cannot read. Offline buffering
  stores encrypted blobs. Group-key distribution and rotation on membership change
  is the hard part.

Trust tier is a **server property**, shown prominently. Raising exposure (e.g.
enabling admin-read) triggers a **consent-or-leave** flow.

---

## 7. Party Server (Phase 1 — Administered)

The Party protocol is a shared wire contract in `core::party`, reused by client
and server, riding on top of the v3 encrypted tunnel.

### What is built

- **`core::party`** — the application protocol: `TrustTier`, `Envelope`,
  `MessagePayload`, `MemberInfo`, `ChannelInfo`, `ChannelKind`; the
  `PartyRequest` / `PartyResponse` enums (Join, ListMembers, ListChannels,
  PostMessage, FetchHistory, SendDm, FetchDmHistory, CreateChannel); a
  deterministic `dm_thread_id(a, b)` (SHA-256 of the sorted member ids); framed
  send/recv over the tunnel (`send_framed`/`recv_framed`); and a `PartyClient`
  with a split reader/writer. All bincode-serialized with round-trip tests.
- **Handshake reuse** — the v3 handshake was extracted into
  `host_handshake`/`client_handshake` returning an `EstablishedTunnel
  { peer_fingerprint, peer_chat_id, cipher, transport_aad }`. The P2P session
  loop, the relay path, and the server all build on the same handshake; existing
  handshake/E2E tests pass unchanged.
- **`messenger-server`** — a real TCP accept loop; `serve_connection`
  (per-connection handshake → a `select` loop that serves requests via the
  dispatcher and writes pushed broadcasts, binding the verified fingerprint to the
  member); a cross-connection **broadcast hub** (a posted message reaches every
  other connected member live); a **persistent owner-only server identity**
  (`server::identity`, RSA PEM under the data dir, so the pinned fingerprint is
  stable across restarts); and **durable state** — `PartyState::load` mirrors
  members + channels + DM threads + history to an embedded **SQLite** database
  (`party.db`), writing each mutation's delta and reconstructing the in-memory
  model on startup (presence resets to offline); a legacy `party_state.json`
  snapshot is imported once on first load.
- **Server-routed DMs** — a deterministic per-pair DM thread; the server stores
  and delivers DMs like channels, with offline history.
- **Channel creation** — members can create channels at runtime.
- **Client integration** — `client::app::party_manager::PartyManager`
  (per-connection read/write task, directory/channel/history tracking, optimistic
  post, DM send/fetch, channel creation, error surfacing); TUI commands
  (`:party-connect`, `:party-post`, `:party-dm`, `:party-create-channel`,
  `:party-status`); and a GUI Party window (`gui::party_view`) with join form,
  server selector, channel + member lists, DM/channel views, and a post box.

### Server data model (MVP)

In memory (authoritative at runtime), mirrored row-by-row to SQLite:

```text
PartyState { name, password: Option<String>, tier, members, channels, dm_threads,
             db: Option<Connection> }
Member  { id, username, fingerprint: Option<String>, online }
Channel { id, name, kind, messages: Vec<Envelope> }   # messages = durable history
```

SQLite tables: `members`, `channels` (with a `position` column for stable
ordering), `messages`, `dm_threads`, `dm_messages` (envelopes stored as JSON).

### What remains

- The filesystem **blob store** for files (Phase 2) — the SQLite metadata store is
  now in place (see §9).
- A dedicated **TUI Party pane** (beyond command output) and **server-identity
  TOFU** confirmation UI in the GUI.
- Roles/permissions, governance/audit, lock/password gating per channel (Phase 3);
  files (Phase 2); the E2EE tier (Phase 4).

---

## 8. Files / "Drive" (Phase 2)

**Shipped (slice 1):** content-addressed inline file sharing in the Party server.
A `MessagePayload::File(FileMeta)` carries a file reference in channel and DM
history; `PartyRequest::PostFile` / `SendFileDm` upload bytes inline (bounded by
`MAX_INLINE_FILE_BYTES = 4 MiB`) and post a file message that broadcasts like a
text message; `DownloadFile { hash }` returns the bytes. The server stores each
blob once, keyed by SHA-256, reference-counted, with the bytes on disk under
`<data_dir>/blobs/<hash>` and metadata (`hash, size, mime, refcount`) in the
`blobs` SQLite table. **Client wiring (done):** the desktop app can now share a
file into a channel or DM (a paperclip in the composer → `PartyManager::send_file`
/ `send_file_dm`, size-checked against `MAX_INLINE_FILE_BYTES`) and download a
received file (a file card → `PartyManager::request_download`, which correlates the
async `FileData` response by content hash and saves via a native dialog).
**Remaining:** chunked transfer for large files; the permission matrix, quotas,
provenance, and the Drive UI panel below; the same wiring in egui/TUI.

Target data model — content-addressable, deduplicated storage:

- **Blob** — raw content stored once: `hash, size, mime, (ciphertext?)`.
- **FileEntry** — a logical file in the UI: `name, owner, server, location, date,
  meta`.
- **PermissionMatrix** — `view / download / delete / share / admin` (view ≠
  download ≠ share; share is explicit; you can only delegate rights you hold).
- **FileReference** — a specific share instance: where / which channel or DM / by
  whom / with which permissions. Blobs are **reference-counted** and deleted only
  when no reference or retention rule holds them.
- **Quotas** — physical (bytes stored, dedup-aware) and logical (per-user /
  per-role references), freed on delete.
- **Provenance / audit** — who uploaded, where it was shared, what permissions
  traveled with it.
- **UI** — a Drive-like panel: list, size, date, location, quota used/left, and
  download / delete / move / permissions actions.

---

## 9. Data Model (objects)

`Identity(global)`, `ServerProfile(per-server)`, `Server`, `Membership`,
`Channel`, `Message(envelope)`, `DMThread`, `Blob`, `FileEntry`,
`PermissionMatrix`, `FileReference`, `Quota`, `Role`, `Policy`, `AuditLog`.

**Server persistence:** embedded **SQLite** (`party.db`) for metadata under the
operator's data dir, with a filesystem blob store still to come for files
(Phase 2), chosen for zero-ops single-host hosting with room to swap later. The
former interim JSON snapshot is imported once on first load, then superseded.

---

## 10. UI Rewrite (Tauri + React) — shipped

> **Status:** shipped as the `p2pem-desktop` crate (`desktop/`). This section is
> the design of record; the stack notes below record **what actually landed**
> (React/Vite, not the originally-planned SolidJS) and the phase list marks what is
> done. What remains is egui retirement and packaging (phases E–F).

The egui GUI is an accumulation, not a design: a top menu bar, a status
bar, a left "Chats" sidebar, a central panel, a pile of modeless dialogs (the
dialog file alone is ~84 KB), and — the clearest symptom — the **Party experience
in a separate floating window**, disconnected from everything else. Users juggle
two mental models. This rewrite realizes §3's tab-rail vision with a designed UI.

### Target shape — a 3-pane shell

```text
┌──┬───────────────┬─────────────────────────────┐
│ R│   List pane   │      Content pane           │
│ a│ (chats /      │  (messages + composer, OR   │
│ i│  channels +   │   directory, OR a settings  │
│ l│  members /    │   page — no floating modals)│
│  │  relay sess.) │                             │
└──┴───────────────┴─────────────────────────────┘
 icon rail (top→bottom): P2P · Relay · Party · … · identity/lock/settings
```

- The **icon rail** replaces the menu bar and switches mode (P2P / Relay / Party);
  identity, lock, and settings dock at the bottom.
- The **list pane** is contextual to the mode.
- The **content pane** holds the conversation *or* a full-pane page for directory
  and settings — eliminating most modal dialogs.
- **Overlays only for genuinely interruptive flows**: fingerprint verification,
  password/unlock, consent-or-leave. Settings, Contacts, Connect, Host become
  inline pages.
- **Party stops being a floating window** and becomes the Party tab, sharing the
  same list+content layout as P2P. One mental model.

### Stack (as shipped)

The plan originally called for SolidJS + TypeScript + Tailwind + Kobalte/Motion
One/TanStack Virtual. What actually shipped is a **simpler React stack** — chosen
for a familiar toolchain and fast iteration; the a11y/animation/virtualization
libraries were dropped in favor of hand-rolled components and plain CSS.

| Layer | Shipped choice | Why |
|---|---|---|
| Shell | **Tauri 2** | Native window + system webview; the Rust core stays behind an IPC boundary; capability/CSP security model (tighter than Electron); no bundled Node runtime. |
| Frontend | **React 19 + JSX** (no TypeScript) | Ubiquitous toolchain, fast to build against the bridge; the message stream has not needed fine-grained-reactivity tuning yet. |
| Styling | **Plain CSS** (`styles.css` / `themes.css` / `app-system.css` / `polish.css`) with the design tokens below | No Tailwind; the token palette lives in CSS custom properties. |
| Components | **Hand-rolled** (`components/ui.jsx`) + **lucide-react** icons | No Kobalte/Motion One/TanStack Virtual yet; accessibility and virtualization are follow-ups. |
| Build | **Vite 8** + `@vitejs/plugin-react` | `tauri dev`/`tauri build` orchestrate it; `npm run dev`/`build` for frontend-only work. |

**Accepted costs (owned deliberately):** a JS toolchain joined a previously
pure-Rust repo; the ratatui TUI stays Rust; **packaging is not yet rebuilt** (the
release pipeline still ships the egui binary — see below); the security surface
grew by a system webview + IPC (crypto stays in Rust; the CSP in `tauri.conf.json`
is restrictive, no external hosts). Because egui is not yet deleted, both GUIs
coexist during the migration.

### The Rust↔web bridge

The business logic is already view-agnostic — the existing TUI proves it by
driving the same managers as the GUI. The rewrite keeps the managers in Rust and
exposes them to the web UI:

- **Commands** (`#[tauri::command]`): each manager method the UI calls becomes a
  command (`connect`, `host`, `send_message`, `set_connection_password`,
  `confirm_fingerprint`, `party_join`, `party_post`, `party_send_dm`,
  `party_create_channel`, `party_fetch_history`, …).
- **Events** (`app_handle.emit`): the existing `SessionEvent` / party-event mpsc
  streams map almost 1:1 onto Tauri's event channel. A background task drains
  them and emits typed events the React frontend subscribes to (via `onBridge` in
  `desktop/src/lib/bridge.js`) — replacing per-frame `try_lock()` polling with push.
- **Shared types:** *as shipped*, the bridge is hand-written in `lib/bridge.js`
  (no generated TS types — the plan's `ts-rs`/`tauri-specta` step was dropped along
  with TypeScript). DTOs are still defined once in Rust (`serde`); keeping the JS
  bridge's field names aligned with the command signatures is a manual discipline
  (and a Tauri arg-naming footgun — see `CLAUDE.md`). Wire framing stays in `core`;
  the `to_plain_bytes`/`from_plain_bytes` symmetry discipline is unaffected.

The rewrite landed as its **own top-level crate** rather than nested under
`client/` (so the egui `client` binary is untouched during the migration):

```text
core/ (unchanged)
client/ (egui + ratatui, unchanged)
desktop/
├── src-tauri/   Rust: #[tauri::command] bridge + event pump over client's managers (lib.rs)
└── src/         React/Vite app: rail / list / content + overlays + CSS design tokens
```

`client --tui` opens the ratatui UI and `client` opens the egui GUI; the Tauri
desktop app is launched separately with `cd desktop && npx tauri dev`. Phase E
(below) will retire egui; only then does a single `client` launch route to Tauri.

### Visual language (design tokens)

> This section originally documented a warm-amber/neon-green/Inter target that
> was never implemented — the shipped system used a sky-blue accent and IBM
> Plex Sans instead, and that drift went unremarked for a while. It's now
> superseded by the "control teal-indigo" brand pass below; treat
> **`design/tokens.json`** (repo root) as the source of record for exact hex
> values going forward instead of re-embedding them in this prose, since
> that's exactly what drifted last time.

A dark, layered aesthetic — information illuminated on a deep-navy base, depth
from tonal layering rather than borders or heavy shadows.

- **Surfaces (no 1px section borders):** structure comes from background shifts,
  negative space, and tonal transitions, not divider lines. See `design/tokens.json`
  → `themes.dark`/`themes.light` for the exact surface ramp (`bg`/`s1`/`s2`/`s3`/`s4`).
- **Accent:** brand "control teal-indigo" (`design/tokens.json` → `brand`), a
  teal→indigo gradient (`#2dd4bf`→`#4f46e5`) used on the app icon/logo mark; UI
  chrome (buttons, borders, selection) uses the flat mid-tone `flatAccent`
  (`#3e8dd2`) for the Dark and Light themes. Midnight/Forest/Rose keep their own
  distinct accent hues (violet/green/pink) as alternate theme personalities, not
  brand-compliance failures — only Dark/Light carry the brand color.
  `success`/`warning`/`error` are separate semantic colors, unaffected by theme.
- **Typography:** `Space Grotesk` for display/headlines, `IBM Plex Sans` for
  body/labels, `IBM Plex Mono` for monospace — this is what's actually shipped
  (`desktop/src/app-system.css` `--font`/`--display`/`--mono`).
- **Elevation:** tonal layering over drop shadows; floating modals use a soft
  ambient "ghost" shadow and optional translucent blur. Any border that
  accessibility requires is a faint `outline_variant`, felt not seen.
- **Components:** primary buttons are solid accent, no border, modest radius
  (no full pills); inputs use the highest surface with a focus underline/glow
  rather than a heavy outline; lists separate items with spacing / alternating
  surfaces, not divider lines.

These tokens carry the theme names Light / Dark / Midnight / Forest / Rose as
CSS custom-property token sets (`desktop/src/app-system.css`, `desktop/src/themes.css`)
— the plan's Tailwind token system was replaced by plain CSS variables. The same
theme names exist as `core::types::Theme` (Rust, shared by egui/TUI persistence)
and as egui `Visuals` builders in `client/src/gui/styling.rs`; the ratatui TUI
does not implement per-theme rendering (terminal color-depth constraints make a
5-theme TUI disproportionate) but does use the brand accent for theme-neutral
chrome (active-pane borders, key hints) — see `client/src/tui/overlays.rs`'s
`BRAND_ACCENT`. There is no automated cross-language token pipeline; a Rust test
(`client/src/gui/styling.rs`'s `token_drift_tests` module) parses `design/tokens.json`
and asserts it matches the egui `Visuals` builders, so future drift fails a test instead of
going unnoticed again.

### Packaging rebuild

The current pipeline still assumes one eframe binary named `p2pem-classic`:
`release.yml` builds `-p p2pem-classic` then hand-rolls Inno Setup
(`setup.iss`), a macOS `.app`+`.dmg`, and a Linux tarball; `build-and-package.ps1`
and `setup.iss` hardcode that name. **Done:** `desktop/src-tauri/tauri.conf.json`
already exists (productName `P2PEM`, identifier `com.chat-p2p.p2pem`, window
1040×720) with a restrictive CSP, and `ci.yml` builds the desktop crate (WebKitGTK
deps installed). **Remaining (phase F):** rewrite `release.yml` to install Node +
Rust and run **`tauri-action`** per-OS (Tauri's own bundler → `.msi`/`.nsis`,
`.app`/`.dmg`, `.deb`/`.AppImage`), keeping the `on: push: tags: 'v*'` trigger;
retire `setup.iss` and `build-and-package.ps1`; and add a frontend build/lint job
to `ci.yml`. Until that lands, releases ship the egui binary.

### Phased execution

Sequenced so each phase is reviewable and the shipped binary keeps building (egui +
TUI stay functional throughout). Phases A–D shipped as the `desktop/` crate:

- **A — Scaffold & bridge. ✅** Tauri 2 + React/Vite as a top-level `desktop/`
  crate (`src-tauri/` + `src/`); command + event round-trip; CI builds both
  toolchains (WebKitGTK deps added). (Landed as React, not Solid; no generated TS
  types — the bridge is hand-written in `lib/bridge.js`.)
- **B — P2P tab end-to-end. ✅** Rail → chat list → message view → composer →
  fingerprint-verify overlay → connect/host as inline pages; lock / password flows.
  (Message list is not yet virtualized.)
- **C — Party tab. ✅** Server join, channels + members, channel messages, DMs,
  channel creation, presence — reusing the list+content layout (`Parties.jsx`).
- **D — Relay tab + Settings/Contacts pages. ✅** Remaining dialogs moved into
  inline panes; themes via CSS tokens (`themes.css`).
- **E — Delete egui. ☐ remaining.** Remove `client/src/gui/` and the egui deps;
  route the default GUI launch to Tauri. Not started — egui still ships.
- **F — Packaging + docs. ◐ in progress.** Rewrite `release.yml` to `tauri-action`
  and retire the old packaging scripts (**not done** — release still builds the
  egui binary); add a webview/CSP note to `SECURITY.md` and refresh docs (this
  pass). 
- **G — TUI 3-pane redesign. ☐ remaining.** Apply the rail/list/content model to
  ratatui, preserving the typed command language.

---

## 11. Roadmap

Each phase gets its own detailed plan before code lands.

```text
Phase 0  Workspace refactor                         ✅ done
Phase 1  Party Server MVP (Administered)            ✅ core + SQLite done · UI polish remains
UI       Tauri + React desktop app (§10)            ◐ shipped (A–D) · egui retirement + packaging (E–F) remain
Phase 2  Drive / files                              ◐ slice 1 (inline file sharing) done
Phase 3  Governance & roles
Phase 4  E2EE server tier
Phase 5  Per-server identities

Independent:  P2P connection passwords + conversation lock   ✅ done
```

- **Phase 0 — Workspace refactor. ✅** Split into `core`/`client`/`server` with no
  behavior change; CI moved to `--workspace`; packaging untouched (binary name
  preserved). Test coverage broadened (see §13).
- **Phase 1 — Party Server MVP (Administered). ✅ core complete.** Server binary,
  join-by-address (+ optional password) + username, member directory + presence,
  channels, server-routed group + DM messaging, offline buffering via history
  catch-up, channel creation, the GUI Party window + TUI commands, server-identity
  TOFU on the wire, and SQLite-backed durable state (`party.db`). **Remaining:**
  the filesystem blob store for files (Phase 2); TUI Party pane; GUI
  server-TOFU/error polish (see §7).
- **UI rewrite — Tauri + React. ◐ shipped (A–D).** Landed as the `desktop/`
  crate with P2P, Party, Relay, Contacts, and Settings reaching parity; full plan
  and stack notes in §10. Remaining: delete egui (E) and rebuild packaging so the
  release ships the Tauri app instead of the egui binary (F). Plan a `2.0.0`
  release when Phase F lands (owner authorizes the tag).
- **Phase 2 — Drive / files. ◐ slice 1 done + desktop client wiring.** Inline
  (≤4 MiB) content-addressed file sharing in channels & DMs with hash dedup +
  reference counting and on-disk blobs landed (see §8), and the **desktop app can
  now upload and download** community files. Remaining: chunked transfer for large
  files, delete UX, logical + physical quotas, the Drive panel, and egui/TUI
  parity.
- **Phase 3 — Governance & roles.** Trust-tier labeling, transparency panel,
  consent-or-leave, audit log, roles/permissions, visibility & contact policies,
  channel lock/password.
- **Phase 4 — E2EE server tier.** Per-channel group keys, ciphertext-only storage,
  key rotation on membership change, encrypted offline blobs; admin-read disabled.
- **Phase 5 — Per-server identities.** Distinct per-server profile/keys bound to
  the global identity.
- **Independent — P2P connection passwords + conversation lock. ✅** Shipped; see
  §2.

---

## 12. Backlog

Near-term items not tied to a numbered phase. (When one becomes real behavior,
update the canonical doc — `architecture.md`/`protocol.md` — in the same change.)

**Productization**

- Reduce UI-thread blocking and responsiveness issues (largely addressed by the
  §10 rewrite's push-based event model).
- Tighten persistence, migration, and recovery behavior.
- Improve diagnostics and failure visibility for users; better diagnostic export
  for support/bug reporting.

**Security & privacy**

- Keep security docs accurate as implementation changes; keep dependency risk
  under review.
- Harden mDNS registration/removal behavior and discovery privacy.
- Stronger invite lifecycle controls: signed-invite **expiration is enforced**
  (30 days, timestamp covered by the signature); **revocation** before expiry
  remains open.
- **Ed25519 identity proofs** (planned, own dedicated change — see below).

### Ed25519 migration plan (proposal, not started)

The wire already carries a `SignatureScheme` field in `IdentityProof`, but only
RSA-PSS is implemented, and the `rsa` crate carries the unresolved
RUSTSEC-2023-0071 timing advisory. Moving identity proofs to Ed25519
(`ed25519-dalek` is already a dependency) removes that exposure and shrinks
handshakes, but it is **not** a drop-in swap because the peer **fingerprint is
derived from the identity public key** — naively replacing the key would change
every user's fingerprint and break TOFU continuity for all existing contacts.
Planned shape:

1. **Dual-key identities**: new identities get both an RSA-2048 and an Ed25519
   keypair; existing identities grow an Ed25519 keypair on first unlock after
   upgrade. The fingerprint stays RSA-derived for now (continuity).
2. **Cross-signing**: the RSA key signs the Ed25519 public key once; the proof
   travels with the identity so peers can bind the new key to the fingerprint
   they already trust.
3. **Scheme negotiation**: `IdentityProof` advertises both schemes; peers that
   understand Ed25519 verify the Ed25519 proof (plus the cross-signature on
   first sight), older peers keep verifying RSA-PSS. No protocol version bump
   needed — the scheme field exists.
4. **Fingerprint cutover** (last, separate release): once Ed25519-capable
   versions are assumed, re-derive fingerprints from the Ed25519 key with an
   explicit re-verification prompt (a UX event, not silent), then retire the
   RSA path and the `rsa` dependency.

Steps 1–3 are back-compatible; step 4 is a breaking trust-model event and needs
its own comms/UX design. Each step lands with handshake tests on both the
old/new peer matrix.

**Connectivity**

- Internet connectivity: **UPnP port mapping is shipped** (opt-in; the host
  asks the router to forward the listening port and invites carry the external
  address, falling back to LAN/relay). Still open: NAT-PMP/PCP gateways, real
  hole punching for routers without IGD, and CGNAT (relay remains the answer
  there).

**UX**

- Accessibility pass over interactions and color usage.
- Settings IA cleanup / tabbed organization (folds into the §10 settings page).
- Better contact management UX and trust-state workflows.
- File-transfer **progress** now shows in all three UIs (desktop transfer bar,
  egui progress bar above the input, TUI title indicator); **cancellation**
  remains open — it needs a wire-level abort message (`ProtocolMessage`
  addition with symmetric encode/decode and replay-protected sequencing), so
  it is its own protocol change rather than a UI patch.

**Tracked long-horizon gaps** (intentionally not described as "done" anywhere):
onion routing / anonymity layer, post-quantum migration, hardware-backed identity.

---

## 13. Verification & Test Coverage

The workspace passes **301 automated tests** (`cargo nextest run --workspace`)
spanning unit, integration, and end-to-end suites (the `p2pem-desktop` crate has
no automated tests — it is verified with `cargo check -p p2pem-desktop` and
`npm run build`):

- **Protocol** (`core/src/core/protocol.rs`): round-trip symmetry for every
  `ProtocolMessage` variant, edge values (empty/unicode/max-size), malformed and
  oversized-payload rejection, `TextChunk` invariants, legacy ASCII parsing,
  `IdentityProof` serde, and debug redaction.
- **Crypto / handshake** (`core/src/core`, `core/src/network/session.rs`):
  RSA/AEAD/X25519/HKDF, transcript-bound AAD, replay protection, key rotation.
- **End-to-end pipeline** (`core/tests/session_e2e.rs`): the full A-to-Z path over
  the real session functions — version → ECDH → key derivation → encrypted
  identity proof → TOFU confirm → text, typing, file transfer, ping → disconnect,
  plus the fingerprint-rejection path and the connection-password gate
  (correct / wrong / missing).
- **Party server** (`core::party`, `messenger-server`): protocol round-trips, the
  in-memory `PartyState` (join/password/channels/history/DMs/persistence), the
  request dispatcher, and connection/broadcast end-to-end over the reused v3
  tunnel.
- **Relay** (`core/tests/relay_e2e.rs`): two peers pair through a self-hosted relay
  and exchange an application message over the forwarded encrypted transport.
- **Types** (`core/src/types.rs`): `Config` defaults (privacy-conservative), serde
  round-trips, backward-compatible deserialization.
- **Identity / persistence**: encrypted identity storage, encrypted-history
  round-trip, wrong-key rejection, corrupt-file handling, format auto-detection.
- **ChatManager** (`client/tests/feature_coverage_tests.rs` + in-file): group
  chats, rename/delete, history clearing, toast lifecycle, file-transfer state,
  typing indicators, contact import/association, invites (v1/v2/v3 + tamper
  rejection), invite-QR generation.
- **TUI**: command parsing, focus cycling, message round-trip, multi-chat
  isolation, typing flow.

The one area not deeply automated is GUI pixel rendering; the logic behind it
(`ChatManager`) is covered directly. Per-phase verification targets:

- **Phase 1**: two clients join over LAN, appear in the directory, exchange channel
  + DM messages; a message sent while a peer is offline is delivered on reconnect.
  (Met by the in-memory E2E tests in `server::connection` + client integration.)
- **UI rewrite**: per-phase gates in §10 — both toolchains build in CI; P2P then
  Party flows work end-to-end through the new UI; no `egui`/`eframe` symbols remain
  after Phase E; `tauri build` produces installers on all three OSes.
- **Later phases**: dedup/refcount/quota tests; governance consent-flow tests;
  E2EE group-key rotation tests; render-no-panic tests for new UI.

---

## 14. Status

Phase 0 (workspace) and the Phase 1 Party server core are complete; the workspace
test suite is green. The **Tauri + React desktop app** (§10) has shipped its P2P,
Party, Relay, Contacts, and Settings surfaces (phases A–D) as the `desktop/` crate,
closing most of the Phase 1 UI-polish items; retiring egui and rebuilding packaging
(phases E–F) remain. The Independent P2P hardening (connection passwords +
conversation lock) shipped. This document is the approved north star; each phase
gets its own detailed plan before implementation.
