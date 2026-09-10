# Asynchronous delivery without giving up end-to-end encryption

**Status: design sketch and requirements. Nothing here is implemented.**

**Start with "How it would work"** below — it is the whole feature in plain
terms, with no vocabulary you need to look up. §1–§6 are the same thing for a
reader who already has the vocabulary. §7 is the acceptance criteria: what this
must have to count as finished rather than merely working. §8 is what is still
undecided.

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

## How it would work, in plain terms

The rest of this document assumes you know the vocabulary. This section does
not. Two people, one mailbox.

Think of the mailbox server as a post office that holds **padlocks and boxes**,
never keys. Anyone can snap a padlock shut. Only its owner can open it.

### Setting up, once

When you turn the feature on, your app uploads three things:

- **your public ID** — the thing your fingerprint is made from
- **a stack of about a hundred one-time padlocks** — anyone may close one, only
  you can open it, and each is used exactly once
- **one spare padlock**, replaced weekly, for when the stack runs out

All of them carry your signature, so nobody can swap in fakes. The server is
holding padlocks, not keys, and can open nothing.

### Alice and Bob meet — exactly as they do today

They connect directly and compare the six digits and three emoji. Each app saves
the other's fingerprint. **This step does not change**, and it is what makes
everything after it safe.

### Bob's laptop is closed. Alice sends a message

1. Alice's app asks the mailbox for Bob's padlocks.
2. The server hands over Bob's public ID, one one-time padlock, and the spare.
   That one-time padlock is now used up and deleted.
3. **Alice's app checks Bob's public ID against the fingerprint it saved when
   they met.** A mismatch is refused outright — no prompt, no fallback.
4. It combines her key with Bob's padlocks to work out a secret only she and Bob
   can compute, and locks the message with it.
5. She deposits the locked box addressed to Bob. **The outside does not say
   "from Alice"** — that is inside, under the lock.

Step 3 is the entire security of this arrangement. The server can lie about
whose padlocks it just handed over, but Alice already knows what Bob looks like,
so the lie is caught immediately and without asking the user anything.

### Bob opens his laptop

6. His app asks whether anything is waiting, and collects the box.
7. His private key, plus the matching half of that one-time padlock, works out
   the same secret. The box opens.
8. Only now can he see it came from Alice.
9. His app deletes it from the server.
10. Every message after this changes the lock, so stealing one key does not open
    the others.

### If both are online

Nothing changes. They connect directly, exactly as today. The mailbox is only
ever a fallback.

### What the server can and cannot see

| Can see | Cannot see |
|---|---|
| Bob has a mailbox | What any message says |
| Something was left for Bob at 14:32 | Who left it — no name, only a code that changes daily |
| Roughly how big, padded into size buckets | Who that code belongs to, or what it was yesterday |
| When Bob collected | Anything at all, even if the disk is seized |
| That the same daily code left 11 things | |

This table is the part users read, so it is the part that must not overclaim. It
previously promised that the operator cannot tell **whether Alice and Bob talk
at all**. That is not true of the design as it stands, and the honest version has
two gaps rather than none:

- **Addressing.** Mailboxes are addressed by fingerprint in stage 1 (§8.2), so
  the operator learns who corresponds with whom. That is a consequence of
  choosing fixed addresses, not a limitation of the crypto, and it is the leak
  R3.3 requires `SECURITY.md` to spell out.
- **Timing.** Deposits and collections falling into a rhythm say something about
  who talks to whom even when every box stays shut and every address rotates.
  §4.3 blunts that and cannot eliminate it.

The daily code in the third row is the deposit pseudonym (§8.1). It exists so
the mailbox can rate-limit a flood and tell a sender their message expired —
neither of which is possible against a sender the server cannot distinguish at
all. It buys those at a stated price: within one day the operator can count how
many distinct people wrote to a mailbox and how often. It cannot connect those
codes to people, or to the previous day's.

### What goes wrong

- **Server down when Alice sends** — she cannot send, and must be told so rather
  than left believing it went.
- **Server down when Bob checks** — he simply does not have it yet.
- **Server loses its disk** — the message is gone. Alice was told "left at the
  mailbox", never "Bob read it", and the difference has to be visible in the UI.
- **Padlocks run out** — Alice sends two hundred messages while Bob is away, or
  someone drains the stack deliberately. It falls back to the spare, which is
  slightly weaker, and **both people are told**. A silent downgrade is the
  failure to avoid here.
