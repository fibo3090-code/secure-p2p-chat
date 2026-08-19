# Protocol Specification

This document describes the current wire behavior of the application. It should only describe shipped runtime behavior, not roadmap ideas.

## Constants

```rust
PORT_DEFAULT: 12345
MAX_PACKET_SIZE: 8 MiB
FILE_CHUNK_SIZE: 64 KiB
MAX_TEXT_MESSAGE_BYTES: 64 KiB   // hard cap on a single text message
TEXT_CHUNK_BYTES: 48 KiB         // split threshold (headroom for metadata)
AES_KEY_SIZE: 32
AES_NONCE_SIZE: 12
AES_GCM_TAG_SIZE: 16
REKEY_NONCE_SIZE: 16             // HKDF salt carried by a Rekey message
RSA_KEY_BITS: 2048
HANDSHAKE_TIMEOUT_SECS: 15
MAX_FILE_SIZE: 10 GiB
```

Party server input caps (`server/src/state.rs`), enforced server-side:

```rust
MAX_USERNAME_CHARS: 32
MAX_CHANNEL_NAME_CHARS: 64
MAX_MESSAGE_TEXT_BYTES: 64 KiB   // channel and DM text
```

## Cryptographic Summary

- Session establishment: X25519 ECDH
- Key derivation: HKDF-SHA256
- Transport encryption: AES-256-GCM
- Long-term identity keys: RSA-2048
- Identity proofs: RSA-PSS with SHA-256
- Chat history encryption: ChaCha20-Poly1305

## Framing

Packets use a 4-byte big-endian length prefix followed by payload bytes.

```text
length (u32, big endian) || payload
```

Oversized packets are rejected.

## Handshake

### Protocol generation

Current secure runtime behavior requires protocol version `>= 3`.

### Handshake flow

1. Exchange protocol versions in plaintext.
2. Exchange X25519 ephemeral public keys in plaintext. Each side rejects an
   all-zero peer key on parse and a non-contributory shared secret at
   agreement (low-order-point guard, RFC 7748 §6.1).
3. Derive a shared session key using HKDF-SHA256.
4. Establish encrypted transport with AES-256-GCM.
5. Exchange encrypted `IdentityProof` messages inside that tunnel.
6. Verify the peer's identity signature.
7. Optional connection-password gate (see below).
8. Confirm the peer fingerprint (TOFU) before surfacing the session. Both
   ends also derive a **Short Authentication String** (see below) so users
   can compare a short code instead of the full fingerprint.
9. Enter the message loop.

### Short Authentication String (SAS)

Alongside the 64-char fingerprint, both peers derive an identical SAS from the
handshake transport AAD (itself the transcript hash), via
`HKDF-SHA256(info = "p2pem-sas-v1")`: six decimal digits (`NN-NN-NN`) plus
three emoji from a fixed 32-entry table (~35 bits shown). Because it is bound
to the transcript — which includes both ephemeral keys and both identity keys
— an active MITM necessarily runs two *different* handshakes, so the two
victims see two *different* SAS values. Reading the code aloud over any
out-of-band channel (a call, in person) therefore detects interception with
far less friction than a full-fingerprint compare. The SAS is display-only
verification aid: it is never sent on the wire, and trust is still pinned by
TOFU on the identity fingerprint. The derivation and emoji table are frozen
(a known-answer test guards them) because both peers must compute byte-for-byte
identical strings.

### Connection password (optional)

When the host is configured with a connection password, an extra step runs inside
the established, transcript-bound tunnel *after* identity verification and *before*
the peer is surfaced for fingerprint confirmation:

- The host sends an encrypted one-byte flag indicating whether a password is required.
- The client replies with the (encrypted) password, or an empty payload when none is
  required.
- The host compares the supplied password to the expected one in constant time; a
  mismatch (or a missing password) aborts the session before any TOFU prompt.

Both frames are encrypted with the session cipher and bound to the transport AAD, so
the password is never exposed and is replay-protected by the session's nonces. When
the host has no password configured, the flag is `0` and the exchange is a no-op.

### Transcript binding

The runtime binds encrypted identity proofs and transport packets to the handshake transcript using AAD derived from the transcript hash.

That means:

- ciphertext from one transcript does not authenticate under another
- stripping AAD causes decryption failure

