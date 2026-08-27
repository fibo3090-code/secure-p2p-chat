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
| Something was left for Bob at 14:32 | Who wrote it |
| Roughly how big, padded into size buckets | The contents, even if the disk is seized |
| When Bob collected | Which of Bob's contacts a given message is from |
| An unlinkable per-epoch pseudonym for whoever deposited it | The long-term identity behind that pseudonym |

**It can, however, work out who talks to whom.** This row used to say the
operator "cannot see whether Alice and Bob talk at all", and under the design's
own default that is not true: §8 Q3 is answered "fingerprint-addressed mailboxes
first, rotation deferred", so Bob's address is stable and derived from his
identity. An operator watching a stable address, plus the timing of deposits and
collections, recovers a good deal of the social graph — §4.3 says exactly that
three sections later, and the plain-terms table is the part users actually read.
So it says it here too.

What is genuinely hidden is **content**, and the sender's long-term identity: the
envelope carries a per-epoch pseudonym (§4.2), not a name. The honest gaps are
**timing** and **stable addressing**. §4.3 blunts them and cannot eliminate them.

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
one genuinely open question left (R6.1, §4.6).

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

### 4.2 The mailbox is blind — and the sender is pseudonymous, not anonymous

The server stores a row of: **address**, **deposit pseudonym**, **opaque
ciphertext**, **timestamp**. The sender's *identity* goes inside the sealed
payload, never in the envelope.

That middle field is a correction, and it is worth being explicit about why it
is there, because the first draft of this document did not have it and could not
have worked.

**The tension.** R3.1 says the envelope must not name the sender. R5.2 says
deposits must be rate-limited *per sender*. R4.4 says TTL expiry must be
reported *to the sender*. A server that cannot identify the sender can do
neither of the last two: it cannot count what it cannot distinguish, and it
cannot send a notice to a party it cannot address. Three `MUST`s, and no
implementation satisfies all three.

Signal resolves this with **sealed-sender delivery tokens**: the server issues a
sender a blind-signed token, the sender spends one per deposit, and the server
can enforce a budget without learning whose budget it is. That is the right
answer and it is a subsystem — a blind signature scheme, token issuance, double-
spend prevention, revocation — that appears nowhere in the staging plan in §5 and
would roughly double stage 2.

**The decision, for now: a per-epoch pseudonym.** Each sender derives, per
recipient and per epoch (say, per UTC day),

```text
deposit_id = HKDF(shared_secret_with_recipient, "p2pem-deposit-v1" || epoch)
```

and puts that in the envelope. It is unlinkable across epochs and across
recipients, and it is derived from a secret the server does not have, so it names
nobody: the operator learns "the same party deposited eleven times today for this
recipient", not who they are.

⚠️ **It is not server-verifiable, and that limits what it can be used for.** The
server does not hold the shared secret, so it cannot check that a `deposit_id`
was honestly derived. A sender who wants to evade a per-pseudonym budget simply
puts a fresh random value in the field each time. Any requirement that depends on
the server *enforcing* something per sender therefore cannot rest on this, and
saying otherwise would be the same mistake as the requirement it replaces —
stating something the design cannot do. So, precisely:

| Wanted | Rests on | Enforceable now? |
|---|---|---|
| Rate-limit deposits (R5.2) | recipient address + source IP, both of which the server can see and check | **Yes** |
| Rate-limit *per sender* | a token the server can verify without identifying | **No** — needs delivery tokens (stage 6) |
| Report TTL expiry (R4.4) | the pseudonym, as a return address | **Yes** — see below |

