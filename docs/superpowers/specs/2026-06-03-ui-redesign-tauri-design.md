# UI Redesign → Tauri + React Web Frontend — Design

**Status:** shipped (phases A–D) · **Date:** 2026-06-03 · **Updated:** 2026-07-03

> This design shipped as the `p2pem-desktop` crate (`desktop/`): React 19 + Vite,
> plain CSS, `lucide-react` icons, driving the same `ChatManager`/`PartyManager` over
> a `#[tauri::command]` bridge. P2P, Party, Relay, Contacts, and Settings reached
> parity; retiring egui and rebuilding the release packaging remain. The canonical,
> maintained view is `docs/05_platform_spec.md` §10 — treat this file as the
> original design record.

## Summary

Replace the egui desktop GUI with a **web frontend (React) running in a Tauri 2
webview**, driven by the existing Rust core over a command/event bridge. The
React mockup (`P2PEM (standalone)`) becomes the real frontend. The TUI and the
UI-agnostic `ChatManager` are unchanged; egui is kept working until the web UI
reaches parity, then retired.

This redesign also re-frames the app's core navigation model so the
"server / relay / p2p" surfaces stop being *pseudo-separated, pseudo-unified*.

## The conceptual model (the heart of the redesign)

A conversation is described by **two orthogonal axes**:

- **kind** — `dm` (1:1) · `group` (flat, 3+) · `community-channel`
- **transport** — `direct` (a peer exposes a port) · `relay` (a blind broker
  box, zero retention, set up in advance, solves port-forwarding) · `server`
  (a persistent box that terminates the tunnel and holds channels/history)

Support matrix (✅ now · 🔜 future · — n/a):

| kind ↓ / transport → | Direct P2P | Relay | Server |
|---|---|---|---|
| **DM (1:1)** | ✅ | ✅ | — |
| **Group (flat)** | 🔜 P2P mesh | 🔜 relay fan-out | ✅ |
| **Community (channels)** | — | — | ✅ |

Key insight that resolves the original confusion: **relay is a *transport*, not a
destination.** `core/network/relay.rs` is a blind byte-pipe (`copy_bidirectional`)
that forwards an already-E2EE v3 session — it never sees plaintext and cannot
retain data. So a relayed DM is still a DM (badged `via relay`), and the
"mini-server that doesn't retain data" the user pictured is really a **server with
`retain_history = off`**, not a beefed-up relay. Three trust models, three
programs; relay sits as plumbing under the conversation layer.

## Information architecture

**Left rail** (identity avatar in the foot):

- **💬 Messages** — the home. One unified, WhatsApp-style list of *all*
  conversations (DM, group, channel), each with a **type badge** + **transport
  glyph** (🔌 direct · 🛰️ relay · 🖥️ server) + trust state (✓ verified / ⚠ unverified).
- **👥 Communities** — server-level org view (your servers, member directory,
  channel tree). Individual channels also appear in Messages (WhatsApp pattern).
- **👤 Contacts** — address book.
- **⚙️ Settings** — includes **"My infrastructure"**, which absorbs the mockup's
  `RelayPane` dashboard (run-your-own-relay, token rotate, configured relays,
  active routes, event log). The workshop, not a room you talk in.

**Title bar:** brand + live connection status · a single **`+ New`** button ·
theme toggle · lock.

**The one `+ New` creator** (so every connection method has a home):

- **Message someone** → pick 1..N participants (3+ gated "coming soon") →
  transport sub-choice: *Direct (expose a port)* or *Via relay (relay address +
  password)*. Future connection methods slot in here as more transport options.
- **Create / join a server** → *Create* (name · `retain history on/off` ·
  password) or *Join* (address · password · username).

## Architecture

```text
React (Vite) ──Tauri commands──▶ Rust bridge ──▶ ChatManager (unchanged)
   webview   ◀──event stream───   (src-tauri)        │
                                                core / network / crypto / identity
```

- `ChatManager` stays the single source of truth (already egui-free).
- Bridge = **Tauri commands** (request/response: unlock, list conversations,
  send message, connect, create server, …) + an **event stream** that mirrors the
  existing `SessionEvent` poll loop (`Connected`, `MessageReceived`,
  `ShowFingerprintVerification`, `Disconnected`, …) pushed to JS.
- New conversation fields `kind` + `transport`, and a participant/session **set**
  per conversation (replacing "the one peer") so groups don't force a later
  refactor. Badges derive from these fields.

## Roadmap (decomposition — each phase gets its own plan)

0. **Stance:** keep egui working until parity, then retire it. No regression.
1. ✅ **Bridge foundation** — Tauri 2 app in the workspace; webview; command/event
   bridge wrapping `ChatManager`; placeholder frontend. **Thin vertical slice:**
   unlock identity → see the real conversation list → send/receive one message
   with a connected peer.
2. ✅ **Frontend + design-system port** — mockup React modules → real Vite/React
   project; rail, title bar, theme, unified Messages list + ChatPane on live data.
3. ✅ **Conversation model** — `kind` + `transport` on `Chat` (`core/types.rs`,
   serde-default); badges: `TransportBadge` (🔌 direct / 🛰️ relay / 🖥️ server) +
   kind chip in the conv list + chat header. Participant-set still future (Phase 6).
4. 🚧 **`+ New` creator** — Connect/Host/Invite wizard done; **relay-as-transport
   wired** (Direct/Via-relay sub-toggle → `host_via_relay`/`connect_via_relay`
   bridge cmds → `ChatManager::{start_host_via_relay,connect_via_relay}`). Verify
   modal exists. Remaining: dedicated Identity modal polish.
5. **Other surfaces** — Communities, Contacts, Settings + relocated
   My-infrastructure → reach parity → retire egui.
6. **Future** — P2P / relay group chats on the participant-set model.

## Phase 1 scope (next to plan)

**Goal:** prove the Tauri+core architecture end-to-end with the smallest honest
vertical slice, without disturbing egui/TUI.

In scope:
- Add a Tauri 2 app to the Cargo workspace (e.g. `desktop/` with `src-tauri`),
  building alongside `core`/`client`/`server`. egui app still builds and runs.
- A minimal bridge module exposing `ChatManager` via Tauri commands:
  `unlock(password)`, `list_conversations()`, `select_conversation(id)`,
  `send_message(id, text)`, plus an event stream emitting the mapped
  `SessionEvent`s to the webview.
- A deliberately minimal placeholder frontend (no design system yet): unlock
  screen → list of conversations → open one → send/receive text.
- Decide and document the workspace/process model (does Tauri reuse the
  `messenger-core` crate directly; where the tokio runtime lives; how identity
  unlock maps to the existing blocking-auth gate).

Out of scope for Phase 1: the design-system port, badges/transport fields,
the creator wizard, Communities/Contacts/Settings, retiring egui.

**Phase 1 done when:** from the Tauri window, a user unlocks their identity,
sees their real conversations, connects to a peer (via the existing connect
path), and exchanges a message — with egui and TUI still building and passing
`cargo test --workspace`.

## Open questions (resolve during Phase 1 planning)

- Crate layout: standalone `desktop` crate depending on `messenger-core`, vs.
  reusing parts of `client`. (client currently owns `ChatManager`,
  `party_manager`, persistence — likely the bridge depends on `client` as a lib.)
- Tokio runtime ownership under Tauri (Tauri has its own async runtime).
- Bundler/packaging implications for the existing `build-and-package.ps1`.
- Frontend toolchain: Vite + React (matching the mockup) — confirm during P2.