### IdentityProof

The proof contains:

- `public_key_pem`
- `signature`
- `version`
- `chat_id`
- `signature_scheme`

Current runtime support:

- the wire keeps `signature_scheme`
- the runtime currently advertises and accepts only `RSA-PSS`

## Message Types

Runtime messages include:

- `Version`
- `EphemeralKey`
- `SupportedSignatureSchemes`
- `Text`
- `TextChunk`
- `FileMeta`
- `FileChunk`
- `FileEnd`
- `FileCancel` (binary tag 12) — aborts the in-flight transfer on the chat;
  sent by either the sender or the receiver. The peer stops streaming (sender)
  or discards the partial temp file (receiver). Shares the per-session
  replay-protected sequence space like every other framed message.
- `Ping`
- `TypingStart`
- `TypingStop`
- `Rekey`
- `Ack`

## Replay Protection

Application messages carry sequence numbers.

The transport rejects messages that are:

- duplicated
- older than the last accepted sequence
- out of order

Handshake-only messages without sequence numbers bypass that validation.

## Large text messages

Text larger than `TEXT_CHUNK_BYTES` (48 KiB) is split by the app layer into
`TextChunk` messages (`message_id`, `chunk_index`, `total_chunks`, `text_part`) and
reassembled by the receiver into one logical message. A single message is
hard-capped at `MAX_TEXT_MESSAGE_BYTES` (64 KiB); encoding or decoding a larger one
is rejected. Each chunk carries its own `seq`, so replay protection covers chunks
exactly like file chunks.

## Delivery receipts

`Ack { acked_seq, seq }` acknowledges that the frame the peer sent with
transport sequence `acked_seq` was received and processed: a text message
recorded in history (the single `Text` frame, or the final `TextChunk` of a
large one), or a file finalized on disk (`FileEnd` — for a gated transfer the
receipt is sent only when the user accepts it). The sender correlates the
receipt via the wire seq its session loop stamped on the outgoing frame and
marks the message `delivered`.

`Ack` carries its own `seq` and shares the session sequence namespace, so it
is replay-protected like every other frame. Peers that predate the variant
drop the unknown tag; since replay validation only requires strictly
increasing sequences (gaps are fine), interoperability is unaffected — the
sender's messages simply never show as delivered.

## Rekeying

The transport rotates the session key automatically during the message loop
(`core/src/network/session.rs`):

- **only the host initiates** a rekey (deterministic single initiator): after
  `REKEY_MESSAGE_COUNT` (100) messages or `REKEY_TIME_INTERVAL` (5 min), the host
  sends a `Rekey { nonce, seq }` carrying a fresh `REKEY_NONCE_SIZE` (16-byte) salt.
  A single initiator rules out both sides rotating in the same round trip. The
  host rekeys on its keep-alive tick too, so a host that is only receiving still
  rotates on schedule.
- both peers independently derive the next key as
  `rekey_session_key(current_key, nonce)` (HKDF), and all subsequent frames use it
- the `Rekey` frame shares the session `seq` namespace (so it is replay-protected)
  and is consumed by the transport — it is never surfaced to the application
- the initiator applies the new key immediately after sending (the frame itself is
  encrypted under the old key); the receiver switches on receipt
- **In-flight old-key frames:** because the peer keeps sending under the old key
  until it processes the `Rekey`, the receiver retains the *previous* key and
  tries it as a fallback when the current key fails to decrypt. The retained key
  is dropped as soon as a frame decrypts under the current key (proof the peer
  has switched). This bounded dual-key window is what prevents the rotation from
  dropping the session on either the simultaneous-initiation path or the
  old-key-still-in-flight path; a frame that decrypts under *neither* key is
  treated as genuine desync/tampering and fails the session closed.
- **Limitation:** rekeying folds in no new DH material, so this provides forward
  secrecy but **not** post-compromise security — see `SECURITY.md`.

## Invite Links

### Legacy v1

Format:

```text
chat-p2p://invite/<base64_json>
```

Properties:

- unsigned
- still accepted for compatibility
- should not be emitted by current UI flows

### Signed v2/v3/v4

Format:

```text
chat-p2p://invite/v2/<url_safe_base64_json>
```

