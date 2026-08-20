# Asynchronous delivery without giving up end-to-end encryption

**Status: design sketch and requirements. Nothing here is implemented.**

§1–§6 sketch a design. §7 is the acceptance criteria — what this feature must
have to be considered finished rather than merely working. §8 lists what is
still undecided, and marks the two questions that cannot be deferred.

This document exists because the two message tiers each lack what the other has,
and no path currently connects them.

| | P2P DMs | Communities |
|---|---|---|
| End-to-end encrypted | yes — X25519 → HKDF → AES-256-GCM | **no** — the server stores `text: String` |
| Delivers when you are offline | **no** — both peers must be online at once | yes — durable server history |

Neither column is wrong on its own. The problem is that a user has to pick, and
the choice is between "private" and "works".

Two pieces of evidence, so this is not an impression:

- Searching the workspace for `offline_queue|store_and_forward|outbox` returns
  nothing. If the peer's app is closed, the message does not exist.
- `PartyState::post_dm(from: Uuid, to: Uuid, text: String)` — community direct
  messages are plaintext on the operator's disk. `TrustTier::E2EE` is declared
  in `core/src/party/mod.rs` and unimplemented; `Administered` is the default
  and the only tier that exists.

`docs/platform_spec.md` §11 Phase 4 plans an E2EE tier for **community
channels** (per-channel group keys). That is a real and separate problem. It
does not address DMs, which is where most private conversation actually happens
and where the async gap is total.

---

## 1. Three missing pieces, routinely conflated

"Async E2EE" is not one feature. It is three, and they are independently useful:

1. **Somewhere to leave a message** — a mailbox that holds bytes until the
   recipient collects them. Without this, nothing else matters.
2. **A key to encrypt to, chosen while the recipient is offline** — the current
   handshake derives a session key from a *live* X25519 exchange. There is no
   live peer to exchange with. This needs published prekeys (X3DH).
3. **A ratchet** — so that compromising one key does not decrypt everything
   before and after it.

(1) and (2) together are the minimum for the feature to exist. (3) is what makes
it worth calling secure, and it can follow.

## 2. What already exists and can be reused

More than it first appears:

- **Ed25519 identity keys** landed recently (`feat(crypto): Ed25519 identity
  proofs`). X3DH wants an identity key, and this is one. It can be converted to
  X25519 by the standard birational map, or a separate X25519 identity key can
  be published alongside it — the second is less clever and less likely to be
  got wrong.
- **`ProtocolMessage` is an append-only enum** and peers already drop unknown
  frames harmlessly (that is how `Ack` shipped compatibly). New frame types are
  cheap.
- **The relay server already exists**, is already deployed by anyone who wants
  reachability, and already handles rendezvous by token. It is already trusted
  with nothing — it bridges ciphertext it cannot read.
- **Content-addressed blob storage** with reference counting exists in the
  community server and is the right shape for mailbox entries.

## 3. Where the mailbox should live

Three options, and the answer is not obvious:

| Option | For | Against |
|---|---|---|
| Community server | Storage, roles and history already there | Couples private DMs to joining a community; the server is `Administered` by design |
| **Relay server** | Already exists, already deployed for NAT traversal, already blind to content, already the thing you run for reachability | Grows a stateful role it does not have today |
| New mailbox service | Clean separation | A fourth thing to write, deploy and keep alive |

**Leaning: the relay — but the recommendation is conditional, and the condition
is not small.** Holding sealed bytes is a modest extension of a component whose
job is already "help two people who cannot reach each other directly" and whose
security posture is already "sees only ciphertext".

The condition: today the relay is *stateless and disposable*. You start one, it
brokers a connection, and nothing is lost if it dies. A mailbox makes it durable
infrastructure — it needs storage, backup, eviction, and an uptime expectation,
and losing it means losing undelivered messages. That is a different operational
product, and §6 of the original draft listed it as a caveat without letting it
touch the recommendation here. It should: **if the relay is not going to grow a
persistence and operations story, it is the wrong host**, and a separate mailbox
service is the honest answer despite being more work.

## 4. Shape of the design

### 4.1 Prekeys

Each identity publishes to its mailbox server:

- **IK** — long-term identity key. Already have it.
- **SPK** — signed prekey, medium-term, rotated on a schedule, signed by IK.
- **OPK** — a batch of one-time prekeys, consumed one per new session.

A sender fetches `(IK, SPK, one OPK)`, runs X3DH, and derives an initial root
key with nobody else online. The OPK is what gives forward secrecy for the very
first message; running out of them must degrade to SPK-only rather than fail,
and must be visible rather than silent.

### 4.2 The mailbox is blind

