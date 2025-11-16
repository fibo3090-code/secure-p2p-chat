# 7. Security

This document outlines the security policy for the Encrypted P2P Messenger, including the threat model, cryptographic specifications, and guidelines for reporting vulnerabilities.

## Security Overview

This application implements **military-grade end-to-end encryption** with **forward secrecy**, matching the security standards of leading messaging apps like Signal and WhatsApp. The security of the application is a top priority, and this document provides a transparent overview of the measures taken to protect users' privacy and data.

### Threat Model

The application is designed to protect against the following threats:

-   **Eavesdropping**: An attacker who has access to the network traffic between two peers will not be able to read the content of the messages. All messages are encrypted end-to-end.
-   **Tampering**: An attacker cannot modify messages in transit without being detected. The use of GCM authentication tags ensures the integrity and authenticity of every message.
-   **Replay Attacks**: An attacker cannot capture and resend old messages. A unique, randomly generated nonce is used for each message, preventing them from being replayed.
-   **Key Compromise**: The compromise of a user's long-term identity keys will not compromise the security of past conversations. Forward secrecy, achieved through the X25519 ECDH key exchange, ensures that each session has a unique set of keys that are discarded after the session ends.
-   **Downgrade Attacks**: An attacker cannot force the application to use a weaker, outdated version of the protocol. The handshake process includes a version negotiation step to prevent this.

### Assumptions

This security model makes the following assumptions:

-   **Users verify fingerprints**: The security of the initial connection depends on the users verifying each other's fingerprints through a secure, out-of-band channel. This is the most critical step in preventing man-in-the-middle attacks.
-   **The operating system is not compromised**: The application cannot protect against threats that originate from a compromised operating system, such as keyloggers or malware that can read the application's memory.
-   **The application is used on a trusted network**: While the application is designed to be secure even on untrusted networks, it is recommended to use it on a trusted network, such as a home LAN or a secure VPN, for an additional layer of security.

### Key Handling & Persistence

-   **Identity Keys**: Long-term RSA-2048 identity keys are generated locally on the user's device and stored on disk. Future versions of the application will include support for an encrypted keystore, which will protect these keys with a user-provided password.
-   **Session Keys**: Ephemeral AES-256-GCM session keys are derived for each session using X25519 ECDH and HKDF. These keys are kept in memory only for the duration of the session and are never written to disk.
-   **Fingerprints**: A user's fingerprint is the SHA-256 hash of their PEM-encoded public key, represented as a lowercase hexadecimal string.

## Cryptographic Specifications

### Encryption Primitives

-   **Message Encryption**: AES-256-GCM (Galois/Counter Mode) provides both authenticated encryption and additional authenticated data (AEAD).
-   **Key Exchange**: X25519 Elliptic Curve Diffie-Hellman (ECDH) is used for the key exchange, providing a high level of security and performance.
-   **Identity**: RSA-2048 with OAEP padding is used for the long-term identity keys.
-   **Fingerprinting**: SHA-256 is used to generate fingerprints for public keys.

### Forward Secrecy

Forward secrecy is a critical feature of this application. It ensures that a compromise of a user's long-term keys does not compromise the security of their past conversations. This is achieved as follows:

1.  **Ephemeral Keys**: For each new session, a new X25519 key pair is generated. These keys are used only once and are discarded at the end of the session.
2.  **Key Derivation**: The shared secret derived from the ECDH key exchange is used as input to a Key Derivation Function (HKDF-SHA256) to generate a unique 32-byte AES-256 session key.
3.  **Identity vs. Encryption**: The long-term RSA keys are used only for identity verification (via fingerprints) during the handshake. They are not used for session encryption.

### Handshake Sequence (Protocol v2)

The handshake process is designed to be secure and robust:

1.  **Version Negotiation**: Both peers exchange and verify their supported protocol version to prevent downgrade attacks.
2.  **RSA Public Key Exchange**: Peers exchange their long-term RSA public keys for identity and fingerprint verification.
3.  **X25519 Ephemeral Key Exchange**: For each session, new ephemeral X25519 keys are exchanged to provide forward secrecy.
4.  **ECDH Computation**: A shared secret is computed using the ephemeral keys.
5.  **HKDF-SHA256 Key Derivation**: The final AES session key is derived from the shared secret.
6.  **Encrypted Communication**: All subsequent communication is encrypted with the derived session key.

## Reporting Security Issues

If you discover a security vulnerability, please **DO NOT** open a public GitHub issue. Instead, please report the vulnerability by emailing `[YOUR_SECURITY_EMAIL_ADDRESS_HERE]` (replace with a real address). We will investigate all reports and do our best to fix the issue as soon as possible.

When reporting, please include (to the extent possible):

-   A clear description of the issue and its potential impact.
-   Steps to reproduce the vulnerability and a proof-of-concept if available.
-   The affected versions or commits and details about your environment.
-   Any ideas you have for mitigation.
