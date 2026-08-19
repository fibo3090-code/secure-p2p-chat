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

An accurate audit of what exists today (last verified against the codebase at
**1.15.0**, 2026-08-03).

### P2P — mature

Protocol v3 handshake (X25519 ECDH → HKDF-SHA256 → AES-256-GCM), TOFU fingerprint
verification with a transcript-bound SAS, forward secrecy, replay protection,
automatic rekeying, 1:1 text (chunked past 48 KiB, hard-capped at 64 KiB) + file
transfer up to 10 GiB with an acceptance gate and either-side cancellation,
delivery receipts (`Ack`), typing indicators, encrypted local history with a
persisted read mark, opt-in mDNS LAN peer discovery, and signed invite links
(multi-address, 30-day expiry). "Group chat" today is only client-side fan-out
over multiple 1:1 sessions — there is no server-backed group.

### Independent P2P hardening — done

An optional **connection password** (verified inside the established v3 tunnel,
after identity verification and before TOFU, with a constant-time compare) and a
**conversation lock** (the host refuses new connections) ship today. Both are
reachable from the desktop app (the Host/Connect panes + a lock toggle) and the
TUI (`:connection-password`, `:lock`).

### Relay — stateless 1:1 rendezvous with hole punching

`core/src/network/relay.rs` (+ `punch.rs`). Pairs one host and one joiner by
token. When both peers are punch-capable, the server hands each side the
other's observed public endpoint plus LAN candidates and the peers **TCP hole
punch** a direct connection (simultaneous open from the reused source port,
validated and mutually confirmed — see `punch.rs`); the relay then carries no
session bytes at all. Only when punching fails does it bridge ciphertext via
`copy_bidirectional`; it never terminates chat encryption. Wire-compatible
both ways with pre-punch peers and servers. No auth, rooms, or storage. The
`--relay-server` mode ships (`client/src/main.rs`, `Args.relay_server` →
`network::run_relay_server`). It cannot host a multi-user room.

### Party server — built (Administered MVP)

The Party server exists and works for the Administered tier. See §7 for detail.
In short: `core::party` defines the application protocol; `messenger-server` runs a
real TCP listener with a persistent owner-only identity, applies requests to an
in-memory `PartyState`, fans out live broadcasts across connections, serves
channels and server-routed DMs with offline history catch-up, and persists state
to an embedded SQLite database (with file bytes in a content-addressed blob store
on disk). A client can join, chat in channels, DM, create channels, and — from the
desktop app — share and download files, via the desktop **Communities** tab or TUI
commands.

### Current UI — one desktop app (Tauri/React) plus a ratatui TUI

The product is **P2PEM Desktop**: the `p2pem-desktop` crate (`desktop/src-tauri`)
wrapping a **React/Vite** web UI (`desktop/src/`) that realizes the tab-rail /
list / content shell. It drives `ChatManager` and `PartyManager` through a
`#[tauri::command]` bridge and covers onboarding, P2P conversations, contacts,
invites, fingerprint verification, relays, the Party/Communities surface,
settings, and toasts. The stack landed as React rather than the
originally-planned SolidJS (see §10).

A **ratatui** TUI ships alongside it in the `client` binary, for headless boxes
and terminal users. It drives the same managers.

The **egui/eframe GUI has been deleted** (§10 phase E). Shipping two desktop
apps meant a customer arriving at the releases page had to guess which to
install — and the older, less capable one looked like the default. Nothing
in the workspace depends on egui/eframe any more, which also removed the
winit/wayland half of the Linux dependency surface. Two capabilities that
lived in egui-coupled code were rehomed rather than lost: the log collector
(now `client/src/logbuf.rs`, a bounded `tracing` layer) and the design-token
drift guard (now `desktop/src/lib/tokens.test.js` for the CSS, plus
`token_drift_tests` in `client/src/tui/overlays.rs` for the TUI accent).

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

The rail below is the original plan. As shipped it reads **Chats · Communities ·
Relays · Contacts · Settings** — Contacts earned a slot of its own, and "Party"
survived only as the protocol and code name.

```text
┌───────────┬────────────────────────────────────┐
│ Chats     │                                     │
│ Communit. │   (active tab's content)            │
│ Relays    │   Communities: servers → channels · │
│ Contacts  │     DMs · members · files · roles   │
│ ───       │                                     │
│ ⚙         │                                     │
└───────────┴────────────────────────────────────┘
```

Identity/keys/local vault fold into **settings** rather than being a headline tab.
Advanced surfaces sit behind an "advanced" affordance.

