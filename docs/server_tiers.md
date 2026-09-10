# Encryption tiers for Community servers

**Status: design. Nothing here is implemented.**

`TrustTier` already exists in `core/src/party/mod.rs` with two variants —
`Administered` (the default, and the only one that does anything) and `E2EE`
(declared, unimplemented). This document is what turns that placeholder into a
real ladder.

**Peer-to-peer direct messaging does not change.** The v3 handshake, TOFU, SAS,
rekey and file transfer in `core/src/network/` are out of scope here. This is
entirely about what the Community server can and cannot read.

## The problem this fixes

Community direct messages are **plaintext on the operator's disk today**.
`PartyState::post_dm(from, to, text: String)` stores exactly that. Anyone who
runs a community — or seizes its disk, or compromises it — reads every private
message ever sent through it. That is a shipped privacy hole, not a missing
feature, and it is the reason tier 2 comes before tier 3.

## The ladder

Three tiers, each strictly more private than the one above it. There is no
combination where channels are sealed but DMs are not: private messages are the
more sensitive half, and a ladder people can reason about is worth more than one
that covers every permutation.

| | DMs | Channels | Operator can |
|---|---|---|---|
| **1. Administered** (today) | plain | plain | read, search, moderate everything |
| **2. PrivateE2EE** | **sealed** | plain | read and moderate channels; DMs are opaque |
| **3. FullE2EE** | **sealed** | **sealed** | see who posted where and when, and nothing else |

**Channels are sealed per channel, not per server.** A community can run
`#general` in the clear — moderatable, searchable, joinable with full history —
and `#core-team` sealed. DMs follow one server-wide setting, because a DM does
not belong to a channel and there is nothing per-thread to configure.

The channel list therefore reveals *which* channels are encrypted. That is
unavoidable — a client has to know before it posts — and it is the kind of thing
that has to be visible in the UI anyway, so it costs nothing that was not
already spent.

## Decisions already taken

These three are settled. They are recorded here because each is frozen into the
wire format, and re-deciding one later means a migration.

**Someone joining a sealed channel sees nothing posted before they joined.**
No key ever travels backwards. This is the honest cryptographic answer — they
never held the old key, so the server *cannot* hand them readable history even
if it wanted to — and it is what makes tier 3 tractable at all: there is no
historical-key transport to design, secure, or abuse. It also means removing
someone actually removes them, rather than removing them from future messages
while any remaining member can still hand over the archive.

**The tier ladder is monotonic** — see the table above.

**Channels are per-channel, DMs are per-server.**

## The part that decides whether any of this is worth building

Today the **server** tells you who everyone is. `MemberInfo` carries
`{ id, username, online, role }` — no key, no fingerprint. Community trust is a
pin on the *server's* identity (`connect_and_join`'s two-step flow), not on each
member's.

That is fine while the server can read everything anyway. The moment messages
are sealed, it stops being fine: if the operator can substitute a member's
public key, they can read every message sealed "to" that member, and end-to-end
encryption buys **nothing at all** against the one adversary it is for.

So tiers 2 and 3 require, before anything else:

- `MemberInfo` carries each member's **identity public key**, alongside the
  fingerprint derived from it.
- Clients **pin a member's key on first sight** and refuse it silently changing —
  the same TOFU the P2P side already does, applied per member rather than per
  server.
- A key that changes shows a **"safety number changed"** state, and the UI is
  honest that an unverified member is unverified. `derive_safety_number(A, B)`
  gives the pairwise short code; the existing SAS emoji table is reused so users
  compare the same kind of thing they already know.

Without this, tiers 2 and 3 are theatre. With it, the operator is reduced to
what they can see from the outside, which is the point.

## Tier 2 — sealed DMs

A community DM has to reach someone who is **offline**. That is the whole
purpose of the community server, and it is what makes this harder than it
sounds: Alice cannot run a live handshake with a laptop that is shut.

The answer is prekeys, and it is the one piece of the (now abandoned) standalone
mailbox design that survives — landing **in the community server** rather than
in a new service, so this adds no infrastructure:

- Each member, while online, uploads a batch of signed **one-time prekeys** and
  one signed fallback prekey. The server stores public keys only.
- Alice fetches Bob's identity key, one one-time prekey, and the fallback. The
  one-time key is consumed and deleted.
- **Alice checks the identity key against her pin for Bob.** A mismatch is
  refused outright — no prompt, no fallback to trust. This is the step the
  previous paragraph exists to make possible.
- She derives a key only she and Bob can compute, seals the message, and posts
  it. The server stores ciphertext and routes it as it routes anything else.

`MessagePayload` gains a `Sealed` variant carrying opaque bytes. `Envelope`'s
`sender`, `channel`, `seq` and `timestamp` stay in the clear, because the server
needs them to order, page and deliver.

Forward secrecy comes from the per-message ratchet; a stolen key does not open
earlier messages. A one-time prekey that is exhausted falls back to the signed
prekey, and that fallback **must be visible**, not silent — draining someone's
one-time prekeys is otherwise a free downgrade.

## Tier 3 — sealed channels

A channel is a group, so it needs a key every current member holds.

- Each sealed channel has a **channel key**, generated by the client that
  creates it.
- The key is distributed by **sealing it to each member's identity key** — by a
  client, never by the server, which must not learn it. The server stores and
  serves the sealed blobs like any other opaque object.
- **The key rotates whenever the member set changes.** On join, so the newcomer
  cannot read backwards. On removal, so the departed cannot read forwards. This
  single rule is what implements the back-history decision.
