# Platform Spec: Hybrid Messenger

This is the canonical, forward-looking design spec for evolving the app from a
1:1 P2P messenger into a hybrid platform. Unlike [04_protocol.md](04_protocol.md)
(which describes only shipped wire behavior), this document owns the long-range
vision, architecture, trust model, and phased roadmap. Each phase gets its own
detailed spec → plan → build before any code lands.

## Context & Goals

Non-technical users (the motivating case: classmates) can't port-forward, so they
can't use today's pure-P2P app. The owner wants a **self-hosted central server**
that friends join with a simple flow — IP + optional password + pick a username —
then see a member directory, chat in channels and DMs, make groups, and share
files, including delivery to people who are currently offline.

Reframing: the classmate use-case is a **Party Server**, not the existing relay.
The relay only pairs two peers; it cannot host a multi-user room.

Owner decisions that shape this spec:

- **Spec-first.** Write the complete platform spec before building.
- **Two trust tiers.** An **Administered** (server-trusted) tier as the default,
  and an optional **E2EE** tier.
- **One cohesive product**, not glued-together pieces.
- **Single Discord-like tabbed UI** under one identity.

## Current State (audit)

- **P2P**: mature. v3 handshake (X25519 → HKDF → AES-256-GCM), TOFU fingerprint
  verification, forward secrecy, replay protection, rekey, 1:1 text + up to 10 GiB
  file transfer, typing indicators, encrypted local history, signed invites.
  "Group chat" today is only client-side fan-out over multiple 1:1 sessions — there
  is no real server-backed group. No connection passwords or conversation locks.
- **Relay**: stateless 1:1 rendezvous (`src/network/relay.rs`). Pairs one host and
  one joiner by token and forwards ciphertext via `copy_bidirectional`. No auth,
  rooms, or storage. The `--relay-server` mode already ships (see `src/main.rs`,
  `Args.relay_server` → `network::run_relay_server`). **It cannot host a
  multi-user room.**
- **Party / Server**: does not exist yet. This is the new backend.
- **Crate**: the single crate is named `encodeur_rsa_rust` (see `Cargo.toml`).
  Today's modules (`src/lib.rs`): `app`, `core`, `gui`, `identity`, `network`,
  `support`, `transfer`, `tui`, `types`, `util`.

## Product Shape

One app, one identity, a left tab rail (Discord-like):