As shipped, the desktop rail reads **Chats · Communities · Relays · Contacts ·
Settings** — "Party" survived only as the protocol/code name (`core::party`,
`party_*` commands), and Contacts earned a rail slot of its own. This document
keeps saying "Party" for the protocol and server; the user-facing word is
"Communities".

---

## 4. Architecture

The workspace (already in place) lets the client and server share code yet ship as
one product:

```text
core/             crypto, protocol/wire types, framing, identity, transport, shared
                  types, and the Party application protocol (core::party). Reused everywhere.
client/           app core (ChatManager, PartyManager, persistence, diagnostics) + the
                  ratatui TUI. No GUI toolkit: the desktop app links this as a library.
server/           the Party server: TCP listener, PartyState, dispatcher, broadcast hub,
                  persistent identity, SQLite store, content-addressed blob store.
desktop/src-tauri the Tauri 2 shell (p2pem-desktop): #[tauri::command] bridge + event
                  pump over client's managers, wrapping the React/Vite UI in desktop/src/.
```

Relay stays a thin mode (`--relay-server`) alongside `core`.

**Binaries** ship from one repo/release, in two clearly-separated tiers:

- **P2PEM Desktop** (`p2pem-desktop`) — *the app*. Built by `tauri-action` into
  real per-OS installers (`.msi`/`.exe`, `.dmg`, `.deb`/`.AppImage`) and
  attached as the primary artifacts. The release body leads with a table telling
  the user which file to take.
- **P2PEM Tools** — one archive per OS containing the terminal client
  (`p2pem`, the `p2pem-classic` crate, which also runs a relay via
  `--relay-server`) and the community server (`p2pem-server`, the
  `messenger-server` crate). Secondary, and labelled as such.

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
  PostMessage, FetchHistory, SendDm, FetchDmHistory, CreateChannel, and the
  Phase 2 file trio PostFile, SendFileDm, DownloadFile); a
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
  `:party-status`); and the desktop **Communities** surface
  (`desktop/src/components/Parties.jsx` over the `party_*` bridge commands) with
  join form, server selector, channel + member lists, DM/channel views, a post
  box, and fingerprint pinning per saved community.

### Server data model (MVP)

In memory (authoritative at runtime), mirrored row-by-row to SQLite:

```text
PartyState { name, password: Option<String>, tier, members, channels, dm_threads,
             blobs, max_blob_bytes, db: Option<Connection>, blob_dir: Option<PathBuf> }
Member  { id, username, fingerprint: Option<String>, online }
Channel { id, name, kind, messages: Vec<Envelope> }   # messages = durable history
```

SQLite tables: `members`, `channels` (with a `position` column for stable
ordering), `messages`, `dm_threads`, `dm_messages` (envelopes stored as JSON),
and `blobs` (`hash, size, mime, refcount`; the bytes themselves live under
`<data_dir>/blobs/<hash>`).

### Roles, channel access, and the audit log (Phase 3 — shipped)

- **`Role`** (`core::party`) is ordered `Guest < Member < Admin < Owner`, so every
  server-side check reads as `role >= Role::Admin`. A **guest** is read-only
  everywhere; a **member** posts, uploads and creates public channels; an
  **admin** manages channels, anyone's files, and the roles below their own; the
  **owner** is the first identity to join — the operator starts the server and
  then joins it, which is the only bootstrap that does not require an admin to
  already exist. A role may only ever be granted *strictly below* the granter's
  own, the owner cannot be demoted, and a second owner is never minted, so an
  admin cannot take the community from the person running it.
- **`ChannelKind` is enforced.** It was previously stored, persisted, shipped to
  clients and checked nowhere; the interim fix made it fail closed, which left
  three of its four values unusable. Now: `Public` is read/write for everyone,
  `Locked` and `Announce` are readable by everyone and writable by admins, and
  `Private` is limited to the channel's own membership list (admins included, so
  they can moderate it). A private channel is filtered out of `ListChannels`
  entirely, so it does not advertise its existence to a non-member.
- **The channel list is therefore per member**, which is why it is no longer
  broadcast: the hub sends one identical frame to every connection, so
  broadcasting one member's view would either leak the private channels or hide
  them from the people in them. The server pushes `DirectoryChanged` and each
  client re-fetches its own.
- **The directory is live.** `Join` and disconnect both broadcast the refreshed
  member list; presence goes offline only when a member's *last* connection does,
  so a second open client does not report them as gone.
- **Audit log** — role changes, channel creation/deletion/access changes and file
  deletions are recorded with actor, action and detail, readable by admins only
  (the log says who did what to whom).

