# Phase 1 — Party Server MVP (Administered): Design & Plan

Implementation plan for **Phase 1** of the [platform spec](05_platform_spec.md):
a self-hosted **Party server** that non-technical users join with IP + optional
password + a username, then see a member directory and chat in channels, with the
server storing history so messages reach people who were offline. This is the
**Administered** tier (the server stores plaintext); the E2EE tier is Phase 4.

## Goals (MVP)

- A `messenger-server` binary the owner runs on their network.
- A client joins by address (+ optional server password) and picks a username.
- Member directory with presence.
- Public channels with server-routed messaging.
- Offline buffering: the server stores channel history; a reconnecting member
  fetches what they missed.
- The encrypted, authenticated transport reuses the existing Protocol v3 handshake
  (the server has its own identity, TOFU-verified by clients once).

Server-routed 1:1 **direct messages** are implemented (a deterministic per-pair DM
thread; the server stores and delivers them like channels, with offline history).
Out of scope for the MVP (later sub-phases / phases): roles & permissions,
governance/audit, files/Drive (Phase 2), E2EE (Phase 4).

## Layering

The Party protocol is a **shared wire contract**, so it lives in `core` (reused by
both client and server), riding *on top of* the v3 encrypted tunnel:

```text
client ───v3 handshake (core)───▶ encrypted+authenticated tunnel ───▶ Party protocol (core::party)
                                                                         │
                                                              messenger-server applies it to PartyState
```

## Build slices

This phase is delivered in safe, independently-testable slices:

1. **Protocol + state foundation (this slice).**
   - `core::party`: the Party application protocol — `TrustTier`, `Envelope`
     (`{ tier, sender, channel, seq, timestamp, payload }`), `MessagePayload`,
     `MemberInfo`, `ChannelInfo`, and the `PartyRequest` / `PartyResponse` message
     enums, with bincode (de)serialization and round-trip tests.
   - `messenger-server::state`: an in-memory `PartyState` holding the server config
     (name, optional password, tier), members, and channels, with pure methods:
     `join`, `members`, `channels`, `post_message`, and `history_since` (offline
     catch-up). Fully unit-tested. No network yet — deterministic and fast.

2. **Handshake reuse (done).** The v3 handshake was extracted from
   `run_host_session_over_stream` / `run_client_session_over_stream` into
   `host_handshake` / `client_handshake` in `core::network`, returning an
   `EstablishedTunnel { peer_fingerprint, peer_chat_id, cipher, transport_aad }`.
   The session functions now apply trust policy + the P2P loop on top; the server
   reuses the same handshake. No behavior change — the handshake unit tests, the
   A-Z session E2E, and the relay E2E all pass unchanged.

3. **Server runtime (in progress).** Done: `core::party::{send_framed, recv_framed}`
   (encrypted Party-message framing over the tunnel); `server::connection::serve_connection`
   (per-connection `host_handshake` → a `select` loop that serves requests via the
   dispatcher *and* writes pushed broadcasts, binding the handshake-verified
   fingerprint to the member); `server::hub::Hub` cross-connection **broadcast
   fan-out** (a posted message reaches every other connected member live); a
   **persistent server identity** (`server::identity` stores the RSA key as an
   owner-only PEM under the data dir, so the fingerprint clients pin is stable
   across restarts); a real TCP accept loop in `main`; and **durable state** —
   `PartyState::load`/`persist` auto-saves members + channels + history to a JSON
   snapshot under the data dir and restores it on startup (presence resets to
   offline), so the server survives restarts. **Note:** the snapshot is the interim
   durability mechanism; migrating it to the embedded **SQLite** + blob store the
   spec calls for (the `Snapshot` shape maps cleanly to tables) is a follow-up.

4. **Client Party UI (in progress).** Done: `client::app::party_manager::PartyManager`
   (per-connection read/write task, directory/channel/history tracking, optimistic
   post); TUI commands (`:party-connect` / `:party-post` / `:party-status`); and a
   **GUI Party window** (`gui::party_view`) — join form, server selector, channel +
   member lists, message view, and a post box. **Remaining:** a dedicated TUI Party
   pane (beyond command output), server-identity TOFU confirmation UI, and
   surfacing connect/post errors in the GUI.

## Data model (server, MVP)

```text
PartyState { name, password: Option<String>, tier, members: Map<Uuid,Member>,
             channels: Map<Uuid,Channel> }
Member  { id, username, fingerprint: Option<String>, joined_at, online }
Channel { id, name, kind, messages: Vec<Envelope> }   # messages = durable history
```

Persistence (slice 3) maps these to SQLite tables + a blob store; the in-memory
model is the source of truth at runtime.

## Verification

- Slice 1: `core::party` round-trip tests for every message; `PartyState` unit
  tests for join (open / correct-password / wrong-password / duplicate-username),
  posting + monotonic per-channel seq, and `history_since` offline catch-up.
- Slice 2: handshake-extraction must keep all existing `session.rs` handshake tests
  green and derive identical keys on both sides.
- Slice 3: integration test — two clients join over loopback, appear in the
  directory, exchange channel messages; a message posted while a peer is offline is
  delivered on reconnect via `history_since`.

## Status

Slices 1–2 are complete and slice 3 is well underway: a client can complete the v3
handshake to the server, join, post to a channel, fetch history, and — with the
broadcast hub — receive other members' messages live (verified by in-memory
end-to-end tests in `server::connection`). `main` runs a real TCP listener with a
persistent, owner-only server identity, and durable state (a JSON snapshot of
members/channels/history that survives restarts). The client can join and chat via
TUI commands and a GUI Party window. Remaining: a dedicated TUI Party pane, GUI
TOFU/error surfacing, and migrating the server snapshot to embedded SQLite + blobs.
