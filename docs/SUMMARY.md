# Summary of Project Documentation

This document provides a high-level summary of the Encrypted P2P Messenger project based on the existing documentation.

## Project Overview

The project is a **secure, peer-to-peer desktop messaging application** built with Rust and the `egui` library. Its primary goal is to provide a private and secure communication channel without relying on a central server.

## Core Features

*   **End-to-End Encryption**: Utilizes a combination of RSA for identity and AES-256-GCM for message encryption.
*   **Forward Secrecy**: Implemented using the X25519 Elliptic Curve Diffie-Hellman (ECDH) key exchange to protect past conversations even if long-term keys are compromised.
*   **Peer-to-Peer (P2P) Architecture**: Direct communication between users on a local network (LAN) or VPN, eliminating the need for a central server.
*   **File Transfer**: Supports sending and receiving files with chunking.
*   **User-Friendly Interface**: A modern GUI with features like typing indicators, emoji support, and desktop notifications.
*   **Local Persistence**: Chat history and user identity are stored locally on the user's device.

## Key Technologies

*   **Programming Language**: Rust
*   **GUI Framework**: `egui`
*   **Asynchronous Runtime**: `tokio`
*   **Cryptography Libraries**: `rsa`, `aes-gcm`, `x25519-dalek`, `hkdf`
*   **Serialization**: `serde`, `serde_json`, `bincode`

## Architecture

The application follows a layered architecture:

1.  **GUI Layer**: Handles user interaction (built with `egui`).
2.  **Business Logic Layer**: Manages the application's state, including chats and sessions.
3.  **Core Layers**:
    *   **Network**: Manages TCP connections and the communication protocol.
    *   **Crypto**: Implements all cryptographic operations.
    *   **Transfer**: Handles file transfers.
    *   **Identity**: Manages the user's persistent RSA keys.

## Protocol

The application uses a custom TCP-based protocol (version 2) with the following characteristics:

*   **Length-Prefixed Framing**: Each message is prefixed with its length.
*   **Secure Handshake**: A multi-step handshake process establishes a secure session, including:
    *   Version negotiation to prevent downgrade attacks.
    *   Exchange of RSA public keys for identity verification.
    *   Exchange of ephemeral X25519 keys to ensure forward secrecy.
    *   Derivation of a session-specific AES key using HKDF.

## Contribution and Development

The project has clear guidelines for contributions, including:

*   **Conventional Commits** for commit messages.
*   A requirement for `cargo fmt` and `cargo clippy` to be run before submitting pull requests.
*   A well-defined branching strategy and release process.

## Roadmap

The project has an ambitious roadmap for future development, with plans for:

*   **v2.0**: Features like automatic peer discovery (mDNS), NAT traversal for internet connectivity, and message search.
*   **v3.0**: Advanced features such as post-quantum cryptography, mobile applications, and voice/video calls.

## Security

Security is a primary focus of the project, with:

*   A detailed **threat model** that considers eavesdropping, tampering, and key compromise.
*   A strong emphasis on **fingerprint verification** to prevent man-in-the-middle attacks.
*   A responsible **vulnerability disclosure policy**.