### What remains

- The E2EE tier (Phase 4) and per-server identities (Phase 5).
- Governance breadth beyond roles: transparency panel, consent-or-leave,
  visibility & contact policies, per-channel passwords.
- A per-file permission matrix (§8) — today's rules are role + channel kind.

---

## 8. Files / "Drive" (Phase 2)

**Shipped (slice 1):** content-addressed inline file sharing in the Party server.
A `MessagePayload::File(FileMeta)` carries a file reference in channel and DM
history; `PartyRequest::PostFile` / `SendFileDm` upload bytes inline (bounded by
`MAX_INLINE_FILE_BYTES = 4 MiB`) and post a file message that broadcasts like a
text message; `DownloadFile { hash }` returns the bytes. The server stores each
blob once, keyed by SHA-256, reference-counted, with the bytes on disk under
`<data_dir>/blobs/<hash>` and metadata (`hash, size, mime, refcount`) in the
`blobs` SQLite table. Downloads are access-checked
(`PartyState::blob_bytes_for(member, hash)` — channel members or DM participants
only), and total blob bytes are capped by an operator-adjustable server-wide
ceiling (`MAX_TOTAL_BLOB_BYTES`, 1 GiB by default) as a stand-in until real
quotas land. **Client wiring (done):** the desktop app can now share a
file into a channel or DM (a paperclip in the composer → `PartyManager::send_file`
/ `send_file_dm`, size-checked against `MAX_INLINE_FILE_BYTES`) and download a
received file (a file card → `PartyManager::request_download`, which correlates the
async `FileData` response by content hash and saves via a native dialog).
**Deletion, provenance, quotas and the Drive panel (shipped):**

- **`file_refs`** records every *share* of a blob — hash, display name, uploader,
  location (channel id or DM thread id), and when. It is deliberately separate
  from the message that references the blob: sequence numbers are what clients
  merge history on, so deleting a file must not remove an envelope and renumber
  the channel. `ListFiles` turns these rows into the Drive listing, filtered to
  what the caller may see, and access to a blob is decided over this table rather
  than by rescanning every message — so a deleted reference stops granting
  access immediately.
- **`DeleteFile`** drops one reference and reclaims the bytes (memory, row and
  file) when the last one goes. This is the half of reference counting that never
  existed: uploads incremented the count and nothing decremented it, so deleting
  was impossible and storage only grew. Deleting a channel releases everything
  shared in it, which would otherwise be stranded with nothing holding a count
  and nothing able to reach it. Allowed for the uploader or an admin.
- **Quotas are physical and logical.** `MAX_TOTAL_BLOB_BYTES` (1 GiB) bounds the
  server; `MAX_MEMBER_BLOB_BYTES` (128 MiB) bounds each member, because the
  server-wide ceiling alone lets the first member to reach it deny the feature to
  everyone else. Both count *distinct* content, so sharing one file into three
  channels costs its size once — and freeing it means deleting every reference.
  Admins are exempt: they are who clears space when it fills. `FetchQuota`
  reports used/limit for the Drive panel's readout.
- **Drive UI** — the desktop app's folder tab lists every visible file with its
  size, uploader, location and date, a per-member quota bar, download, and a
  two-click delete on the files the server says you may delete.

**Chunked transfer (shipped):** files past `MAX_INLINE_FILE_BYTES` are offered
with `StartUpload` (declaring their size, so the server refuses on quota,
ceiling or permission *before* any bytes move), streamed as `UploadChunk`s of
`PARTY_CHUNK_BYTES` (256 KiB), then committed with `FinishUpload`, which
verifies the assembled length before storing. Downloads mirror it with
`DownloadChunk`/`FileChunk` and are reassembled in order and checked against the
requested hash. `MAX_PARTY_FILE_BYTES` (100 MiB) is the hard ceiling;
`MAX_CONCURRENT_UPLOADS` bounds what one connection can hold, and a disconnect
discards its spools. Both front-ends pick inline or chunked by size — nothing in
the UI changes.

**Per-file permissions (shipped):** each share carries a
[`FilePermissions`] set — `view / download / delete / share` — kept genuinely
separate, because seeing that a file exists is not being able to fetch it, and
neither implies being allowed to put it somewhere else. Resolution is
location-scoped: an admin holds everything; anyone who cannot reach the
location holds nothing (uploaders included, so making a channel private does
not leave its files reachable by the people just excluded from it); within a
reachable location the uploader holds everything, an explicit per-member grant
overrides the location default, and otherwise the default applies. A guest
never holds a write right whatever they were granted. `SetFilePermissions` is
refused unless the caller's own rights cover what they are handing out, and
only the uploader or an admin may change a share's grants at all. `ShareFile`
makes the `share` right real: because content is addressed by hash, re-posting
a file you hold costs a reference rather than a transfer, and the new share
starts at the default — you pass on the file, not your authority over it.

