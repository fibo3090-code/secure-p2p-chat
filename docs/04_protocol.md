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

- **RSA**: 2048-bit RSA with OAEP padding and SHA-256 (RSA-OAEP-SHA256) is used for Identity Proofs (signatures) during the handshake.
- **AES**: AES-256-GCM is used for symmetric encryption of all messages after the handshake is complete.
- **Nonce**: A 12-byte (96-bit) nonce is used. It is **counter-based** (4 bytes random session ID + 8 bytes counter) to guarantee uniqueness and prevent replay attacks within the session.
- **Fingerprint**: The fingerprint of a user's public key is the SHA-256 hash of the PEM-encoded key, represented as a lowercase hexadecimal string.
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
     - `public_key_pem`: RSA Identity Key.
     - `signature`: `RSA_Sign(SHA256("IDENTITY_PROOF" || MyEphemeralPub))`
   - The signature binds the ephemeral key to the long-term identity, preventing MITM.
5. **Fingerprint Verification**:
   - The received RSA key fingerprint is checked against trusted contacts.

## 4.4. Message Format

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
```

## 4.5. Invite Links

Invite links are base64-encoded JSON objects:

```json
{
  "name": "Alice",
  "address": "192.168.1.10:12345", // Optional
  "fingerprint": "a1b2c3d4e5f6...",
  "public_key": "-----BEGIN PUBLIC KEY-----\n..."
}
```
