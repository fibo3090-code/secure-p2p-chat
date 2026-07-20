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
2. Exchange X25519 ephemeral public keys in plaintext.
3. Derive a shared session key using HKDF-SHA256.
4. Establish encrypted transport with AES-256-GCM.
5. Exchange encrypted `IdentityProof` messages inside that tunnel.
6. Verify the peer's identity signature.
7. Optional connection-password gate (see below).
8. Confirm the peer fingerprint (TOFU) before surfacing the session.
9. Enter the message loop.

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

## Compatibility Notes

- history/storage migrations are separate from wire compatibility
- v1 invite support remains for backward compatibility
- protocol docs must be updated whenever message encoding, handshake ordering, or trust semantics change