**Remaining:** `Policy` (§9) — server-wide rules rather than per-file grants.

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
operator's data dir, plus a content-addressed filesystem blob store
(`<data_dir>/blobs/<hash>`) for file bytes — chosen for zero-ops single-host
hosting with room to swap later. The former interim JSON snapshot is imported
once on first load, then superseded. Of the objects listed above, `Blob`,
`FileEntry` (as `MessagePayload::File`), `FileReference` (the `file_refs` table),
`Quota` (server-wide *and* per-member), `Role`, `AuditLog` (the `audit` table)
and `PermissionMatrix` (`FilePermissions`, stored per share with per-member
grants) all exist. `Policy` is the one that remains design-only.

---

## 10. UI Rewrite (Tauri + React) — shipped

> **Status:** shipped as the `p2pem-desktop` crate (`desktop/`), and now the
> only desktop app — phases A–F are complete. This section is the design of
> record; the stack notes below record **what actually landed** (React/Vite, not
> the originally-planned SolidJS). Phase G (the TUI 3-pane redesign) remains.

The egui GUI it replaced was an accumulation, not a design: a top menu bar, a
status bar, a left "Chats" sidebar, a central panel, a pile of modeless dialogs
(the dialog file alone was ~84 KB), and — the clearest symptom — the **Party
experience in a separate floating window**, disconnected from everything else.
Users juggled two mental models. This rewrite realizes §3's tab-rail vision with
a designed UI.

### Target shape — a 3-pane shell

```text
┌──┬───────────────┬─────────────────────────────┐
│ R│   List pane   │      Content pane           │
│ a│ (chats /      │  (messages + composer, OR   │
│ i│  channels +   │   directory, OR a settings  │
│ l│  members /    │   page — no floating modals)│
│  │  relay sess.) │                             │
└──┴───────────────┴─────────────────────────────┘
 icon rail (top→bottom): Chats · Communities · Relays · Contacts · settings
```

- The **icon rail** replaces the menu bar and switches mode (Chats /
  Communities / Relays / Contacts); identity, lock, and settings dock at the
  bottom.
- The **list pane** is contextual to the mode.
- The **content pane** holds the conversation *or* a full-pane page for directory
  and settings — eliminating most modal dialogs.
- **Overlays only for genuinely interruptive flows**: fingerprint verification,
  password/unlock, consent-or-leave. Settings, Contacts, Connect, Host become
  inline pages.
- **Party stops being a floating window** and becomes the Communities tab, sharing
  the same list+content layout as chats. One mental model.

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
pure-Rust repo; the ratatui TUI stays Rust; the security surface grew by a system
webview + IPC (crypto stays in Rust; the CSP in `tauri.conf.json` is restrictive,
no external hosts). Packaging has since been rebuilt around `tauri-action`, and
egui is deleted — so there is one desktop app, not two.

### The Rust↔web bridge

The business logic is already view-agnostic — the TUI proves it by driving the
same managers as the desktop app. The rewrite keeps the managers in Rust and
exposes them to the web UI:

- **Commands** (`#[tauri::command]`): each manager method the UI calls becomes a
  command — as shipped, `start_host` / `connect_peer` (both taking the optional
  connection password), `send_message`, `confirm_fingerprint`, `party_join`,
  `party_post`, `party_send_dm`, `party_create_channel`, `party_history`, … all
  registered through the shared `invoke_handler()` in `lib.rs`.
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
`client/`, which kept the migration reviewable and left the `client` binary
buildable throughout:

```text
core/ (unchanged)
client/ (app core + ratatui TUI; egui deleted in phase E)
desktop/
├── src-tauri/   Rust: #[tauri::command] bridge + event pump over client's managers (lib.rs)
└── src/         React/Vite app: rail / list / content + overlays + CSS design tokens
```

The `client` binary is the terminal UI (its retired `--gui` flag now exits with a
pointer to the desktop download rather than silently starting a different
interface). The desktop app runs with `cd desktop && npx tauri dev`.

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
theme names exist as `core::types::Theme` (Rust, for persistence); the ratatui TUI
does not implement per-theme rendering (terminal color-depth constraints make a
5-theme TUI disproportionate) but does use the brand accent for theme-neutral
chrome (active-pane borders, key hints) — see `client/src/tui/overlays.rs`'s
`BRAND_ACCENT`.

