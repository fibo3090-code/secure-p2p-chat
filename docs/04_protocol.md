# Protocol Specification

This document describes the current wire behavior of the application. It should only describe shipped runtime behavior, not roadmap ideas.

## Constants

```rust
PORT_DEFAULT: 12345
MAX_PACKET_SIZE: 8 MiB
FILE_CHUNK_SIZE: 64 KiB
AES_KEY_SIZE: 32
AES_NONCE_SIZE: 12
RSA_KEY_BITS: 2048
HANDSHAKE_TIMEOUT_SECS: 15
MAX_FILE_SIZE: 10 GiB
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
6. Verify identity signatures and fingerprints.
7. Enter the message loop.

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
- `FileMeta`
- `FileChunk`
- `FileEnd`
- `Ping`
- `TypingStart`
- `TypingStop`
- `Rekey`

## Replay Protection

Application messages carry sequence numbers.

The transport rejects messages that are:

- duplicated
- older than the last accepted sequence
- out of order

Handshake-only messages without sequence numbers bypass that validation.

## Rekeying

The transport supports rekeying.

Current runtime behavior:

- rekeying is handled at the transport layer
- the first valid rekey event observed on the active session is processed
- the runtime does not implement a separate multi-message nonce tie-break exchange

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

### Signed v2

Format:

```text
chat-p2p://invite/v2/<url_safe_base64_json>
```

Payload fields:

- `version`
- `timestamp`
- `nonce`
- `name`
- `address`
- `fingerprint`
- `public_key`
- `relay_server` (optional)
- `relay_token` (optional)

Wrapper fields:

- `payload`
- `signature`

Current verification behavior:

- the payload is serialized using the app’s `serde_json::to_string()` representation
- the RSA-PSS signature is verified against those exact bytes
- the timestamp is informational only and is not used for expiry enforcement
- invalid addresses are dropped during import rather than trusted blindly
- relay route data is preserved when present in a signed invite

## Compatibility Notes

- history/storage migrations are separate from wire compatibility
- v1 invite support remains for backward compatibility
- protocol docs must be updated whenever message encoding, handshake ordering, or trust semantics change