The URL prefix is `v2` for all signed invites. The **payload** carries its own
`version` field: `2` for a direct invite, `3` when the invite embeds a relay
route (`relay_server`/`relay_token` present), `4` when it carries several
direct-connect candidate addresses (`addresses` present). References to
"v3/v4 invites" elsewhere in the docs mean this payload version — the URL
format is unchanged, so older parsers reject newer payload fields gracefully
rather than failing on an unknown URL prefix.

Payload fields:

- `version` (`2` direct, `3` relay-routed, `4` multi-address)
- `timestamp`
- `nonce`
- `name`
- `address` (primary/first candidate — what pre-v4 clients connect to)
- `fingerprint`
- `public_key`
- `relay_server` (optional, payload v3)
- `relay_token` (optional, payload v3)
- `addresses` (optional, payload v4): all direct-connect candidates in
  priority order (e.g. UPnP external address first, LAN second); peers try
  them in turn with a bounded per-attempt timeout. **Serialization rule:**
  the field is omitted entirely when it would carry fewer than 2 entries,
  and both the generator and verifier use the same omit-when-empty rule so
  invites minted before this field existed re-serialize byte-identically
  and their signatures keep verifying.

Wrapper fields:

- `payload`
- `signature`

Current verification behavior:

- the payload is serialized using the app’s `serde_json::to_string()` representation
- the RSA-PSS signature is verified against those exact bytes
- the signed timestamp is enforced: future-dated invites (beyond clock skew)
  and invites older than the expiry window are rejected
- invalid addresses are dropped during import rather than trusted blindly;
  candidate lists are deduplicated preserving order
- relay route data is preserved when present in a signed invite

## Relay Rendezvous and Hole Punching

The relay control protocol (`core/src/network/relay.rs`) runs over the same
length-prefixed framing, carrying bincode-encoded enums. Because bincode
encodes the variant index, **new variants are append-only**.

Requests (client → server): `Host { token }` / `Join { token }` (legacy,
always bridged) and `HostV2 { token, punch }` / `JoinV2 { token, punch }`
(punch-capable; `punch.local_addrs` lists LAN candidates as `ip:port`,
already carrying the punch source port). Responses (server → client):
`Waiting`, `Paired`, `Error(String)`, and `PunchStart { peer_public,
peer_locals }`. After a `PunchStart`, each client answers with a
`PunchOutcome { success }` report.

Flow:

1. Both peers register with the same 32-hex-char token. The server observes
   each peer's public endpoint from the control connection's source address —
   it is never self-reported.
2. If **both** registered punch-capable, the server sends each side a
   `PunchStart` with the other's observed public endpoint and LAN candidates,
   then waits (bounded) for both `PunchOutcome`s.
3. Each peer re-binds the control connection's local port
   (`SO_REUSEADDR`/`SO_REUSEPORT`; the control connection itself is dialed
   from a reuse-enabled socket), listens on it, and dials every candidate in
   parallel with retries — a TCP simultaneous open. Every established socket
   is validated by exchanging a 25-byte hello: magic `P2PPNCH1` (8) + role
   byte (1; host=0, joiner=1; must differ) + tag (16, first 16 bytes of
   SHA-256 of the token). The host then sends `0xA5` (SELECT) on its chosen
   socket and the joiner confirms with `0x5A` (ACK), so both ends provably
   settle on one connection.
4. Both reports `success: true` → the server drops both control connections;
   the session runs on the punched socket (peer label `p2p:<addr>`).
   Anything else → the server sends `Paired` to both and bridges bytes
   (`copy_bidirectional`; peer label `relay:<server>`).

The punch hello tag is rendezvous pairing, not authentication: whichever
socket wins carries the full v3 handshake (ECDH, identity proof, TOFU).
Compatibility: a legacy peer simply gets the bridged path (the server never
starts a punch phase unless both sides are capable); a legacy *server* drops
the unknown V2 variant, which new clients detect (connection closed before
any response) and silently re-register in legacy mode. To keep that detection
unambiguous, a new server acknowledges every accepted join with an immediate
`Waiting` frame before handing the socket to the host — so a later disconnect
(e.g. the host vanished at the rendezvous) is treated as a genuine failure
rather than mistaken for a legacy server and pointlessly retried.
`P2PEM_NO_HOLEPUNCH=1` disables punching client-side.

## Party (Communities) protocol