The server stores a row of: **address**, **opaque ciphertext**, **timestamp**.
It must not learn the sender — sender identity goes *inside* the sealed
payload, not in the envelope. This is sealed-sender, and it is the difference
between "the operator cannot read your messages" and "the operator cannot read
your messages but knows exactly who you talk to and when".

Constraints that have to be designed in, not bolted on: a per-recipient storage
cap, a TTL after which undelivered messages are dropped, and a size cap well
under `MAX_PACKET_SIZE`. All three are the same class of limit the community
server already enforces (`MAX_TOTAL_BLOB_BYTES`, `MAX_MEMBER_BLOB_BYTES`) and
for the same reason.

### 4.3 Metadata is the honest weak point

A mailbox addressed by fingerprint leaks the social graph to its operator
through arrival and collection patterns, even with perfect content encryption.
Mitigations worth *considering* — not necessarily shipping:

- rotating per-pair mailbox addresses derived from a shared secret, so the
  address is unlinkable across messages
- fixed-size padding buckets
- batched, randomised collection rather than fetch-on-arrival

A small self-hosted mailbox will have weak anonymity properties regardless. The
right response is to document that plainly in `SECURITY.md` — the file already
has a "What This App Does Not Claim" section, and this belongs in it — rather
than to imply a protection the deployment cannot deliver.

### 4.4 The ratchet, and a problem it exposes

Today: one ephemeral X25519 exchange per session gives forward secrecy *between*
sessions, and `Rekey` rotates the key every 100 messages *within* one. There is
no post-compromise security: an attacker who takes a session key and stays
passive keeps reading until the session ends.

A Double Ratchet fixes that and is the natural fit once X3DH exists. Writing one
from scratch is not advisable; `vodozemac` (Rust, audited, from Matrix) is the
credible option to evaluate.

