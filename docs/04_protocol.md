# 4. Protocol Specification

This document details the network protocol used by the Encrypted P2P Messenger. A strict adherence to this protocol is required for compatibility between different versions of the application.

## 4.1. Constants

The following constants are defined to ensure that all clients can communicate effectively.

```rust
const PORT_DEFAULT: u16 = 12345;
const MAX_PACKET_SIZE: usize = 8 * 1024 * 1024;  // 8 MiB
const FILE_CHUNK_SIZE: usize = 64 * 1024;         // 64 KiB
const AES_KEY_SIZE: usize = 32;                   // 256 bits
const AES_NONCE_SIZE: usize = 12;                 // 96 bits (GCM standard)
const RSA_KEY_BITS: usize = 2048;
const HANDSHAKE_TIMEOUT_SECS: u64 = 15;
```

## 4.2. Cryptography

The protocol relies on a combination of cryptographic primitives to ensure confidentiality, integrity, and authenticity.

- **RSA**: 2048-bit RSA with OAEP padding and SHA-256 (RSA-OAEP-SHA256) is used for Identity Proofs (signatures) during the handshake. RSA support is maintained for backward compatibility.
- **Ed25519**: Edwards-curve Digital Signature Algorithm (Ed25519) is supported as a modern alternative to RSA-2048. New identities default to Ed25519.
- **Signature Scheme Negotiation**: The `IdentityProof` message includes the `SignatureScheme` (RSA = 0, Ed25519 = 1) to allow peers to support multiple signature algorithms simultaneously.
- **AES**: AES-256-GCM is used for symmetric encryption of all messages after the handshake is complete.
- **Nonce**: A 12-byte (96-bit) nonce is used. It is **counter-based** (4 bytes random session ID + 8 bytes counter) to guarantee uniqueness and prevent replay attacks within the session.
- **Sequence Numbers**: All messages include a sequence number (`seq`) for replay detection at the transport layer.
- **Fingerprint**: The fingerprint of a user's public key is the SHA-256 hash of the PEM-encoded key (RSA) or the raw public key bytes (Ed25519), represented as a lowercase hexadecimal string.
- **Transport Format (Encrypted)**: Encrypted messages are sent over the wire in the following format: `nonce(12) || ciphertext || tag(16)`.

## 4.3. Network Protocol

### TCP Framing (Length-Prefixed)

Messages are framed using a simple length-prefix scheme:
`Length (4 bytes, Big Endian) || Payload (N bytes)`

Maximum packet size is 8 MB.

### Handshake (Protocol v3)

The handshake uses an **ECDH-first** approach (Protocol v3) to ensure forward secrecy and identity privacy.

1. **Version Exchange**: Peers exchange `u32` (Big Endian) protocol version. Must be >= 3.
2. **Ephemeral Key Exchange (Plaintext)**:
   - Peers exchange 32-byte X25519 ephemeral public keys.
   - These keys are unique to the session.
3. **Session Key Derivation**:
   - `SharedSecret = ECDH(MyEphemeralPriv, PeerEphemeralPub)`
   - `SessionKey = HKDF-SHA256(SharedSecret)`
   - An encrypted tunnel (AES-256-GCM) is established immediately.
4. **Encrypted Identity Exchange**:
   - Peers exchange `IdentityProof` messages inside the encrypted tunnel.
   - `IdentityProof` contains:
     - `signature_scheme`: Negotiated scheme (0 = RSA, 1 = Ed25519)
     - `public_key_pem`: Identity Key (RSA or Ed25519 in PEM format)
     - `signature`: Signature of the session's Ephemeral Key
       - RSA: `RSA_Sign(SHA256("IDENTITY_PROOF" || MyEphemeralPub))`
       - Ed25519: `Ed25519_Sign("IDENTITY_PROOF" || MyEphemeralPub)`
   - The signature binds the ephemeral key to the long-term identity, preventing MITM.
5. **Fingerprint Verification**:
   - The received identity key fingerprint is checked against trusted contacts.
   - RSA fingerprint: SHA-256 of PEM-encoded key
   - Ed25519 fingerprint: SHA-256 of raw public key bytes

## 4.4. Replay Protection

All messages include a **sequence number** (`seq: u64`) assigned by the sender. The transport layer maintains per-session state:

- **Sender**: Increments `seq` with each message (starting from 1)
- **Receiver**: Tracks `last_recv_seq` and rejects any message with `seq <= last_recv_seq`
- **Effect**: Out-of-order, duplicate, and old messages are rejected before emission to the application

This provides defense-in-depth against replay attacks at the protocol level, complementing nonce-based replay detection in the encryption layer.

## 4.5. Session Key Rotation (Rekeying)

To provide **perfect forward secrecy** for long-running sessions, session keys are rotated periodically. This ensures that if a session key is ever compromised, only recent messages are exposed.

### Rekeying Schedule

Keys are rotated when either condition is met (whichever comes first):

1. **Message Count**: Every 100 messages sent/received
2. **Time Interval**: Every 5 minutes of session activity

### Rekeying Process

1. **Initiator** (either peer that detects rekeying condition):
   - Generate a random 16-byte nonce using `generate_rekey_nonce()`
   - Create a `Rekey` message with the nonce
   - Send the message encrypted with the current session key
   - Derive next key: `next_key = HKDF-SHA256(current_key, nonce, "key-rotation")`
   - Update local cipher to use the new key