- Messages are sealed under the key current at the time they were posted.
  Members keep old keys for history they were present for; nobody else ever
  receives them.

`ChannelKind` keeps working unchanged: `Locked` and `Announce` are *write*
permissions enforced on metadata the server can still see, and `Private`
membership is a server-side list. None of them need to read message content.

## Files

Blobs are content-addressed and deduplicated — one file shared into three
channels costs its size once. **Sealed files break that**, and pretending
otherwise would be the wrong trade:

- A sealed file is encrypted with a random per-file key, and that key is sealed
  into the message that references it.
- The stored ciphertext is content-addressed like anything else, but two
  identical plaintexts encrypt to different bytes, so **they do not deduplicate**.
- Making them deduplicate means deriving the file key from the plaintext hash
  (convergent encryption), which tells the operator when two members uploaded
  the same file. That is a real leak for a small saving, and it is not proposed.

The existing quota accounting is unaffected: it counts stored bytes, and sealed
bytes are still bytes.

## What the operator keeps, and what they lose

Worth being blunt, because "encrypt everything" sounds free and is not.

**Kept at every tier:** who is a member, who posted in which channel and when,
message sizes, file sizes, the channel list, role changes, the audit log,
storage quotas, and the ability to delete a message or a file — deletion is a
metadata operation and does not need the content.

**Lost on a sealed channel:** server-side search, moderation-by-reading, and
any admin tool that inspects what was actually said. An admin can still remove a
member, delete a channel, or delete a message they have been *shown*; they
cannot go looking.

**Lost on sealed DMs:** the same, and it is the point.

**Never covered by any tier:** who talks to whom, and when. Sealing content does
not hide the shape of the traffic, and no tier here claims to.

## Requirements

| | Requirement |
|---|---|
| T1.1 | `MemberInfo` `MUST` carry each member's identity public key, and clients `MUST` verify the fingerprint is derived from it rather than trusting both fields independently. |
| T1.2 | A member's identity key `MUST` be pinned on first sight and a silent change `MUST` be refused. |
| T1.3 | A changed member key `MUST` surface as a distinct "safety number changed" state, not as an error and not silently. |
| T1.4 | The UI `MUST` distinguish verified from merely pinned members. |
| T2.1 | Prekey bundles `MUST` be signed by the identity key, and the signature checked before anything is encrypted to them. |
| T2.2 | A prekey bundle whose identity key does not match the pin `MUST` be refused outright — no prompt, no first-use fallback. |
| T2.3 | A one-time prekey's private half `MUST` be destroyed once used. |
| T2.4 | Falling back to the signed prekey `MUST` be visible to both parties. |
| T2.5 | Prekey *fetches* `MUST` be rate-limited, or draining a victim's one-time keys is an unauthenticated loop. |
| T3.1 | A sealed channel's key `MUST` be generated and distributed by clients; the server `MUST NOT` be able to derive it. |
| T3.2 | The channel key `MUST` rotate on every membership change, in both directions. |
| T3.3 | A historical channel key `MUST NOT` be transportable to a member who was not present — there is no protocol message that carries one. |
| T4.1 | `Join` `MUST` negotiate a protocol capability, so a client that cannot read sealed content is told rather than silently shown nothing. |
| T4.2 | A client `MUST NOT` post plaintext into a channel it believes is sealed, or vice versa; the tier is authenticated as part of the message, not inferred. |
| T5.1 | `SECURITY.md` `MUST` state what each tier does and does not hide, in the same plain terms as the table above. |
| T5.2 | Every `MUST` here `MUST` have a test that fails if it regresses. |

## Staging

| Stage | What | Why this order |
|---|---|---|
| 0 | Member identity keys in `MemberInfo`, per-member TOFU, safety numbers, `Join` capability negotiation | Every later stage is worthless without it, and it is useful on its own — members become verifiable even at tier 1 |
| 1 | Prekey storage and fetch on the server | Pure addition; nothing reads it yet |
| 2 | **Tier 2**: sealed DMs | Closes the shipped plaintext-DM hole |
| 3 | Sealed files | Independent of tier 3, needed by tier 2 DMs with attachments |
| 4 | **Tier 3**: channel keys, rotation on membership change | The hard half, and the only part that costs the operator moderation |

Stage 0 is not optional and is not a formality. It is where the security of
everything after it comes from.

## Open questions

1. **Does an operator get to force a tier?** A community advertising itself as
   fully encrypted is making a promise; a member's client currently has no way
   to prove the server did not quietly downgrade a channel. The tier is
   authenticated per message (T4.2), so a downgrade is *detectable* — but what
   the client should do about it (refuse, warn, both) is not decided.
2. **What happens to an existing plaintext channel that is switched to sealed?**
   Cleanest is that it cannot be: sealing is chosen at creation. Converting means
   either abandoning the history or re-encrypting it, and re-encrypting it means
   the client that does so has read all of it.
3. **Multi-device.** Sealed DMs are sealed to a device's keys. The P2P side has
   the same question and it is deferred there; here it decides whether a member's
   second client can read their own DMs at all.
4. **Key backup.** Losing the channel keys means losing the history, permanently
   and by design. Whether that is acceptable, or whether keys should be derivable
   from the identity key (and therefore recoverable, and therefore compromised
   together), is not decided.

---

Related: `docs/protocol.md` (the v3 P2P handshake, unchanged by this),
`docs/architecture.md`, `SECURITY.md` (limits and open risks),
`docs/platform_spec.md` §11.