- **A stranger tries to mail you** — refused by default. You have to have met
  live once (R1.7).

### The part that is still undecided

Step 9 — *delete after collection* — quietly assumes Bob has **one** device.
Give him two and whichever checks first takes the message; the other never sees
it.

Fixing that means Alice locks a separate copy for each of Bob's devices and the
mailbox holds one per device. That is a different design, and it has to be
chosen before any of this is built rather than bolted on afterwards. It is the
one genuinely open question left (R6.1, §4.6, §8.4) — everything else that was
undecided has a recommended answer in §8.

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

The server stores a row of: **address**, **deposit pseudonym**, **opaque
ciphertext**, **timestamp**. It must not learn the sender's identity — that goes
*inside* the sealed payload, never in the envelope.

The pseudonym is `HKDF(pairwise_secret, "deposit" ‖ epoch)`: derivable only by
the two parties, unlinkable to a long-term identity, and unlinkable to the same
pair's pseudonym in the next epoch. It is there because a mailbox that cannot
distinguish senders at all cannot rate-limit them (R5.2) and cannot tell one
that their message expired (R4.4) — see §8.1, which is where that trade is
argued rather than assumed. It is the difference between "the operator cannot
read your messages" and "the operator cannot read your messages but knows
exactly who you talk to and when"; the pseudonym sits deliberately between those
two, and §8.1 says exactly how far.

Constraints that have to be designed in, not bolted on: a per-recipient storage
cap, a TTL after which undelivered messages are dropped, and a size cap well
under `MAX_PACKET_SIZE`. All three are the same class of limit the community
server already enforces (`MAX_TOTAL_BLOB_BYTES`, `MAX_MEMBER_BLOB_BYTES`) and
for the same reason.

### 4.3 Metadata is the honest weak point

A mailbox addressed by fingerprint leaks the social graph to its operator
through arrival and collection patterns, even with perfect content encryption.
**Stage 1 accepts that leak** (§8.2) rather than pretending otherwise, because
rotating addresses breaks the per-recipient quota and the unknown-sender policy
as they are currently written.

Shipping in stage 1:

- fixed-size padding buckets
- batched, randomised collection rather than fetch-on-arrival

Deferred to a later stage, with the quota and unknown-sender policies restated
against it first:

- rotating per-pair mailbox addresses derived from a shared secret, so the
  address is unlinkable across messages

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

### 4.5 Verification: what async actually costs, and how to pay it

This app's entire verification story is **SAS on a live handshake**.
`derive_sas(transcript_material)` (`core/src/core/crypto.rs`) hashes the
transport AAD — the transcript of a handshake that *both parties just ran
together*. That is what makes it a MITM detector: an attacker running two
handshakes produces two different transcripts.

**An asynchronous first message has no such transcript.** The sender encrypts to
prekeys fetched from a server, alone, with the recipient offline.

Be precise about what that does and does not break, because the first draft of
this section overstated it. Two things remain comparable with no live handshake
at all:

- **The identity key's fingerprint.** Alice fetched a bundle claiming to be
  Bob's. She can hash the identity key in it and compare that fingerprint with
  Bob out of band — which is the app's existing "advanced" verification path,
  already rendered as the colour safety grid (`colorgrid.rs` / `colorgrid.js`,
  derived from a fingerprint, not from a session). A forged bundle produces a
  different fingerprint and is caught.
- **The X3DH shared secret**, which both sides derive identically — Alice when
  she sends, Bob when he collects. A SAS *can* be computed from it. Just not
  simultaneously.

So the MITM detection is intact and the mailbox cannot silently impersonate.
What breaks is narrower: **the ergonomics of SAS** — the "you are both on a call
right now, read six digits to each other" flow that makes verification something
people actually do. That is worth fixing properly, but it is a usability
problem, not a break in the security property.

The fix, in the order the work should be done:

1. **Signed prekey bundles, checked against a pinned fingerprint, do most of
   it.** For a contact already `Verified`/`Trusted`, Alice has Bob's identity
   fingerprint pinned; a bundle that does not match it is refused with no user
   interaction and no SAS. This covers essentially all real traffic, because
   people asynchronously message peers they already know.
2. **First contact is the only case left, and it is a policy decision.** The
   conservative default — *async first contact is not allowed; establish the
   contact once live (or by invite plus verification), and asynchronous
   messaging works forever after* — costs almost nothing in practice and keeps
   today's trust model exactly as it is. It should be the default.
