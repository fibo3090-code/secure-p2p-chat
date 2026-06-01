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

Out of scope for the MVP (later sub-phases / phases): server-routed DMs, roles &
permissions, governance/audit, files/Drive (Phase 2), E2EE (Phase 4).

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

2. **Handshake reuse (next slice).** Extract the v3 handshake from
   `run_host_session_over_stream` into a reusable `establish_*` step in `core` that
   returns `(cipher, transport_aad, peer_fingerprint, stream)`, guarded by the
   existing handshake tests, so the server can run the handshake and then a *Party*
   message loop (instead of the P2P `ProtocolMessage` loop). Security-sensitive —
   done carefully with the handshake tests as a safety net.

3. **Server runtime.** TCP accept loop → per-connection handshake → Party message
   loop driving `PartyState`; persistence via embedded SQLite + filesystem blob
   store under the operator's data dir.

4. **Client Party tab.** Connect-to-server flow (address + password + username),
   server-identity TOFU, member list, channel view; GUI + TUI.

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

Slice 1 (protocol + state foundation) is implemented and tested. Slices 2–4 follow.