Adopting it exposes something worth fixing on its own merits: **`history_key` is
derived from the long-term private key** (`core/src/identity/`, hashing the
PKCS#8 DER). One stolen `identity.json` therefore decrypts every message ever
stored, forever. There is forward secrecy on the wire and none at rest. That is
a bigger practical weakness than anything in the transport, and it is
independent of everything else here.

### 4.5 The trust model breaks, and this is the hardest part

This app's entire verification story is **SAS on a live handshake**.
`derive_sas(transcript_material)` (`core/src/core/crypto.rs`) hashes the
transport AAD — the transcript of a handshake that *both parties just ran
together*. That is what makes it a MITM detector: an attacker running two
handshakes produces two different transcripts.

**An asynchronous first message has no such transcript.** The sender is
encrypting to prekeys fetched from a server, alone. There is nothing for the
recipient to compare against, and nothing to show the sender at send time.

This is not a UI gap. It is a hole in the property the app is built around, and
it has to be answered before anything is written:

- X3DH does produce a shared secret, so a SAS **can** be derived from it — but
  only once both sides have run it, i.e. after the recipient comes online. So
  the model changes from "verify, then talk" to "talk, then verify", and the UI
  must carry unverified-but-delivered as a first-class state rather than
  pretending it is either of the two states that exist today.
- `known_trusted` must not be relaxed to make this convenient. The rule that an
  invite link is not verification, and that only a chat-stored or
  `Verified`/`Trusted` contact fingerprint auto-accepts, is load-bearing and was
  a fixed bug once already.
- The mailbox operator must not be able to impersonate. Prekeys are fetched
  *from the operator*, so the operator can serve attacker-controlled prekeys.
  The signed prekey's signature by the identity key is what prevents this — and
  it only prevents it if the identity key was verified some other way. Which
  returns to the point above.

### 4.6 Multi-device is a fork in the road, not a later feature

If the mailbox deletes an entry when it is collected, single-device is baked
into the wire format. Retrofitting multi-device then means per-device sessions,
fanout at send time, and a different deletion rule — i.e. redesigning both the
mailbox and the ratchet, for everyone, with migration.

**This must be decided before stage 1**, and the honest options are:

1. Design fanout in now (each device its own identity-bound session, sender
   encrypts N times, mailbox holds per-device entries). Considerably more work,
   no regret later.
2. Commit permanently to one device, write it in `SECURITY.md` as a
   design limit, and never quietly change position.

Deferring the choice is choosing (1)'s cost with (2)'s constraints.

### 4.7 What the operator can do besides read

"Blind" is a confidentiality property. It says nothing about an operator who is
*active* rather than curious, and who can:

- **drop** messages — censorship that looks like the recipient being offline
- **withhold** and claim the mailbox is empty
- **replay** stored entries
- **correlate** deposit and collection timing into a social graph

Only some of these are preventable. Drops become *detectable* with an
authenticated per-conversation sequence number inside the ciphertext — the same
trick `send_framed`'s `FrameSeq` already plays on party frames, and the same one
the P2P message loop has used per-session since v3. Replay is rejected the same
way. Timing correlation largely is not preventable at this scale, and belongs in
`SECURITY.md` under what the app does not claim.

### 4.8 Files

Stage 2 below says "text only", which is a real limitation rather than a
simplification: file transfer is a headline feature of this app, and a mailbox
that cannot carry files means the async path and the live path do different
things. Files also cannot simply be inlined — `MAX_FILE_SIZE` is 10 GiB and no
mailbox should accept that. The plausible shape is a sealed *reference*
(content hash + a key) in the mailbox, with the bytes fetched on collection
under the existing chunked path, but this is unresolved and should not be
hand-waved.

## 5. Staging

Smallest useful increments, each shippable and testable alone:

| Stage | What | Why this order |
|---|---|---|
| 1 | Publish and fetch prekeys. No mailbox. | Pure key distribution, no storage, no new trust. Testable end to end. |
| 2 | **Blind mailbox on the relay, text only.** | This is the feature. Capped, TTL'd, sealed-sender. |
| 3 | Double Ratchet replacing the per-session schedule. | Needs stage 1. Independent of stage 2. |
| 4 | Separate `history_key` from the identity key. | Standalone win; do it whenever. |
| 5 | Community E2EE tier (spec Phase 4) reusing stage 1. | Group keys are much easier once prekey distribution exists. |

Stage 2 is where a user notices anything.

## 6. What this does not solve

- **Group DMs and multi-device are covered above and below respectively** — see
  §4.5, which is where multi-device actually belongs; it is not a
  "does not solve", it is a decision that has to be made first.
- **Group DMs.** `ChatKind::Group` appears exactly twice in the workspace, both
  in a serde round-trip test. It is a type, not a feature.
- **The mailbox being available.** Store-and-forward turns an optional relay
  into infrastructure that has to stay up.

## 7. Requirements

What follows is the acceptance criteria. **"Perfect" is defined narrowly and
testably: there is no known decision a future maintainer would have to undo, and
every accepted limitation is written down rather than discovered.** A feature
that is merely working is not perfect; a feature that is working and whose
compromises are all deliberate and documented is.

Requirements are `MUST` (perfect requires it), `MUST NOT` (perfect forbids it),
or `SHOULD` (perfect prefers it, and its absence must be justified in writing).

### R1 — Trust

| | Requirement |
|---|---|
| R1.1 | An asynchronously delivered first message `MUST NOT` be presented as coming from a verified peer. |
| R1.2 | The UI `MUST` carry "delivered but unverified" as a first-class state. Today only "verified" and "awaiting verification" exist, and neither is honest here. |
| R1.3 | A verification path `MUST` exist that does not require the original live handshake — a SAS over the X3DH transcript, computed once both sides have run it. |
| R1.4 | `known_trusted` `MUST NOT` be widened to auto-accept a fingerprint on the strength of a mailbox delivery. An invite link is not verification; neither is a message arriving. |
| R1.5 | A malicious mailbox operator serving forged prekeys `MUST NOT` be able to impersonate a contact whose identity key the user has verified. |
| R1.6 | Prekey bundles `MUST` be signed by the identity key, and the signature `MUST` be checked before any content is encrypted to them. |

### R2 — Confidentiality and secrecy over time

| | Requirement |
|---|---|
| R2.1 | The mailbox `MUST NOT` hold plaintext, nor any key from which plaintext is derivable. |
| R2.2 | Every message `MUST` be encrypted under a key that is **not** derivable from the long-term identity key alone. This also closes the `history_key` weakness in §4.4. |
| R2.3 | Forward secrecy `MUST` hold per message after session establishment. |
| R2.4 | Post-compromise security `MUST` hold: a passive attacker holding one message key loses access within a bounded number of messages. |
| R2.5 | The first message `MUST` have forward secrecy when a one-time prekey is available. |
| R2.6 | When one-time prekeys are exhausted, the session `MUST` degrade to signed-prekey-only rather than fail, and the degradation `MUST` be visible to both users — not silent. |
| R2.7 | Key derivations `MUST` bind the full transcript, as the v3 handshake already does. |

### R3 — Metadata

| | Requirement |
|---|---|
| R3.1 | The mailbox envelope `MUST NOT` name the sender. Sender identity goes inside the sealed payload. |
| R3.2 | Ciphertext `MUST` be padded to fixed size buckets, so length does not leak message size. |
| R3.3 | Mailbox addresses `SHOULD` be unlinkable across messages. If they are not, the social-graph leak `MUST` be documented in `SECURITY.md`. |
| R3.4 | Everything the operator can still observe `MUST` be enumerated in `SECURITY.md` under what the app does not claim. |

### R4 — Delivery integrity

| | Requirement |
|---|---|
| R4.1 | An operator dropping messages `MUST` be **detectable** — an authenticated per-conversation sequence number inside the ciphertext, as `FrameSeq` already does for party frames. |
| R4.2 | Replayed mailbox entries `MUST` be rejected. |
| R4.3 | Out-of-order collection `MUST NOT` lose messages or wedge a session. |
| R4.4 | A message the sender believes was delivered `MUST NOT` be silently lost by mailbox eviction; TTL expiry `MUST` be reported to the sender. |

### R5 — Abuse resistance

| | Requirement |
|---|---|
| R5.1 | Per-recipient storage `MUST` be capped, as `MAX_MEMBER_BLOB_BYTES` already caps party storage and for the same reason. |
| R5.2 | Deposits `MUST` be rate-limited per sender and per IP, reusing `network::ratelimit`. |
| R5.3 | Deliberate one-time-prekey exhaustion by an attacker `MUST NOT` degrade a victim's secrecy silently — see R2.6. |
| R5.4 | There `MUST` be a policy for unsolicited mail from unknown identities, and its default `MUST` be the conservative one. |
| R5.5 | Entries `MUST` have a TTL and `MUST` be evicted on collection. |

### R6 — The two forks that must be closed before stage 1

| | Requirement |
|---|---|
| R6.1 | Multi-device `MUST` be decided before any wire format is frozen: fanout designed in, or single-device committed to permanently and documented. Not deferred. |
| R6.2 | Mailbox hosting `MUST` be decided with its operational story attached — the relay only if it grows persistence, backup and an uptime expectation. |

### R7 — Compatibility and operations

| | Requirement |
|---|---|
| R7.1 | Peers without the feature `MUST` continue to work. `ProtocolMessage` is append-only and unknown frames are dropped; new frames `MUST` keep that property. |
| R7.2 | `to_plain_bytes()`/`from_plain_bytes()` `MUST` stay symmetric. |
| R7.3 | The mailbox `MUST` be optional. With none configured the app `MUST` behave exactly as it does today. |
| R7.4 | Loss of the mailbox `MUST NOT` lose already-delivered history. |
| R7.5 | If `history_key` derivation changes, there `MUST` be a migration or an explicit, documented decision to leave old history on the old key. Silently making history undecryptable is the one unforgivable outcome. |

### R8 — Testability

The bar here is the one this repository already holds itself to, not a lower one.

| | Requirement |
|---|---|
| R8.1 | Every new derivation `MUST` have a frozen known-answer test, as `derive_sas` and `history_key` do. |
| R8.2 | An end-to-end test `MUST` drive a real mailbox server in-process against real client objects, in the style of `client/tests/party_e2e.rs`. |
| R8.3 | Every new parser `MUST` be covered by proptest in the stable suite (`core/tests/fuzz_parsers.rs`), which is what gates PRs. |
| R8.4 | A `cargo-fuzz` target `MUST` exist for the mailbox frame, alongside the three in `core/fuzz/`. |
| R8.5 | A test `MUST` assert the operator **cannot** read — driving the real server and asserting on what its state actually holds, not asserting that a function was not called. |
| R8.6 | The offline path `MUST` be tested end to end: recipient absent, message deposited, recipient returns, message collected and decrypted. |
| R8.7 | Every `MUST` above `MUST` have a test that fails if it regresses. A requirement with no test is a wish. |

### Definition of done

The feature is **perfect** when:

1. every `MUST` has a failing-on-regression test,
2. every `SHOULD` that was not met has a written justification,
3. every residual limitation appears in `SECURITY.md`,
4. no decision has been deferred that would later require a wire-format change,
5. and R6.1 and R6.2 were answered **before** implementation began, not after.

Anything less is fine engineering. It is just not perfect, and the difference
should be stated rather than blurred.

## 8. Open questions

Ordered by how expensive they are to answer late. The first two are **blocking**
— R6 says they must be closed before implementation, because both are frozen
into the wire format:

1. **Multi-device: fanout, or permanently single-device?** (R6.1) Changing
   position later means redesigning the mailbox and the ratchet, with migration,
   for everyone.
2. **Where does the mailbox live, and who keeps it up?** (R6.2) The relay only
   if it grows persistence and an uptime expectation; otherwise a separate
   service.

Answerable later without regret:

3. Rotating mailbox addresses from the start, or fingerprint-addressed first and
   accept the documented graph leak while the feature proves itself? (R3.3)
4. `vodozemac`, or a ratchet over the existing primitives?
5. Migrate existing history when `history_key` changes, or leave old history on
   the old key and start fresh? (R7.5 — either is acceptable; silence is not.)
6. How do files ride the mailbox? (§4.8 — sealed reference plus the existing
   chunked fetch is the plausible shape, but it is not designed.)

---

Related: `docs/protocol.md` (v3 handshake), `docs/platform_spec.md` §11
(phases), `SECURITY.md` (limits and open risks).