3. **If async first contact is wanted anyway**, it needs a pairwise identity
   code — `derive_safety_number(IK_A, IK_B)` over the two identity keys in a
   fixed order, Signal's safety-number construction. Being identity-derived
   rather than transcript-derived, it needs no handshake, is stable across
   sessions, and can be compared at any time. It complements `derive_sas`
   rather than replacing it, and like `derive_sas` and `history_key` it needs a
   frozen known-answer test.

Two rules that do not move:

- `known_trusted` must not be relaxed to make any of this convenient. That only
  a chat-stored or `Verified`/`Trusted` contact fingerprint auto-accepts is
  load-bearing, and widening it was a real bug once already.
- The UI must carry "delivered, sender unverified" as a first-class state.
  Neither of the two states that exist today describes it honestly.

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

### 4.9 What this costs, and whether to build it at all

A design document that lists only what a feature gains is selling, not
designing. This feature takes real things away from the app, and they should be
weighed before any of the above is worth reading twice.

**"There is no server" stops being true.** Today the strongest claim this
project can make is that a conversation exists on exactly two machines. A
mailbox means a message sometimes rests on a third one. Encrypted, but present —
and its operator learns that *someone* deposited something for a given recipient
at a given time. §4.3 covers mitigating that; none of them make it untrue.

**The security story stops fitting in a sentence.** "Compare six digits with the
person you are talking to" is explicable to anyone. Prekeys, mailboxes and
ratchets make it a paragraph, and a story users cannot follow is one they cannot
rely on. That is a real loss, not a documentation problem.

**New failure modes that cannot happen today.** Mailbox unreachable: messages
stuck. Mailbox loses its disk: messages gone. A pure peer-to-peer app has
neither.

**More surface to keep correct.** The existing parsers have taken roughly 15
million fuzz iterations without a crash. Anything added starts at zero, in the
part of the codebase where mistakes are most expensive.

**It requires infrastructure somebody pays for and keeps alive.** Store-and-
forward only works if the mailbox is up when the sender deposits *and* when the
recipient collects. That is a hosting bill and an operational commitment, which
is a poor fit for a project run as a side project.

### The resolution

The trade is acceptable only because R7.3 makes the mailbox **optional**: with
none configured the app must behave exactly as it does today. This is a mode to
opt into, not a replacement for the peer-to-peer path, and it must never become
one.

That said, optional is not free — it is still months of careful work, a server
to run, and a larger system to keep correct. **Sequencing advice: this should
not be the next thing built.** The `history_key` weakness in §4.4 is days rather
than months, needs no server and no recurring cost, and makes the app strictly
better with nothing traded away. It should come first, and it is worth doing
whether or not any of the rest of this document is ever implemented.

## 5. Staging

Smallest useful increments, each shippable and testable alone:

| Stage | What | Why this order |
|---|---|---|
| 1 | Publish and fetch prekeys. No mailbox. | Pure key distribution, no storage, no new trust. Testable end to end. |
| 2 | **Blind mailbox on the relay, text only.** | This is the feature. Capped, TTL'd, and blind to sender identity — carrying the deposit pseudonym of R3.1a, not a name. |
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
| R1.3 | A verification path `MUST` exist that does not require a live handshake. Comparing the identity fingerprint already qualifies; a pairwise `derive_safety_number(IK_A, IK_B)` `SHOULD` be added so the short-code ergonomics of SAS survive. See §4.5. |
| R1.7 | Async **first contact** `MUST` be disallowed by default. Establishing a contact once — live, or by invite plus verification — and allowing asynchronous messaging thereafter keeps today's trust model intact at almost no cost. Enabling it `MUST` be a deliberate setting. |
| R1.8 | A prekey bundle whose identity key does not match a **pinned** fingerprint for that contact `MUST` be refused outright — no prompt, no fallback to first-use trust. |
| R1.4 | `known_trusted` `MUST NOT` be widened to auto-accept a fingerprint on the strength of a mailbox delivery. An invite link is not verification; neither is a message arriving. |
| R1.5 | A malicious mailbox operator serving forged prekeys `MUST NOT` be able to impersonate a contact whose identity key the user has verified. |
| R1.6 | Prekey bundles `MUST` be signed by the identity key, and the signature `MUST` be checked before any content is encrypted to them. |
| R1.9 | Identity rotation `MUST` be possible, as a **succession statement signed by the outgoing key** naming the incoming one. A contact holding the old pin verifies it, moves the pin, and enters R1.2's "delivered but unverified" state with a visible *safety number changed* marker until re-verified out of band. A key arriving with no valid succession signature still hits R1.8 unchanged — that is the MITM case, and it stays as strict as it is. Without this, R1.8's hard refuse is not just strict but permanent: a compromised or reinstalled key means being unreachable to every contact who pinned it, forever, by design. See §8.3. This also constrains §4.6 option 1 — if adding a second device changes the identity key, every existing contact hits R1.8. |
| R1.10 | Losing the private key outright `MUST` be documented as requiring out-of-band re-verification. R1.9 covers a *planned* rotation, where the old key is still available to sign; it cannot cover a key that is gone, and implying otherwise would be worse than saying so. |

