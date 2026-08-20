# Asynchronous delivery without giving up end-to-end encryption

**Status: design sketch. Nothing here is implemented.**

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

**Recommendation: the relay.** Adding "hold sealed bytes addressed to a
fingerprint" is a modest extension of a component whose existing job is already
"help two people who cannot reach each other directly", and whose existing
security posture is already "sees only ciphertext". A new service would be more
architecturally tidy and materially more work for no security gain.

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

- **Multi-device.** One identity is one device. Async delivery makes this worse,
  not better: a message collected by one device is gone for the others unless
  fanout is designed in from the start.
- **Group DMs.** `ChatKind::Group` appears exactly twice in the workspace, both
  in a serde round-trip test. It is a type, not a feature.
- **The mailbox being available.** Store-and-forward turns an optional relay
  into infrastructure that has to stay up.

## 7. Open questions

1. Does the mailbox live on the relay, or is coupling delivery to NAT traversal
   a mistake that is hard to undo later?
2. Rotating mailbox addresses from the start, or fingerprint-addressed first and
   accept the graph leak while the feature proves itself?
3. `vodozemac`, or a ratchet over the existing primitives?
4. Migrate existing history when `history_key` changes, or leave old history on
   the old key and start fresh?

---

Related: `docs/protocol.md` (v3 handshake), `docs/platform_spec.md` §11
(phases), `SECURITY.md` (limits and open risks).