- **P2P** tab — direct encrypted DMs/calls between peers (today's strength).
- **Relay** tab — NAT-traversal helper for 1:1 P2P when neither side can
  port-forward.
- **Party** tab — the servers you've joined; each has channels, server-DMs,
  members, and files (the classmate experience).

The **Local** hub (identity, keys, local vault) folds into settings for the MVP
rather than being a headline tab. The UI is simplified by default; P2P/advanced
surfaces sit behind an "advanced" affordance.

```text
┌──────┬─────────────────────────────────────────┐
│ P2P  │                                          │
│ Relay│   (active tab's content)                 │
│ Party│   Party: servers → channels · DMs ·      │
│ ───  │          members · files · (governance)  │
│ ⚙    │                                          │
└──────┴─────────────────────────────────────────┘
```

## Architecture

Evolve the single `encodeur_rsa_rust` crate into a Cargo **workspace** so the
client and server share code yet ship as one cohesive product:

```text
workspace/
├── core/     crypto, protocol/wire types, identity, framing, shared domain types,
│             diagnostics/support. From today's src/core, src/identity, src/types,
│             src/util, src/support. Reused by both client and server.
├── client/   the one unified app: egui GUI + ratatui TUI, ChatManager,
│             persistence. From today's src/gui, src/tui, src/app.
└── server/   the Party server (new). Hosts accounts, channels, storage; run by the
              owner. Reuses `core` for the encrypted transport handshake.
```

Relay stays a thin mode (today's `--relay-server`), unchanged in spirit, living
alongside `core`/`server`.

**Binaries** ship from one repo/release so it never feels like separate projects:
a default `client` binary and a `server` binary (`--party-server`).

> **Phase 0 caveat — binary naming.** Today the binary is `encodeur_rsa_rust`, and
> that name is hardcoded in the packaging pipeline:
> `.github/workflows/release.yml` (Windows `.exe`, macOS bundle, Linux tarball),
> `setup.iss`, and `build-and-package.ps1`. Decide during Phase 0 whether to keep
> `encodeur_rsa_rust` or rename to `client`/`server`. If renaming, those three
> packaging files must be updated in the same change or releases break.

**Transport matrix**

- **P2P / Relay**: the existing v3 E2EE session, unchanged.
- **Client ↔ Party server**: the same v3 handshake establishes an authenticated,
  encrypted channel to the *server* (the server has its own identity/fingerprint,
  TOFU-verified once). A **Party application protocol** (login, directory, channel
  ops, messages, files) rides on top of that channel.

## Identity Model

- **Global private identity** (one per user): the existing RSA key material +
  fingerprint, password-encrypted at rest (`src/identity`). Never exposed
  wholesale.
- **Per-server identity** (Phase 5): a server-scoped profile (display name,
  visibility, and for E2EE servers a server-scoped keypair) bound to the global
  identity by a signature, so one person can present context-appropriate faces.
  The MVP uses the global identity with a per-server **display name**; the data
  model reserves room for distinct per-server keys from the start.

## Security & Trust Tiers

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
- **Private / E2EE (optional)** — a per-channel **group key**; the server stores
  only ciphertext and never sees plaintext; admins cannot read. Offline buffering
  stores encrypted blobs. Group-key distribution and rotation on membership change
  is the hard part (Phase 4).

Trust tier is a **server property**, shown prominently. Raising exposure (e.g.
enabling admin-read) triggers a **consent-or-leave** flow.

## Party Server Design

- **Accounts & membership** — a user joins a server (open / password / invite /
  request); the server stores membership + per-server profile.
- **Directory & presence** — list of members (subject to visibility policy),
  online/offline state.
- **Channels (rooms)** — public / private / locked / password / invite / request;
  plus admin-only (announce) channels.
- **Server DMs** — routed and buffered by the server (NOT P2P); per-user policies:
  open / request / password / closed.
- **Offline buffering** — the server holds messages/files until the recipient
  reconnects (plaintext for Administered, ciphertext for E2EE).
- **Roles & permissions** (Phase 3) — a role hierarchy plus per-channel /
  per-action grants.
- **Conversation lock / passwords** — lock = nobody can join; password = a gate to
  join a channel or open a DM.
- **Governance** (Phase 3) — server-wide policy is public to members; a
  transparency panel shows current policy + history; sensitive changes notify users
  and require accept-or-leave (with data export/delete on leave); an audit log
  records channel/role/permission/visibility/security changes, bans, and invites.

## Files / "Drive" (Phase 2)

Content-addressable, deduplicated storage:

- **Blob** — raw content stored once: `hash, size, mime, (ciphertext?)`.
- **FileEntry** — a logical file in the UI: `name, owner, server, location, date,
  meta`.
- **PermissionMatrix** — `view / download / delete / share / admin` (view ≠
  download ≠ share; share is explicit; you can only delegate rights you hold).
- **FileReference** — a specific share instance: where / which channel or DM / by
  whom / with which permissions. Blobs are **reference-counted** and deleted only
  when no reference or retention rule holds them.
- **Quotas** — physical (bytes stored, dedup-aware) and logical (per-user /
  per-role references); recommended: dedup physical storage + a logical quota per
  reference, freed on delete.
- **Provenance / audit** — who uploaded, where it was shared, what permissions
  traveled with it.
- **UI** — a Google-Drive-like panel: list, size, date, location, quota used/left,
  and download / delete / move / permissions actions.

## Data Model (objects, concise)

`Identity(global)`, `ServerProfile(per-server)`, `Server`, `Membership`,
`Channel`, `Message(envelope)`, `DMThread`, `Blob`, `FileEntry`,
`PermissionMatrix`, `FileReference`, `Quota`, `Role`, `Policy`, `AuditLog`.

**Server persistence**: embedded **SQLite** for metadata + a filesystem blob store
(both under the operator's data dir). Chosen for zero-ops single-host hosting, with
room to swap later.

## UI (unified, tabbed)

Left rail tabs: **P2P · Relay · Party**. The Party tab mirrors Discord: server list
→ channels + server-DMs + member list + files + (admin) governance/audit. The
fingerprint-verification, password, and transparency/consent flows are overlays
(GUI dialogs / TUI overlays — reusing the overlay patterns just built for the TUI,
see `src/tui/overlays.rs`). Every Party action is also reachable by command in the
TUI.

## Phased Roadmap

Each phase = its own spec → plan → build.

```text
Phase 0  Workspace refactor        (foundation; no behavior change)
   │
Phase 1  Party Server MVP          ← unblocks the classmates
   │     (Administered tier)
Phase 2  Drive / files
   │
Phase 3  Governance & roles
   │
Phase 4  E2EE server tier
   │
Phase 5  Per-server identities

Independent:  P2P connection passwords + conversation lock  (slot in anytime)
```

- **Phase 0 — Workspace refactor.** Split into `core` / `client` / `server` crates
  with **no behavior change**; CI, build, and packaging updated (mind the
  binary-naming caveat above). The foundation that lets the server share `core`.
- **Phase 1 — Party Server MVP (Administered).** Server binary with SQLite + blob
  store; join-by-IP (+ optional password) + username; member directory + presence;
  channels; server-routed group + DM messaging; offline buffering; the unified
  Party tab (GUI + TUI); server-identity TOFU. *This unblocks the classmates.*
- **Phase 2 — Drive / files.** Upload / list / download / delete in channels &
  DMs; hash dedup + reference counting; logical + physical quotas; the Drive panel.
- **Phase 3 — Governance & roles.** Trust-tier labeling, transparency panel,
  consent-or-leave, audit log, roles/permissions, visibility & contact policies,
  channel lock/password.
- **Phase 4 — E2EE server tier.** Per-channel group keys, ciphertext-only storage,
  key rotation on membership change, encrypted offline blobs; admin-read disabled
  for this tier.
- **Phase 5 — Per-server identities.** Distinct per-server profile/keys bound to
  the global identity.
- **Independent — P2P connection passwords + conversation lock.** Small; can land
  anytime.

## Per-Phase Verification

- **Phase 0**: `cargo build` / `cargo test` / `cargo clippy --all-targets -D
  warnings` / `cargo fmt --all -- --check` green across the `--workspace`; the
  existing app runs unchanged. (CI in `.github/workflows/ci.yml` must be updated to
  the workspace; builds must use a target dir outside OneDrive per project memory.)
- **Phase 1**: two clients join the owner-hosted server over LAN, appear in the
  directory, and exchange channel + DM messages; a message sent while a peer is
  offline is delivered on reconnect and the server stores it (Administered).
  Automated server-protocol tests + client integration tests.
- **Later phases**: dedup / refcount / quota unit tests; governance consent-flow
  tests; E2EE group-key rotation tests; render-no-panic tests for new UI.

## Status

This spec is the approved canonical north star for the platform direction. The
next action is to write the **Phase 0** (workspace refactor) implementation plan
before any code lands.
