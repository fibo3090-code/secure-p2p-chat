# Security Policy

This document outlines the security policy for the Encrypted P2P Messenger, including the threat model, cryptographic specifications, and guidelines for reporting vulnerabilities.

## Security Overview

This application implements **military-grade end-to-end encryption** with **forward secrecy**, matching the security standards of leading messaging apps like Signal and WhatsApp.

### Threat Model

The application is designed to protect against the following threats:

-   **Eavesdropping**: All messages are encrypted end-to-end, making them unreadable to anyone who intercepts the traffic.
-   **Tampering**: GCM authentication tags are used to detect any modification of messages in transit.
-   **Replay Attacks**: Random nonces are used for each message to prevent attackers from replaying old messages.
-   **Key Compromise**: Forward secrecy, achieved through the X25519 ECDH key exchange, ensures that past sessions remain secure even if long-term identity keys are compromised.
-   **Downgrade Attacks**: The protocol includes a version negotiation step to prevent attackers from forcing the use of a weaker, outdated protocol.

### Assumptions

This security model makes the following assumptions:

-   Users **verify fingerprints** on the first connection to prevent man-in-the-middle attacks.
-   The operating system is **not compromised**.
-   The application is used on a **trusted network** (e.g., a home LAN or a secure VPN).

## Cryptographic Specifications

### Encryption Primitives

-   **Message Encryption**: AES-256-GCM
-   **Key Exchange**: X25519 ECDH
-   **Identity**: RSA-2048-OAEP
-   **Fingerprinting**: SHA-256

### Forward Secrecy

Forward secrecy is a critical feature of this application, ensuring that a compromise of long-term keys does not compromise past session keys. This is achieved as follows:

1.  **Ephemeral Keys**: For each new session, a new X25519 key pair is generated. These keys are used only once and are discarded at the end of the session.
2.  **Key Derivation**: The shared secret derived from the ECDH key exchange is used as input to a Key Derivation Function (HKDF-SHA256) to generate a unique 32-byte AES-256 session key.
3.  **Identity vs. Encryption**: Long-term RSA keys are used only for identity verification (fingerprints) and are not used for session encryption.

### Handshake Sequence (Protocol v2)

The handshake process is designed to be secure and robust:

1.  **Version Negotiation**: Both peers exchange and verify the protocol version to prevent downgrade attacks.
2.  **RSA Public Key Exchange**: Peers exchange their long-term RSA public keys for identity and fingerprint verification.
3.  **X25519 Ephemeral Key Exchange**: For each session, new ephemeral X25519 keys are exchanged to provide forward secrecy.
4.  **ECDH Computation**: A shared secret is computed using the ephemeral keys.
5.  **HKDF-SHA256 Key Derivation**: The final AES session key is derived from the shared secret.
6.  **Encrypted Communication**: All subsequent communication is encrypted with the derived session key.

## Reporting Security Issues

If you discover a security vulnerability, please **DO NOT** open a public GitHub issue. Instead, please report the vulnerability by emailing `security@example.com` (replace with a real address). We will investigate all reports and do our best to fix the issue as soon as possible.

### Contribution Guidelines

Please refer to [CONTRIBUTING.md](CONTRIBUTING.md) for details on reporting bugs, suggesting features, and the pull request process.