### R2 — Confidentiality and secrecy over time

| | Requirement |
|---|---|
| R2.1 | The mailbox `MUST NOT` hold plaintext, nor any key from which plaintext is derivable. |
| R2.2 | Every message `MUST` be encrypted under a key that is **not** derivable from the long-term identity key alone. |
| | *This does **not** close the `history_key` weakness in §4.4, as it previously claimed.* R2.2 constrains the key a message travels under; `history_key` encrypts the stored history **at rest**, on a separate path (`client/src/app/persistence.rs`). A stolen `identity.json` still decrypts every message already on disk no matter what R2.2 says about the wire. Stage 4 is what closes that, and only stage 4. |
| R2.3 | Forward secrecy `MUST` hold per message after session establishment. |
| R2.4 | Post-compromise security `MUST` hold: a passive attacker holding one message key loses access **within one round trip** — that is, once the peer has sent anything back. |
| | *The bound is a round trip, not a message count, and the difference is the whole of the async case.* Healing comes from a DH ratchet step, which needs a message **back**. Bob offline for a week while Alice sends 200 messages advances only the sending chain: forward secrecy, zero healing. Stated as "a bounded number of messages" this requirement was unsatisfiable as written, and R8.7 demands a test for every `MUST` — there is no test for a bound that a week of one-way traffic does not reach. The honest form admits the round trip may never come. |
| R2.5 | The first message `MUST` have forward secrecy when a one-time prekey is available. |
| R2.6 | When one-time prekeys are exhausted, the session `MUST` degrade to signed-prekey-only rather than fail, and the degradation `MUST` be visible to both users — not silent. |
| R2.7 | Key derivations `MUST` bind the full transcript, as the v3 handshake already does. |
| R2.8 | A one-time prekey's **private** key `MUST` be destroyed once it has been used. Nothing above required this, and without it R2.5's first-message forward secrecy is a property of a key still sitting on disk. |
| R2.9 | Prekey **bundle fetches** `MUST` be rate-limited, per requester and per target. R5.2 limits deposits only, so draining a victim's one-time prekeys is an unauthenticated `GET` loop — after which every first message falls back to R2.6's signed-prekey-only path and R5.3 only asks that the fall be visible. |
| R2.10 | The signed-prekey-only fallback `MUST` reject a replayed initial message. X3DH's initial message is replayable exactly when no one-time prekey is consumed: the operator re-delivers the first sealed entry and Bob re-derives an identical `SK`. R4.2 is the requirement this belongs to, and its only stated defence is an in-ciphertext sequence number needing durable per-conversation state — which R5.5 evicts and a reinstall destroys. The R2.6 / R4.2 interaction is **not** currently designed. |

### R3 — Metadata