There is no automated cross-language token pipeline, so **two drift guards** keep
`design/tokens.json` honest against what actually ships:

- `desktop/src/lib/tokens.test.js` parses the token file and the shipped CSS and
  asserts every theme's surface ramp, accent, ink, and the semantic colours
  match — plus that the theme registry and the token file list the same themes.
- `token_drift_tests` in `client/src/tui/overlays.rs` asserts the TUI's
  `BRAND_ACCENT` equals `brand.flatAccent`, and that the safety-grid palette
  (`client/src/colorgrid.rs`) is unchanged — it is a security signal users
  compare across devices, so it must not drift between versions.

These replace the equivalent Rust guard that lived in the egui theming module
before phase E removed it.

### Packaging rebuild — done (phase F)

`release.yml` now publishes **one product with one obvious download**:

- `tauri-action` builds P2PEM Desktop per OS with Tauri's own bundler
  (`.msi`/`.nsis`, `.dmg` for both Apple architectures, `.deb`/`.AppImage`) from
  `desktop/src-tauri/tauri.conf.json` (productName `P2PEM`, identifier
  `com.chat-p2p.p2pem`, restrictive CSP). The frontend is rebuilt in the job, so
  a stale committed `desktop/dist/` can never ship.
- A secondary **P2PEM Tools** archive per OS carries the terminal client and the
  community server for self-hosters.
- The release body is composed *before* the artifacts upload, so the page opens
  with a table saying which file to take.

The Inno Setup script (`setup.iss`) and `build-and-package.ps1` — both of which
hardcoded the egui binary — are deleted. `ci.yml` runs the frontend test + build
job and no longer installs the winit/wayland packages egui needed.

### Phased execution

Sequenced so each phase was reviewable and the shipped binary kept building
throughout. Phases A–F are complete:

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
- **E — Delete egui. ✅** `client/src/gui/` and the `eframe`/`egui`/
  `egui_commonmark`/`egui_tracing` dependencies are gone, along with the
  now-unused `rfd`/`emojis`/`windows-sys` entries. `cargo tree` shows no egui
  anywhere in the workspace. Two capabilities were rehomed rather than dropped:
  the log collector (→ `client/src/logbuf.rs`) and the token drift guard
  (→ the two tests described above). `colorgrid` now emits plain `(r,g,b)`
  instead of egui `Color32`, so the terminal UI no longer depends on a GUI
  toolkit to draw a safety grid. `--gui` exits with a pointer to the desktop
  download instead of silently starting a different interface.
- **F — Packaging + docs. ✅** `release.yml` rewritten around `tauri-action` with
  a self-explaining release body; `setup.iss` and `build-and-package.ps1`
  deleted; `ci.yml` runs the frontend job and dropped the egui-only Linux
  packages; webview/CSP and bridge-test notes in `SECURITY.md`; docs refreshed.
- **G — TUI 3-pane redesign. ☐ remaining.** Apply the rail/list/content model to
  ratatui, preserving the typed command language.

---

## 11. Roadmap

Each phase gets its own detailed plan before code lands.

```text
Phase 0  Workspace refactor                         ✅ done
UI       Tauri + React desktop app (§10)            ✅ shipped (A–F) · TUI redesign (G) remains
Phase 1  Party Server MVP (Administered)            ✅ complete (Communities pane in both UIs)
Phase 2  Drive / files                              ✅ done
Phase 3  Governance & roles                         ◐ roles, channel access, audit
                                                      log done · policies remain
Phase 4  E2EE server tier
Phase 5  Per-server identities

Independent:  P2P connection passwords + conversation lock   ✅ done
```

- **Phase 0 — Workspace refactor. ✅** Split into `core`/`client`/`server` with no
  behavior change; CI moved to `--workspace`; packaging untouched (binary name
  preserved). Test coverage broadened (see §13).
- **Phase 1 — Party Server MVP (Administered). ✅ complete.** Server binary,
  join-by-address (+ optional password) + username, member directory + presence,
  channels, server-routed group + DM messaging, offline buffering via history
  catch-up, channel creation, the desktop **Communities** surface + TUI commands
  and pane, server-identity TOFU on the wire (pinned per saved community, with a
  first-join verification prompt), roles and per-channel access, and
  SQLite-backed durable state (`party.db`).