2. **Receiver**:
   - Receives the `Rekey` message (which is already authenticated by the current key's encryption)
   - Validates sequence number (like any other message)
   - Extracts the nonce from the message
   - Derives the same next key using the same nonce and current key
   - Updates cipher to use the new key
   - Does NOT emit the `Rekey` message to the application (it's a protocol-level operation)

3. **Simultaneous Rekey Resolution**:
   - If both peers initiate a rekey at nearly the same time, each will send their own `Rekey` message before receiving the other's
   - When a peer receives a `Rekey` while having initiated its own, it must compare nonces deterministically:
     - Compare nonces using lexicographic byte order (smaller nonce "wins")
     - Both peers derive: `next_key = HKDF-SHA256(current_key, winning_nonce, "key-rotation")`
     - The peer whose nonce lost abandons its locally-generated nonce and does NOT send a second rotation
     - Both peers update cipher using the winning nonce and reset counters
   - This ensures deterministic, symmetric resolution without race conditions or duplicate rotations

4. **Both Peers** (after rekey, whether single or simultaneous):
   - Reset message counter to 0
   - Reset rekey timer
   - Continue encrypted communication with the new key

### Rekey Message Format

```
Type: 10 (u8)
Sequence: seq (u64, Big Endian)
Nonce Length: nonce_len (u32, Big Endian)
Nonce: [u8; nonce_len]
```

The nonce is typically 16 bytes but implementations MUST support variable-length nonces (up to 256 bytes) for future extensibility.

### Security Properties

- **Forward Secrecy**: Compromised session keys don't expose past messages (covered by previous keys before rotation)
- **Deterministic Derivation**: Both peers independently derive the same next key using HKDF, ensuring agreement without additional key exchange
- **No Replay Risk**: Rekeying uses the current (validated) encrypted tunnel; the `Rekey` message itself is authenticated
- **Transparent to Application**: Rekeying is handled at the transport layer; no API changes needed

### Performance

- Rekeying operations (HKDF expansion) are fast: ~0.03ms per operation on modern hardware
- Message overhead: One extra ~30-50 byte message every 100 messages or 5 minutes
- Negligible impact on throughput and latency

## 4.6. Message Format

All application messages are of the `ProtocolMessage` enum type, serialized using `bincode`.

```rust
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub enum ProtocolMessage {
    Version { version: u8 },
    EphemeralKey { public_key: Vec<u8> },
    
    Text { 
        text: String, 
        timestamp: u64,
        seq: u64 
    },
    
    FileMeta { 
        filename: String, 
        size: u64, 
        seq: u64 
    },
    
    FileChunk { 
        chunk: Vec<u8>, 
        seq: u64 
    },
    
    FileEnd { seq: u64 },
    
    Ping { seq: u64 },
    TypingStart { seq: u64 },
    TypingStop { seq: u64 },
}
## 4.6. Invite Links

Invite links are used to share contact information securely. Two versions are supported:

### V1 (Legacy, Unsigned)

Format: `chat-p2p://invite/<base64_json>`

V1 invites are unsigned base64-encoded JSON objects:

```json
{
  "name": "Alice",
  "address": "192.168.1.10:12345",
  "fingerprint": "a1b2c3d4e5f6...",
  "public_key": "-----BEGIN PUBLIC KEY-----\n..."
}
```

**Security Note**: V1 invites lack cryptographic integrity protection. An attacker could intercept and modify the invite link to swap fingerprints or addresses. V1 invites are deprecated and supported only for backward compatibility.

### V2 (Signed with RSA-PSS)

Format: `chat-p2p://invite/v2/<url_safe_base64_json>`

V2 invites are cryptographically signed with RSA-PSS-SHA256 to prevent tampering. The structure is:

```json
{
  "payload": {
    "version": 2,
    "timestamp": 1704067200,
    "nonce": "a1b2c3d4e5f6g7h8i9j0k1l2m3n4o5p6",
    "name": "Alice",
    "address": "192.168.1.10:12345",
    "fingerprint": "a1b2c3d4e5f6...",
    "public_key": "-----BEGIN PUBLIC KEY-----\n..."
  },
  "signature": "<RSA-PSS signature bytes>"
}
```

**Signature Verification Process**:

1. Extract the `payload` and `signature` from the invite
2. Serialize the payload back to JSON using canonical serialization (RFC 8785 JSON Canonicalization: sorted keys, no whitespace, deterministic formatting)
3. Extract the public key from `payload.public_key`
4. Verify using RFC 8785 canonical payload bytes: `RSA_Verify_PSS(public_key, canonical_payload_bytes, signature)` with SHA-256
   - Use exact byte-for-byte canonical form for verification to ensure interoperability
5. If verification succeeds, accept the contact information as authentic
6. If verification fails, reject the invite link as tampered

**Canonical Serialization Requirement**: Implementations MUST use RFC 8785 JSON Canonicalization (UTF-8 encoding, sorted object member order, no insignificant whitespace, deterministic number formatting) for the signed payload. This ensures all compliant implementations produce identical bytes for verification.

**Security Benefits**:

- **Integrity**: RSA-PSS signature prevents any tampering with the invite link
- **Authenticity**: The signature is created with the identity's private key, proving the invite originated from the claimed sender
- **Uniqueness**: Each invite includes a unique random nonce (raw bytes) for transport-layer uniqueness
- **Non-Expiring**: Invites do not expire because the timestamp field is not validated during verification

**Implementation Notes**:

- V2 invites use **URL-safe base64** (RFC 4648 without padding) to avoid URL encoding/decoding issues
- The `nonce` field contains a hex-encoded random value (unique per invite, not an Ed25519 key)
- The timestamp is always in UTC seconds (UNIX epoch) but is NOT validated for expiry
- Invites are transport-layer unique via the nonce; they do not expire