| | Requirement |
|---|---|
| R3.1 | The mailbox envelope `MUST NOT` carry the sender's **long-term identity**, in any form the operator can link across epochs. Sender identity goes inside the sealed payload. |
| R3.1a | The envelope `MAY` carry a **deposit pseudonym**: `HKDF(pairwise_secret, "deposit" ‖ epoch)`, where `epoch` is a coarse time bucket (a day). It is derivable only by the two parties, is unlinkable to a long-term identity, and is unlinkable to the same pair's pseudonym in any other epoch. |
| | *This is the resolution of a contradiction, not a preference.* R3.1 as originally written — the envelope names nothing about the sender at all — cannot hold at the same time as R5.2 (rate-limit **per sender**) or R4.4 (report expiry **to the sender**): a server that cannot distinguish senders can do neither. Three `MUST`s, jointly unsatisfiable, with R8.7 asking for a test of each. The pseudonym is what makes all three satisfiable at once. See §8 for the alternative and what it would cost. |
| R3.2 | Ciphertext `MUST` be padded to fixed size buckets, so length does not leak message size. |
| R3.3 | Mailbox addresses `SHOULD` be unlinkable across messages. Stage 1 deliberately does **not** meet this — fixed, fingerprint-derived addresses, because rotation breaks R5.1 and R5.4 as written (§8.2) — so the second half of this requirement is the operative one: the social-graph leak `MUST` be documented in `SECURITY.md`, and §2's plain-terms table `MUST NOT` claim otherwise. |
| R3.4 | Everything the operator can still observe `MUST` be enumerated in `SECURITY.md` under what the app does not claim. |

### R4 — Delivery integrity

| | Requirement |
|---|---|
| R4.1 | An operator dropping messages `MUST` be **detectable** — an authenticated per-conversation sequence number inside the ciphertext, as `FrameSeq` already does for party frames. |
| R4.2 | Replayed mailbox entries `MUST` be rejected. |
| R4.3 | Out-of-order collection `MUST NOT` lose messages or wedge a session. |
| R4.4 | A message the sender believes was delivered `MUST NOT` be silently lost by mailbox eviction; TTL expiry `MUST` be reported under the depositing pseudonym (R3.1a), which the sender polls for. |
| R4.4a | An expiry notice `MUST` carry a per-message discriminator the sender chose — an opaque tag sealed at deposit time and echoed back. Without one, a pseudonym covering a whole epoch tells the sender that *something* expired, not what: eleven deposits in a day share one pseudonym. The tag is visible to the operator, so it `MUST` be random per message and carry no structure. |
| R4.4b | Fetching an expiry notice `MUST NOT` consume it. The pseudonym is derived identically by both parties, so a destructive fetch lets the recipient collect the sender's expiry notices before the sender does. "Knowing the id is the authentication" holds against strangers, not against the two parties who both derive it. |

### R5 — Abuse resistance

| | Requirement |
|---|---|
| R5.1 | Per-recipient storage `MUST` be capped, as `MAX_MEMBER_BLOB_BYTES` already caps party storage and for the same reason. |
| R5.2 | Deposits `MUST` be rate-limited per deposit pseudonym (R3.1a) and per IP, reusing `network::ratelimit`. |
| R5.2a | A recipient's mailbox budget `MUST` be apportioned per pseudonym rather than being a single pool. A single pool is a denial tool: an attacker who fills Bob's budget blocks Bob's legitimate senders, and a per-IP limit does nothing against a distributed attack. Per-pseudonym apportionment means a flood costs the flooder their own slice and leaves everyone else's intact — which is the whole reason the pseudonym is worth its privacy cost. |
| R5.3 | Deliberate one-time-prekey exhaustion by an attacker `MUST NOT` degrade a victim's secrecy silently — see R2.6. |
| R5.4 | There `MUST` be a policy for unsolicited mail from unknown identities, and its default `MUST` be the conservative one. |
| R5.5 | Entries `MUST` have a TTL and `MUST` be evicted on collection. |

### R6 — The forks that must be closed before stage 1

| | Requirement |
|---|---|
| R6.1 | Multi-device `MUST` be decided before any wire format is frozen: fanout designed in, or single-device committed to permanently and documented. Not deferred. **Still open** — see §8.4. This is the one blocking question left. |
| R6.2 | Mailbox hosting `MUST` be decided with its operational story attached — the relay only if it grows persistence, backup and an uptime expectation. Not frozen into the wire format, so unlike R6.1 it may follow the design rather than precede it. |

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

## 8. Decisions, and the one that is still open

Everything below was a contradiction or a gap. Each now carries a **recommended
resolution** with its reasoning and its cost, so what is left is confirming or
overturning one decision rather than re-deriving all of them. Overturning any of
them is a normal outcome; leaving them unstated was not.

### 8.1 Sender anonymity vs per-sender accounting — **resolved: deposit pseudonym**

R3.1 forbade the envelope naming the sender; R5.2 required rate-limiting per
sender and R4.4 required reporting expiry to the sender. A server that cannot
distinguish senders can do neither. Two ways out:

