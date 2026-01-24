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

## 4.5. Message Format

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
2. Serialize the payload back to JSON (deterministically) to recreate the signed message
3. Extract the public key from `payload.public_key`
4. Verify: `RSA_Verify_PSS(public_key, payload_json, signature)` with SHA-256
5. If verification succeeds, accept the contact information as authentic
6. If verification fails, reject the invite link as tampered

**Security Benefits**:

- **Integrity**: RSA-PSS signature prevents any tampering with the invite link
- **Authenticity**: The signature is created with the identity's private key, proving the invite originated from the claimed sender
- **Uniqueness**: Each invite includes an ephemeral nonce (one-time random Ed25519 key) for uniqueness
- **Timestamp**: Invites include a timestamp field for future expiration support

**Implementation Notes**:

- V2 invites use **URL-safe base64** (RFC 4648 without padding) to avoid URL encoding/decoding issues
- The `nonce` field contains a hex-encoded Ed25519 public key generated ephemeralally per invite
- The timestamp is always in UTC seconds (UNIX epoch)
- Invites are transport-layer unique via the nonce, with future versions adding expiration support