- **UI rewrite — Tauri + React. ✅ shipped (A–F).** Landed as the `desktop/`
  crate with P2P, Party, Relay, Contacts, and Settings at parity; egui deleted
  (E) and the release pipeline rebuilt around it (F), so there is one desktop
  app. Full plan and stack notes in §10. Remaining: the TUI 3-pane redesign (G).
  Shipped in **1.15.0**. The owner chose a minor bump; note that the shipped
  desktop artifact changed name and installer, so existing installations of the
  retired GUI do not upgrade in place and have to be replaced manually.
- **Phase 2 — Drive / files. ✅** Content-addressed sharing in channels & DMs
  (inline below 4 MiB, chunked above it up to 100 MiB), working reference
  counting with deletion and reclamation, `file_refs` provenance, physical +
  per-member logical quotas, the Drive panel in both front-ends, and per-file
  permissions with re-sharing (see §8).
- **Phase 3 — Governance & roles. ◐ roles and audit done.** `Role`
  (Guest/Member/Admin/Owner) is enforced server-side, `ChannelKind` is a real
  access rule rather than a stored decoration, and an admin-only audit log
  records role, channel and file actions (see §7). **Remaining:** trust-tier
  labeling, transparency panel, consent-or-leave, visibility & contact policies,
  per-channel passwords.
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
- Tighten persistence, migration, and recovery behavior. **Partly done in
  1.15.0:** identity and history writes are atomic + `fsync`ed, an unreadable
  identity is a hard explained error instead of a silent regeneration, a failed
  plaintext-history migration no longer leaves messages in the clear, and the
  desktop app offers a first-run identity backup. Still open: the
  whole-file re-encrypt on every history change, and a real schema/migration
  story for the history format.
- Improve diagnostics and failure visibility for users. **Partly done:**
  `export_diagnostics` writes a bundle from the desktop app, and the boot screen
  reports startup failures with retry. Still open: making the bundle easy to
  attach to a bug report, and TUI parity.

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

### Cryptographic hardening roadmap (post-audit)

A security review rated the transport's *design* (not its implementation
bugs) as the main gap. Concrete implementation issues found in that review are
**already fixed** (documented in `CHANGELOG.md` / `SECURITY.md`): the
large-text `total_chunks` remote-OOM, the rekey desync (single deterministic
initiator + bounded dual-key receive window), and removal of the unused RSA
decryption path (keeping RUSTSEC-2023-0071's target operation out of the
product). What remains below are the **architectural** improvements — ordered
by value-per-effort. None is a quick patch; each deserves its own PR with a
threat-model note and dedicated tests. Written down now so we don't lose the
plan even if we implement it "when we have extra time."

**A. Ed25519 identity migration — see the dedicated plan above.**
*Priority: high · Effort: medium.* Removes the `rsa` crate from the
security-critical path entirely (RSA is currently signing-only, so the Marvin
advisory doesn't apply to a live operation, but dropping the dependency closes
the audit finding for good). Back-compatible through step 3; the fingerprint
cutover (step 4) is the only breaking part.