| | Sealed-sender delivery tokens | **Deposit pseudonym** (recommended) |
|---|---|---|
| What the operator learns | nothing beyond "a valid token was spent" | "pseudonym X deposited N times this epoch" |
| Linkable across epochs | no | no — the epoch is in the KDF input |
| Linkable to an identity | no | no — derived from the pairwise secret |
| Rate-limit per sender | yes | yes |
| Report expiry to sender | needs a return path built separately | yes, the pseudonym is the return address |
| Cost to build | a subsystem: blind signatures or a VOPRF, issuance, redemption, double-spend prevention | one HKDF call |
| In the staging plan | no | yes, stage 1 |

Recommendation: **the pseudonym**, with tokens named as a later upgrade rather
than a prerequisite. Requiring tokens at stage 1 means the feature does not
ship, and shipping nothing protects nobody. The pseudonym gives up strictly less
than the status quo, where async messages do not exist at all and the
conversation happens over a community server that stores DMs in plaintext.

What it costs, to be stated in `SECURITY.md` rather than discovered: the
operator can count deposits per pseudonym within an epoch, and therefore learns
how many distinct correspondents a mailbox has that day and how often each
writes. It cannot connect those to identities or across epochs. That is more
than perfect sealed-sender leaks and much less than the envelope naming people.

If you would rather have the tokens: R3.1a is what to delete, and R5.2/R4.4
become blocked on a subsystem that needs its own design document and its own
stage.

### 8.2 Mailbox addressing — **resolved: fixed for stage 1, and say so**

Rotating per-pair addresses are better for unlinkability and break two `MUST`s
as written: R5.1's per-recipient quota and R5.4's unknown-sender policy are both
stated against a stable address. Rotation is not free elsewhere either — it
needs a rendezvous scheme so a sender knows which address to write to after a
gap.

Recommendation: **fixed, fingerprint-derived addresses for stage 1**, with
rotation as a later stage once the quota and unknown-sender policies have been
restated against it. The condition is that §2's plain-terms table stops
promising the operator cannot tell who talks to whom — it has been corrected to
say the opposite, because that is the part users read.

### 8.3 Identity rotation — **resolved: signed succession, and a changed-key state**

R1.8's hard refuse of an unpinned identity key is right against a MITM, and with
no rotation path anywhere it is also permanent: a compromised or merely
reinstalled key means being unreachable to every contact who pinned you, by
design, forever.

Recommendation: a rotation is a **succession statement signed by the outgoing
key** naming the incoming one. A contact holding the old pin verifies the
signature, moves the pin, and enters R1.2's "delivered but unverified" state
with a visible *safety number changed* marker until re-verified out of band.
This keeps R1.8 exactly as strict for a key that arrives with no succession
signature — which is the MITM case — while making a planned rotation possible.

It does not cover losing the private key outright. That case needs an
out-of-band re-verification and should say so rather than pretending otherwise.

### 8.4 Multi-device — **open, and genuinely yours to decide**

This one is not a technical judgement with a defensible default, so it is not
being given one. It is a product commitment: either every message is encrypted
separately to each of a recipient's devices and the mailbox holds one copy per
device, or the app commits to one device per identity, permanently and in
writing.

- **Fanout** costs a per-device prekey directory, per-device sessions, and a
  sender who must discover the current device set — plus deciding what happens
  when a device is added mid-conversation or lost.
- **Single-device** costs nothing now and cannot be reversed later without
  redesigning the mailbox and the ratchet, with migration, for everyone.

R6.1 says this must be closed before implementation, and it means it: both
answers are frozen into the wire format. §2's step 9 (*delete after collection*)
already assumes single-device — with two devices, whichever polls first takes
the message and the other never sees it.

**This is the blocking question.** The three above are settled unless you say
otherwise; this one is not settled at all.

### 8.5 Answerable later without regret

- `vodozemac`, or a ratchet over the existing primitives?
- Migrate existing history when `history_key` changes, or leave old history on
  the old key and start fresh? (R7.5 — either is acceptable; silence is not.)
- How do files ride the mailbox? (§4.8 — a sealed reference plus the existing
  chunked fetch is the plausible shape, but it is not designed.)
- Where the mailbox runs, and who keeps it up (R6.2). Not frozen into the wire
  format, so unlike R6.1 it can follow the design rather than precede it.

---

Related: `docs/protocol.md` (v3 handshake), `docs/platform_spec.md` §11
(phases), `SECURITY.md` (limits and open risks).
