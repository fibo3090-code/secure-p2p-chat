# 3. Architecture

This document provides a detailed look at the architecture of the Encrypted P2P Messenger, including its directory structure, layered design, and module responsibilities.

## 3.1. Directory Structure

The project is organized into the following directory structure:

```text
`chat-p2p/
├── src/
│   ├── main.rs (GUI application)
│   ├── lib.rs (Module exports)
│   ├── types.rs (Data structures)
│   ├── util.rs (Helpers)
│   │
│   ├── app/ (Business Logic Layer)
│   │   ├── chat_manager.rs (Core state management)
│   │   └── persistence.rs (JSON save/load)
│   │
│   ├── core/ (Cryptography Layer)
│   │   ├── crypto.rs (RSA, AES-GCM, X25519)
│   │   └── protocol.rs (Message types)
│   │
│   ├── network/ (Network Layer)
│   │   └── session.rs (TCP sessions, handshake)
│   │
│   ├── transfer/ (File Transfer Layer)
│   │   ├── receiver.rs (Receiving files)
│   │   └── sender.rs (Sending files)
│   │
│   └── identity/ (Identity Layer)
│       └── mod.rs (Persistent RSA keys)
│
├── Cargo.toml
├── README.md
└── SECURITY.md
`
```

## 3.2. Layered Architecture

The application is designed with a clear separation of concerns, following a layered architecture. This makes the codebase easier to understand, maintain, and test.

```text
┌─────────────────────────────────────┐
│   GUI Layer (egui/eframe)           │  ← Handles all user interaction and rendering.
└──────────────┬──────────────────────┘
               │ Shares state via Arc<Mutex<ChatManager>>
┌──────────────▼──────────────────────┐
│   Business Logic Layer (app)        │  ← Manages the application's core state and logic.
└──────────────┬──────────────────────┘
               │ Communicates with other layers via tokio channels
    ┌──────────┼──────────┬──────────┐
    │          │          │          │
┌───▼───┐  ┌──▼────┐  ┌──▼────┐  ┌──▼──────┐
│Network│  │Crypto │  │Transfer│ │Identity │  ← Core functionality layers.
│(TCP)  │  │(RSA/AES)│  │(Files) │ │(RSA Keys) │
└───────┘  └───────┘  └────────┘ └─────────┘
```

## 3.3. Module Responsibilities

- **`src/main.rs` - GUI Application**: The entry point of the application. It initializes the `egui` framework, sets up the main application state, and runs the event loop.

- **`src/app/chat_manager.rs` - Business Logic**: This is the "brain" of the application. It manages all application state, including the list of chats, contacts, and active network sessions. It also handles routing messages between the GUI and the network layer.

- **`src/identity/mod.rs` - Identity System**: Responsible for managing the user's persistent identity. This includes generating, loading, and saving the user's long-term RSA key pair.

- **`src/core/crypto.rs` - Cryptography**: This module contains all the cryptographic logic. It provides functions for RSA encryption/decryption, AES-GCM encryption/decryption, and the X25519 Diffie-Hellman key exchange.

- **`src/core/protocol.rs` - Wire Protocol**: Defines the structure of messages that are sent over the network. It uses `serde` for serialization and deserialization of these messages.

- **`src/network/session.rs` - Network Sessions**: Manages the lifecycle of a TCP connection between two peers. This includes the secure handshake process, sending and receiving messages, and handling connection errors.

- **`src/transfer/` - File Transfer**: This module implements the logic for sending and receiving large files by breaking them down into smaller chunks.

- **`src/types.rs` - Data Structures**: Contains the core data structures used throughout the application, such as `Chat`, `Message`, `Contact`, and various event enums.

### Notable Runtime Events

- `SessionEvent::NewConnection(chat_id, peer_meta)`: This event is a key part of the chat synchronization logic. It is emitted on the host's side when a client successfully connects and provides a `chat_id`. The `ChatManager` listens for this event to create or update the chat on the host's side, ensuring that both peers have a consistent view of the conversation.