**B. Double Ratchet for post-compromise security (PCS).**
*Priority: high (it's the headline crypto gap) · Effort: large.* Today forward
secrecy comes from the ephemeral X25519 handshake plus a **symmetric** rekey
(`next = HKDF(current, nonce)`). Because rekeying folds in **no new DH
material** and the nonces travel on the wire, an attacker who captures one live
session key can follow the ratchet forward until the session ends and a fresh
handshake runs — i.e. **no self-healing after compromise**. The fix is a
Signal-style Double Ratchet:
- **Symmetric-key (chain) ratchet** per direction: a sending chain and a
  receiving chain, each advanced by a KDF per message, giving per-message keys
  (we already have per-message AEAD nonces; this adds per-message *keys*).
- **DH ratchet**: each side ships a fresh ratchet public key; whenever a new
  one is seen, both sides do a DH and derive a new root key that reseeds the
  chains. This is what injects new entropy so a compromised key stops helping
  the attacker after the next round trip.
- **Skipped-message keys**: cache out-of-order/skipped message keys (bounded)
  so reordering/loss doesn't wedge the chain. Our transport is TCP-ordered so
  this is simpler than Signal's, but the bound still matters for a Rekey-style
  transition.
- **Header handling & wire format**: a new `ProtocolMessage` (or an extension
  of `Rekey`) carries the ratchet public key + message number; symmetric
  encode/decode + replay-protected sequencing, gated behind protocol-version
  negotiation so old peers still interoperate during rollout.
  Migration: negotiate at handshake (`PROTOCOL_VERSION` bump); peers that both
  advertise the double-ratchet version use it, otherwise fall back to the
  current session-rekey scheme. Requires the two-peer old/new test matrix and,
  ideally, test vectors cross-checked against a reference implementation. This
  is the single change that most raises the product's security rating; it is
  deliberately **not** rushed.

**C. TOFU key transparency / stronger verification.**
*Priority: medium · Effort: medium→large.* TOFU pins on first use and shouts on
a key change, but nothing detects a malicious key presented *consistently* from
the very first contact, and out-of-band fingerprint comparison is friction
users skip. Incremental, independently shippable improvements:
- **Short Authentication String (SAS) / verified-session flow. ✅** A
  transcript-bound SAS (six digits + three emoji, `derive_sas` in
  `core/src/core/crypto.rs`) rides the TOFU confirmation events and leads the
  verification prompt in both UIs, so users compare a short code instead
  of 64 hex chars; an active MITM's two handshakes yield two different codes.
  Still open: promoting a compared SAS to a persisted "verified" trust state
  (the trust-state field already exists).
- **Safety-number change surfacing**: make a fingerprint change a first-class,
  sticky UI event with a re-verify flow (not just a toast).
- **Key transparency (full)**: an append-only, auditable log (CONIKS/Keybase
  style) so a victim can detect a key swap without an out-of-band channel. This
  needs *infrastructure* (a log server + gossip/audit), so it is a larger,
  likely post-2.0 effort; SAS is the pragmatic near-term step.

**D. X25519 contributory-behaviour hardening. ✅**
*Priority: low · Effort: small.* Done: `parse_x25519_public` rejects the
all-zero key, and `derive_session_key` rejects a non-contributory shared
secret (`was_contributory()`), covering the low-order points. Cheap
defense-in-depth / standard hygiene (RFC 7748 §6.1).

**E. Invite revocation.**
*Priority: low · Effort: medium.* Signed invites now **expire** (30 days) but
cannot be **revoked** before expiry. Options range from short-lived invites
with easy reissue (no infra) to a revocation list distributed via the relay or
Party server (infra). Start with the former; document the latter.

**F. Metadata-resistance (long-horizon).**
*Priority: deferred · Effort: very large.* Onion routing / anonymity layer and a
post-quantum handshake migration (hybrid X25519 + ML-KEM) are tracked as
long-horizon gaps (see below), not scheduled.

**Connectivity**

- Internet connectivity: **UPnP + NAT-PMP port mapping is shipped** (opt-in;
  the host asks the router to forward the listening port — UPnP/IGD first, then
  NAT-PMP — falling back to LAN/relay), and **invites are multi-address**
  (payload v4: external + LAN candidates in priority order, tried in turn by
  the connecting peer with a bounded per-attempt timeout; back-compatible both
  ways with pre-v4 invites/clients). **TCP hole punching is shipped**: the
  relay rendezvous coordinates a simultaneous open between punch-capable
  peers (observed public endpoints + LAN candidates, reused source ports,
  token-tag validation, deterministic socket selection) so relay sessions go
  direct whenever the NATs allow it, bridging only as a fallback
  (`core/src/network/punch.rs`; back-compatible both ways with pre-punch
  peers/servers). Still open: PCP gateways and hard symmetric-NAT/CGNAT
  pairs (the bridged relay remains the answer there).

**UX**

- Accessibility pass over interactions and color usage.
- Settings IA cleanup / tabbed organization (folds into the §10 settings page).
- Better contact management UX and trust-state workflows.
- File-transfer **progress** shows in both UIs, and **cancellation is
  shipped ✅**: a `ProtocolMessage::FileCancel` wire frame (binary tag 12,
  replay-protected) lets either side abort. Sends now stream from a background
  task (so a multi-gigabyte send neither blocks the manager lock nor buffers
  eagerly) and stop mid-flight on cancel; the receiver discards its partial
  temp file. Both directions are tracked (`TransferDirection`) and cancellable
  from the TUI Transfers overlay (↑/↓ select, `c` cancel) and the desktop
  transfer cards.

**Tracked long-horizon gaps** (intentionally not described as "done" anywhere):
onion routing / anonymity layer, post-quantum migration, hardware-backed identity.

---

## 13. Verification & Test Coverage

The `core` + `client` + `server` crates pass **457 automated tests**
(`cargo nextest run -p messenger-core -p p2pem-classic -p messenger-server`).
`--workspace` adds the **17 desktop-bridge tests**, which CI runs on Linux and
macOS only (skipped on Windows, where a Rust test harness linking Tauri aborts at
startup, and unbuildable without the GTK/WebKit dev packages). The frontend adds
**37 tests** (`cd desktop && npm test`). Together they span unit, integration,
and end-to-end suites. Counts drift with every change; re-measure rather than
trusting these numbers:

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
- **Party files** (`server/src/state.rs`): content dedup, the storage ceiling,
  filename sanitization against path traversal, download access control (channel
  member vs DM participant), and blobs + file messages surviving a reload.
- **Relay** (`core/tests/relay_e2e.rs` + in-file): hole-punched direct sessions
  and bridged fallback end-to-end (full handshake + message on both paths), the
  punch engine (loopback punch, token-mismatch rejection, candidate filtering),
  mixed punch-capable/legacy pairings, and a legacy-server emulation proving
  new clients silently re-register in legacy mode.
- **Types** (`core/src/types.rs`): `Config` defaults (privacy-conservative), serde
  round-trips, backward-compatible deserialization.
- **Identity / persistence**: encrypted identity storage, encrypted-history
  round-trip, wrong-key rejection, corrupt-file handling, format auto-detection.
- **ChatManager** (`client/tests/feature_coverage_tests.rs` + in-file): group
  chats, rename/delete, history clearing, toast lifecycle, file-transfer state,
  typing indicators, contact import/association, invites (v1–v4 + expiry and
  tamper rejection), invite-QR generation.
- **Transfers & receipts** (`client/src/app/chat_manager`): the acceptance gate
  (`AwaitingAcceptance` → accept/reject), either-direction cancellation, and a
  sent message marked delivered by the peer's `Ack`.
- **TUI**: command parsing, focus cycling, message round-trip, multi-chat
  isolation, typing flow.
- **Unread & notifications** (`core/src/types.rs`, `client/src/app/chat_manager`):
  the persisted read mark (back-compat with pre-`read_count` history, own
  messages never badging, saturating on a trimmed history) and the notification
  focus gate (silent only for the conversation actually on screen, session-id
  resolution for incoming connections, notify-by-default when a front-end never
  reports presence).
- **Desktop bridge** (`desktop/src-tauri/src/tests.rs`, Linux/macOS CI): the auth
  barrier, the core-enforced password floor, unread reported from the persisted
  read mark, and every frontend payload key binding to its command parameter.
- **Frontend** (`cd desktop && npm test`): password policy, safety-grid colours,
  Communities unread accounting, theme registry, and design-token drift against
  `design/tokens.json`.

The one area not deeply automated is webview pixel rendering; the logic behind it
(`ChatManager`, the bridge, and the frontend's pure modules) is covered directly.
Per-phase verification targets:

- **Phase 1**: two clients join over LAN, appear in the directory, exchange channel
  + DM messages; a message sent while a peer is offline is delivered on reconnect.
  (Met by the in-memory E2E tests in `server::connection` + client integration.)
- **UI rewrite**: per-phase gates in §10. Met: both toolchains build in CI; P2P
  and Party flows work end-to-end through the new UI; **no `egui`/`eframe`
  symbols remain anywhere in the workspace** (`cargo tree` is the check);
  `tauri build` produces installers on all three OSes via `tauri-action`.
- **Phase 2**: dedup, the storage ceiling, and download access control are
  covered (above); still to write — refcount-on-delete and per-user quota tests.
- **Later phases**: governance consent-flow tests; E2EE group-key rotation tests;
  render-no-panic tests for new UI.

---

## 14. Status

Phase 0 (workspace) and the Phase 1 Party server core are complete; the workspace
test suite is green. The **Tauri + React desktop app** (§10) is now *the* app:
phases A–F have shipped, egui is deleted, and the release pipeline publishes one
clearly-labelled product. The Independent P2P hardening (connection passwords +
conversation lock) shipped.

The honest remaining gaps, in the order they cost the most:

1. **No mobile client.** The single biggest structural gap against any
   mainstream messenger. Tauri 2 supports mobile targets, and the bridge is
   already the only UI-coupled layer, so the path exists — but it is a project,
   not a follow-up.
2. **No offline delivery for direct P2P.** Both peers must be online at once.
   Only the community server buffers. Store-and-forward without a trusted server
   is the interesting design problem here.
3. **Every WAN path requires someone to self-host** (a port forward, UPnP, or a
   relay). There is no operated infrastructure, by design and by resourcing.
4. **No third-party audit.** The posture in `SECURITY.md` is self-assessed and
   now says so out loud.
5. **Phase G** — the TUI 3-pane redesign.

This document is the approved north star; each phase gets its own detailed plan
before implementation.