Rides *on top of* an established v3 tunnel to a community server: the handshake
authenticates and encrypts the channel, and these frames carry the application
semantics. Defined in `core/src/party/mod.rs`, shared verbatim by client and
server. All frames are bincode-serialized `PartyRequest` / `PartyResponse`.

### Framing and replay protection

Every Party frame carries an 8-byte big-endian sequence number **inside** the
encryption, so it is covered by the AEAD tag:

```text
encrypt(seq_be_u64 || bincode(message), aad = transport_aad)
```

Each direction of each connection keeps its own `FrameSeq`. A frame whose
sequence does not *advance* the counter is rejected and the connection drops.
Without this an on-path attacker could replay a captured `PostMessage` — or a
server's `Message` broadcast — and both ends would accept it as new.

### Authorization model

- **`Role`** — `Guest < Member < Admin < Owner`, ordered so checks read as
  `role >= Role::Admin`. The **first identity to join becomes the Owner**; every
  later one is a Member. A role may only be granted strictly below the granter's
  own, the owner is never demoted, and a second owner is never created.
- **`ChannelKind`** — `Public` (all read/write), `Locked` and `Announce` (all
  read, admins write), `Private` (only the channel's `members` list, plus admins).
  `ListChannels` is filtered per member, so a private channel does not reveal its
  existence to a non-member.
- **Files** are access-checked at the *download* endpoint against the `file_refs`
  table: a member may fetch a blob only if some surviving reference to it sits in
  a channel they may read or a DM thread they are in. Unknown and forbidden give
  the same answer, so the endpoint never reveals a file's existence.

### Requests

`Join`, `ListMembers`, `ListChannels`, `PostMessage`, `FetchHistory`, `SendDm`,
`FetchDmHistory`, `CreateChannel`, `PostFile`, `SendFileDm`, `DownloadFile`,
then the appended governance set: `CreateChannelOfKind`, `DeleteChannel`,
`SetChannelAccess`, `SetRole`, `ListFiles`, `DeleteFile`, `FetchAuditLog`,
`FetchQuota`.

### Responses

`Joined`, `JoinRejected`, `Members`, `Channels`, `MessagePosted`, `Message`,
`History`, `FileData`, `Error`, `ActionFailed`, then the appended set: `Files`,
`Quota`, `AuditLog`, `Ok`, `DirectoryChanged`.

- **`ActionFailed`** exists separately from `Error` because the client appends
  outgoing messages optimistically and has to know a refusal belongs to the
  message still on screen, so it can take it back rather than leave the user
  believing it was delivered. Correlated FIFO against the client's pending-send
  queue.
- **`DirectoryChanged`** is a nudge to re-request `ListChannels`, not the list
  itself. The list is per member and the hub fans one identical frame out to
  every connection, so broadcasting it would either leak private channels or
  hide them from their own members.

### History paging

`FetchHistory` / `FetchDmHistory` return at most `MAX_HISTORY_BATCH` (200)
envelopes; the client asks again with the last `seq` it received. A whole channel
in one frame stopped fitting past `MAX_PACKET_SIZE` once history grew, and the
send failure dropped the connection with nothing on screen to explain it. The
client **merges** pages by server-assigned sequence rather than replacing.

### Files

Uploads are inline, bounded by `MAX_INLINE_FILE_BYTES` (4 MiB); chunked transfer
for larger files is not yet implemented. Blobs are content-addressed by SHA-256,
deduplicated, and reference-counted per share. `DeleteFile` drops one reference
and reclaims the bytes when the last one goes — but **leaves the message in
history**, because sequence numbers are the identity clients merge on and
removing an envelope would renumber the channel for everyone who already had it.
Clients verify downloaded bytes against the requested hash; the hash *is* the
integrity check.

### Compatibility

The Party protocol is append-only: new variants go on the end of each enum,
because bincode encodes a variant's index. Even so, **community clients and
servers must ship together** across the release that added the per-frame
sequence number and `MemberInfo::role` / `ChannelInfo::members` — an older
client against a newer server drops the connection at the first frame. Anyone
self-hosting a community server has to restart it on the matching version.

## Compatibility Notes

- history/storage migrations are separate from wire compatibility
- v1 invite support remains for backward compatibility
- protocol docs must be updated whenever message encoding, handshake ordering, or trust semantics change