Per-*sender* limiting is the one that needs the token subsystem, and R5.2 is
reworded to ask for what a blind server can actually do: cap deposits per
recipient mailbox and per source address. Those bound the abuse that matters
(filling someone's mailbox, and flooding the server) without needing to know who
is doing it. A sender-specific budget waits for stage 6, and the requirement says
so rather than implying it is met.

For R4.4 the pseudonym is enough, because forging it is self-defeating. It is a
**return address, not a credential**: the server files an expiry notice under the
`deposit_id` the deposit carried, and the sender collects it by polling for that
id over the same fetch endpoint it uses for its own mailbox. An unknown id
returns nothing, and a sender who invents ids simply never collects notices for
its own messages. Knowing the id *is* the authentication — it is derived from a
secret only the two parties hold — so no separate credential and no push channel
is needed. What the server learns is that somebody asked about an id it holds a
notice for, which is the same linkability §4.3 already accounts for.

This is all weaker than sealed sender and it is stated as weaker. What it costs
is listed in §4.3 and belongs in `SECURITY.md`; what it buys is that R4.4 and a
meaningful part of R5.2 become implementable in stage 2 instead of blocking on a
subsystem nobody has scheduled. R3.1 is restated to match — the envelope must not
carry a *long-term* identity — and delivery tokens stay the documented upgrade
path, to be taken whenever the token subsystem is worth building.

Constraints that have to be designed in, not bolted on: a per-recipient storage
cap, a TTL after which undelivered messages are dropped, and a size cap well
under `MAX_PACKET_SIZE`. All three are the same class of limit the community
server already enforces (`MAX_TOTAL_BLOB_BYTES`, `MAX_MEMBER_BLOB_BYTES`) and
for the same reason.

### 4.3 Metadata is the honest weak point

A mailbox addressed by fingerprint leaks the social graph to its operator
through arrival and collection patterns, even with perfect content encryption.

**Decision: fingerprint-derived addresses first, rotation deferred** (§8 Q3,
answered). Rotating per-pair addresses derived from a shared secret would remove
the stable identifier, and it is the better end state — but it is not free, and
what it costs is two other `MUST`s:

- **R5.1**, the per-recipient storage cap, is enforced against an address. If
  the address rotates, "this recipient's quota" is not a thing the server can
  compute; it would need a rotation-invariant recipient token, which is the same
  unlinkable-budget problem as §4.2 and has the same answer (delivery tokens) at
  the same cost.
- **R5.4**, the unknown-sender policy, is decided per recipient against what the
  recipient has previously seen. Rotating addresses make "previously seen" mean
  something different and need designing rather than asserting.

So: fixed addresses, both of those `MUST`s implementable, and the graph leak
written down instead of designed away. Rotation is a stage-6 item with the two
requirements above re-derived when it lands, not a to-do.

Shipping alongside it, because neither depends on rotation:

- fixed-size padding buckets (R3.2)
- batched, randomised collection rather than fetch-on-arrival

A small self-hosted mailbox will have weak anonymity properties regardless. The
right response is to document that plainly in `SECURITY.md` — the file already
has a "What This App Does Not Claim" section, and this belongs in it — rather
than to imply a protection the deployment cannot deliver. The plain-terms table
in §"What the server can and cannot see" now says the same thing, because that
is the part users read.

### 4.4 The ratchet, and a problem it exposes

Today: one ephemeral X25519 exchange per session gives forward secrecy *between*
sessions, and `Rekey` rotates the key every 100 messages *within* one. There is
no post-compromise security: an attacker who takes a session key and stays
passive keeps reading until the session ends.

A Double Ratchet fixes that and is the natural fit once X3DH exists. Writing one
from scratch is not advisable; `vodozemac` (Rust, audited, from Matrix) is the
credible option to evaluate.

**What it fixes, precisely.** Post-compromise security comes from a DH ratchet
step, and a DH ratchet step needs a message *back*. In the case this whole
document is about — Bob is offline for a week — Alice's two hundred messages all
advance the sending chain and nothing else: symmetric-ratchet forward secrecy,
zero healing. An attacker holding a chain key reads all two hundred.

So the honest statement of the bound is **one round trip, which may never
come** — not "a bounded number of messages". R2.4 is written that way, because a
requirement stated as a message count cannot have the test R8.7 demands: there
is no number of messages that heals a one-way conversation.

### 4.4a Replay, and the prekey-exhaustion path

X3DH's initial message is replayable whenever no one-time prekey is consumed. In
the SPK-only fallback of R2.6 the recipient derives the identical `SK` from the
identical inputs every time, so an operator can re-deliver the first sealed entry
and Bob decrypts it again — a genuine message, delivered twice, with no
tampering to detect.

R4.2 says replays must be rejected, and the only mechanism this document offered
was an in-ciphertext sequence number. That needs durable per-conversation state,
which R5.5 evicts on collection and a reinstall destroys — exactly the two
moments an attacker would pick. The R2.6 / R4.2 interaction was not addressed;
it is addressed here.

Three things close it, and all three are cheap:

1. **One-time prekeys are one-time on the recipient's side too.** The private
   OPK is deleted the moment it is used, not when the server says the public one
   was handed out. Nothing in the document required this, and without it "one
   time" is a naming convention.
2. **The initial message carries a random `initial_id`, and the recipient keeps
   a bounded set of the ones it has accepted**, keyed per sender identity and
   aged out on the same TTL as the mailbox entry. This is durable state, but it
   is small and bounded, and losing it (reinstall) degrades to accepting one
   replay of a message from before the reinstall — an attacker's best outcome is
   a duplicate the user can see, not a decryption they could not otherwise get.
3. **The SPK-only path is rate-limited at the fetch, not just at the deposit.**
   See below.

**Draining someone's one-time prekeys must not be free.** Nothing in the original
requirement set rate-limited bundle *fetches*, so exhausting a victim's OPKs was
an unauthenticated `GET` loop: after it, every first message to that victim falls
back to SPK-only, R2.5's first-message forward secrecy is gone, and R5.3 only
asked that the downgrade be visible. Visible is not the same as prevented. Bundle
fetches are therefore rate-limited per requester and per target, the batch is
replenished on a schedule, and exhaustion is treated as the attack signal it
usually is.

Adopting it exposes something worth fixing on its own merits: **`history_key` is
derived from the long-term private key** (`core/src/identity/`, hashing the
PKCS#8 DER). One stolen `identity.json` therefore decrypts every message ever
stored, forever. There is forward secrecy on the wire and none at rest. That is
a bigger practical weakness than anything in the transport, and it is
independent of everything else here.

⚠️ **Nothing in R2 closes it.** R2.2 used to claim it did, and it does not: R2.2
constrains the key each *message* is encrypted under in transit, while
`history_key` encrypts the history file at rest on an entirely separate path
(`client/src/app/persistence.rs`). A perfect transport leaves the at-rest
weakness exactly where it was. **Stage 4 is what closes it**, and R2.2 now says
so instead of taking credit for it.

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
| 2 | **Blind mailbox on the relay, text only.** | This is the feature. Capped, TTL'd, sealed-sender. |
| 3 | Double Ratchet replacing the per-session schedule. | Needs stage 1. Independent of stage 2. |
| 4 | Separate `history_key` from the identity key. | Standalone win; do it whenever. **This is the stage that closes §4.4's at-rest weakness — R2.2 does not.** |
| 5 | Community E2EE tier (spec Phase 4) reusing stage 1. | Group keys are much easier once prekey distribution exists. |
| 6 | Rotating mailbox addresses, and sealed-sender delivery tokens. | Deferred on purpose (§4.2, §4.3): both need a rotation-invariant recipient/sender token, and both break a `MUST` in R5 until that exists. Taking them later costs a migration; taking them now costs stage 2. |

Stage 2 is where a user notices anything.

Two things that look like stage 6 and are **not** deferrable, because they are
cheap and their absence is exploitable from day one: destroying a one-time
prekey's private half at first use (R2.9) and rate-limiting bundle *fetches*
(R5.3a). Both belong in stage 1, where prekeys are introduced.

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
| R1.8 | A prekey bundle whose identity key does not match a **pinned** fingerprint for that contact `MUST` be refused outright — no prompt, no fallback to first-use trust. The refusal `MUST` be attributed to a *changed key*, not reported as a delivery failure, and `MUST` offer re-verification (R1.9). Refusing silently and refusing permanently are different things; only the first is wanted. |
| R1.9 | Identity **rotation** `MUST` be a supported operation, not an accident. A contact whose identity key changes `MUST` land in an explicit "safety number changed" state — R1.2's "delivered but unverified", carrying the reason — from which the user can re-verify out of band and re-pin. Without this, R1.8's hard refusal makes a compromised key a permanent loss of *every* contact: the one user who most needs to rotate is the one who has just been compromised, and under R1.8 alone rotating makes them unreachable forever. |
| R1.10 | Rotation `SHOULD` be signed by the key being retired, and that signature `MUST` be treated as **evidence, not authorization**. ⚠️ The distinction is the whole requirement. A signature from the retired key proves control of that key — and in the case rotation exists for, an attacker *has* that key, so "signed by the old key" is exactly what a hostile rotation looks like too. It may therefore raise confidence in a routine rotation (a planned key change, a device migration) and `MUST NOT` on its own authorise one. Authorisation comes from out-of-band re-verification (R1.9) whenever the retired key may be compromised, and the user `MUST` be told which of the two happened, because they carry different assurance and only one of them is a MITM defence. |
| R1.11 | A **revocation** — "this identity is burned, refuse it" — `MUST` be expressible and `MUST` be honoured even when no replacement key is offered. |
| R1.12 | Under §4.6 option 1 (one identity key, all devices), adding a device `MUST NOT` change the identity key, so it `MUST NOT` trigger R1.9. Rotation and enrolment are different events and must not present as the same one. |
| R1.4 | `known_trusted` `MUST NOT` be widened to auto-accept a fingerprint on the strength of a mailbox delivery. An invite link is not verification; neither is a message arriving. |
| R1.5 | A malicious mailbox operator serving forged prekeys `MUST NOT` be able to impersonate a contact whose identity key the user has verified. |
| R1.6 | Prekey bundles `MUST` be signed by the identity key, and the signature `MUST` be checked before any content is encrypted to them. |

### R2 — Confidentiality and secrecy over time

| | Requirement |
|---|---|
| R2.1 | The mailbox `MUST NOT` hold plaintext, nor any key from which plaintext is derivable. |
| R2.2 | Every message `MUST` be encrypted **in transit** under a key that is not derivable from the long-term identity key alone. ⚠️ This does **not** close the `history_key` weakness in §4.4: that key encrypts the history file at rest, on a separate path (`client/src/app/persistence.rs`), and a perfect transport leaves it untouched. **Stage 4 closes it**, and R2.8 is the requirement that says so. |
| R2.3 | Forward secrecy `MUST` hold per message after session establishment. |
| R2.4 | Post-compromise security `MUST` hold **after one round trip**: once the peer has sent anything the session has ratcheted, and a passive attacker holding an earlier chain key is locked out. Stated as a round trip and not as "a bounded number of messages", because in the store-and-forward case the bound does not exist: Bob is offline for a week, Alice's two hundred messages advance only the sending chain, and an attacker holding that chain key reads all two hundred. Forward secrecy, zero healing. R8.7 requires a test for every `MUST`, and "a bounded number of messages" has no test — the round-trip form does: send, receive a reply, assert the pre-reply key no longer decrypts. |
| R2.5 | The first message `MUST` have forward secrecy when a one-time prekey is available. |
| R2.6 | When one-time prekeys are exhausted, the session `MUST` degrade to signed-prekey-only rather than fail, and the degradation `MUST` be visible to both users — not silent. |
| R2.7 | Key derivations `MUST` bind the full transcript, as the v3 handshake already does. |
| R2.8 | `history_key` `MUST` stop being derivable from the long-term identity key (stage 4). This is the requirement R2.2 used to be read as making and does not. Subject to R7.5: a migration, or a written decision to leave old history on the old key. |
| R2.9 | A one-time prekey's **private** half `MUST` be destroyed at first use, by the recipient, independently of what the server believes it handed out. Nothing previously required this, and without it "one-time" is a naming convention rather than a property. |

### R3 — Metadata

| | Requirement |
|---|---|
| R3.1 | The mailbox envelope `MUST NOT` carry a sender's **long-term identity**; that goes inside the sealed payload. It `MAY` carry a per-epoch, per-recipient deposit pseudonym derived from a secret the server does not hold (§4.2). *Restated deliberately.* The original — "`MUST NOT` name the sender" — was unsatisfiable alongside R5.2 (rate-limit per sender) and R4.4 (report TTL expiry to the sender): a server that cannot distinguish senders can neither count them nor address them. Full sealed-sender with blind-signed delivery tokens is the end state and is not in the §5 staging plan; the pseudonym is what makes R5.2 and R4.4 implementable now, and it is weaker, and §4.3 and `SECURITY.md` say so. |
| R3.2 | Ciphertext `MUST` be padded to fixed size buckets, so length does not leak message size. |
| R3.3 | Mailbox addresses `SHOULD` be unlinkable across messages. **They are not, in the shipped design** (§4.3: fingerprint-derived, rotation deferred), because rotation breaks R5.1's per-recipient quota and R5.4's unknown-sender policy, both `MUST`s. The justification is this row; the leak `MUST` therefore be documented in `SECURITY.md`, and `MUST NOT` be contradicted by the plain-terms table in this document. |
| R3.4 | Everything the operator can still observe `MUST` be enumerated in `SECURITY.md` under what the app does not claim. |

### R4 — Delivery integrity

| | Requirement |
|---|---|
| R4.1 | An operator dropping messages `MUST` be **detectable** — an authenticated per-conversation sequence number inside the ciphertext, as `FrameSeq` already does for party frames. |
| R4.2 | Replayed mailbox entries `MUST` be rejected — **including the initial message on the SPK-only path**, which is the case the in-ciphertext sequence number does not cover. X3DH's initial message is replayable whenever no one-time prekey is consumed: the recipient re-derives the identical `SK` from identical inputs, so an operator can re-deliver the first sealed entry and it decrypts again. R2.9 (destroy the private OPK at first use) removes the ordinary path; the fallback path needs R4.2a. See §4.4a. |
| R4.2a | The initial message `MUST` carry a random `initial_id`, and the recipient `MUST` keep a bounded set of the ones it has accepted, per sender identity. ⚠️ **Those tombstones `MUST` outlive the mailbox TTL, and `MUST` be bounded by the signed prekey's acceptance window instead.** Ageing them out on the mailbox TTL does not work: an operator who kept a copy of the ciphertext simply re-submits it after the tombstone expires, the SPK-only path re-derives the identical `SK`, and the message is accepted again — R4.2 violated by waiting. The SPK window is the right bound because it is what actually ends the replay: once the recipient stops accepting that signed prekey, the old initial message cannot derive a usable key at all. A reinstall still destroys the set; the cost is bounded at one accepted replay of a pre-reinstall message, which is a visible duplicate rather than a decryption the attacker could not otherwise obtain. |
| R4.3 | Out-of-order collection `MUST NOT` lose messages or wedge a session. |
| R4.4 | A message the sender believes was delivered `MUST NOT` be silently lost by mailbox eviction; TTL expiry `MUST` be reported to the sender. The **retrieval path `MUST` be specified**, not assumed: the server files the notice under the `deposit_id` the deposit carried, and the sender collects it by polling that id over the same fetch endpoint it already uses — no push channel, no callback, no separate credential, because knowing the id *is* the authentication (it is derived from a secret only the two parties hold). An unknown id returns nothing. A design in which the server cannot address the sender, or in which no defined channel carries the notice, `MUST NOT` claim this requirement. |

### R5 — Abuse resistance

| | Requirement |
|---|---|
| R5.1 | Per-recipient storage `MUST` be capped, as `MAX_MEMBER_BLOB_BYTES` already caps party storage and for the same reason. |
| R5.2 | Deposits `MUST` be rate-limited per **recipient mailbox** and per **source IP**, reusing `network::ratelimit`. Both are values the server can see and check, so both are enforceable against a blind envelope, and together they bound the abuse that matters: filling one person's mailbox, and flooding the server. Note the limiter's own precondition: it must canonicalise IPv4-mapped IPv6 before masking, or every IPv4 client on a dual-stack listener shares one bucket. |
| R5.2a | Rate limiting **per sender** `MUST NOT` be claimed until delivery tokens exist (stage 6). ⚠️ An earlier revision of R5.2 rested it on the R3.1 deposit pseudonym; that does not work, and the reason is worth keeping. The pseudonym is derived from a secret the *server does not hold*, so the server cannot check that any given value was honestly derived — a sender who wants more than their budget puts a fresh random value in the field each time and the limit never binds. A pseudonym is usable as a return address (R4.4), where forging it costs the forger their own notice; it is not usable as a budget, where forging it costs the victim. Anything the server must *enforce* per sender needs a token the server can verify without learning who holds it. |
| R5.3 | Deliberate one-time-prekey exhaustion by an attacker `MUST NOT` degrade a victim's secrecy silently — see R2.6. |
| R5.3a | Prekey **bundle fetches** `MUST` be rate-limited, per requester and per target. Nothing previously limited them, which made draining a victim's one-time prekeys a free, unauthenticated `GET` loop; after it, every first message to that victim falls back to SPK-only and R2.5's first-message forward secrecy is gone for as long as the attacker keeps the batch empty. R5.3 only asked that the downgrade be *visible*, and visible is not prevented. |
| R5.3b | The one-time prekey batch `MUST` be replenished automatically while the client is online, and exhaustion `MUST` be surfaced to the *victim* as a probable attack rather than only to the sender as a downgrade. |
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

R8.7 ("every `MUST` above `MUST` have a test") is also a check on the *wording*
of every requirement above it. A `MUST` that cannot be phrased as a failing test
is not a requirement, it is a hope — R2.4's original "within a bounded number of
messages" was the example, and it is why that row now says "after one round
trip". When a requirement below fails this check, fix the requirement.

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
2. every `SHOULD` that was not met has a written justification — R3.3 is one,
   and its justification is that satisfying it would break two `MUST`s,
3. every residual limitation appears in `SECURITY.md` **and** in the plain-terms
   table in this document, which is the part a non-specialist reads,
4. no decision has been deferred that would later require a wire-format change —
   note that the R3.1 pseudonym is a field in the envelope precisely so that
   replacing it with a delivery token later is a value change, not a shape change,
5. and R6.1 and R6.2 were answered **before** implementation began, not after.

No two `MUST`s contradict each other. That was not true of the first draft:
R3.1 against R5.2 and R4.4 was a set of three requirements no implementation
could satisfy, and it survived review because each row reads correctly on its
own. A requirement set is only acceptance criteria if it is jointly satisfiable,
so checking the rows against each other is part of the bar, not a courtesy.

Anything less is fine engineering. It is just not perfect, and the difference
should be stated rather than blurred.

## 8. Open questions

Ordered by how expensive they are to answer late. The first two are **blocking**
— R6 says they must be closed before implementation, because both are frozen
into the wire format:

1. **Multi-device: fanout, or permanently single-device?** (R6.1) Changing
   position later means redesigning the mailbox and the ratchet, with migration,
   for everyone. This is the only genuinely unresolved blocking question — §4.5
   answers the verification one.
2. **Where does the mailbox live, and who keeps it up?** (R6.2) The relay only
   if it grows persistence and an uptime expectation; otherwise a separate
   service.

**Answered here**, recorded so they are not reopened by accident:

- **Q3 — Rotating mailbox addresses, or fingerprint-addressed first?** (R3.3)
  **Fingerprint-addressed first.** Rotation is the better end state and it breaks
  two `MUST`s on the way: R5.1's per-recipient quota has no unit to count against
  once the address moves, and R5.4's unknown-sender policy is decided against a
  recipient's history of senders. Both want the same rotation-invariant token
  that §4.2 defers. So: fixed addresses, the graph leak documented in
  `SECURITY.md` **and** in the plain-terms table rather than designed away, and
  rotation as stage 6, which re-derives those two requirements. §4.3.
- **Q7 — Sender anonymity versus rate limiting and TTL reporting.** A **per-epoch
  deposit pseudonym** for addressing, and rate limits on what the server can
  actually check. R3.1 as originally written was unsatisfiable alongside R5.2 and
  R4.4: a server that cannot distinguish senders can neither count them nor
  address them. The split is by *who bears the cost of forging the field*: as a
  **return address** (R4.4) a forged pseudonym costs only the forger their own
  expiry notice, so the pseudonym is enough; as a **budget** (R5.2) a forged
  pseudonym costs the victim, and since the server cannot verify the derivation
  it is worth nothing there. So per-sender limiting waits for blind-signed
  delivery tokens (stage 6), and R5.2 asks instead for per-recipient and per-IP
  limits, which a blind server can enforce today. §4.2. Decide it before the
  envelope is written, because it is a field.
- **Q8 — Identity rotation and revocation.** **In scope, as R1.9–R1.12.** R1.8's
  hard refusal is right against MITM and, on its own, makes a compromised key a
  permanent loss of every contact — the user who most needs to rotate is the one
  who has just been compromised. The "safety number changed" state belongs in
  R1.2's "delivered but unverified", carrying its reason.

**Still answerable later without regret:**

- **Q4** — `vodozemac`, or a ratchet over the existing primitives?
- **Q5** — Migrate existing history when `history_key` changes, or leave old
  history on the old key and start fresh? (R7.5 — either is acceptable; silence
  is not.)
- **Q6** — How do files ride the mailbox? (§4.8 — sealed reference plus the
  existing chunked fetch is the plausible shape, but it is not designed.)

---

Related: `docs/protocol.md` (v3 handshake), `docs/platform_spec.md` §11
(phases), `SECURITY.md` (limits and open risks).
