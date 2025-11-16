# 6. Roadmap

> "Design is not just what it looks like and feels like. Design is how it works." - Steve Jobs

This document outlines the development roadmap for the Encrypted P2P Messenger. The roadmap is guided by a design philosophy that prioritizes simplicity, user experience, and innovation.

## Design Philosophy

-   **Simplicity**: The application should be easy to use for everyone, regardless of their technical expertise. We strive to remove technical jargon from the UI and provide smart defaults.
-   **User Experience First**: Every feature should have a clear purpose and be intuitive to use. We aim to delight users with a polished and responsive interface.
-   **Integration**: All features should feel like they belong together, with a consistent design language and seamless integration.
-   **Innovation**: We want to leverage the peer-to-peer nature of the application to create unique and innovative features that are not possible with traditional client-server architectures.

## Current Status (v1.3.1)

### What Works

-   **Core Security**: RSA-2048-OAEP + AES-256-GCM encryption with X25519 ECDH Forward Secrecy.
-   **Messaging**: Bidirectional text messaging, file transfer, typing indicators, and desktop notifications.
-   **UI/UX**: A modern interface with avatars, timestamps, an emoji picker, and drag & drop file support.
-   **Persistence**: Reliable storage of chat history and a persistent user identity.
-   **Synchronized Chat Creation**: A recent update (v1.3.0) fixed a critical bug where new chats were not synchronized between peers, leading to "all recipients offline" errors.

### Known Limitations

1.  **LAN-only**: The application does not currently support communication over the internet (WAN) as it lacks NAT traversal capabilities.
2.  **Manual Fingerprint Verification**: Users must manually verify fingerprints to prevent man-in-the-middle attacks.
3.  **Some unit tests need updates** to reflect recent changes.

## High-Level Roadmap

-   **v1.3: The Usability Release**: This version focused on fixing critical bugs and improving the reliability of the core messaging experience.
-   **v2.0: The Professional Release**: This version will focus on features that make the application suitable for professional and enterprise use, such as moderation tools, NAT traversal, and message search.
-   **v3.0: The Next Generation**: This version will explore advanced features like post-quantum cryptography, mobile applications, and real-time voice/video calls.

## Detailed Feature Roadmap

### Sprint 1: Connection UX
*Goal: Make connecting to peers "just work".*

1.  **Smart Connection Discovery (mDNS/Bonjour)**: Automatically discover other users on the same network, removing the need for manual IP address entry.
2.  **QR Code Connection**: Scan a QR code on another user's device to instantly connect to them.
3.  **Visual Fingerprint Verification**: Replace long hex strings with more user-friendly verification methods, such as colored grids or memorable word lists.
4.  **Intelligent Error Messages**: Provide clear and actionable advice when connections fail.
5.  **Connection History**: Easily reconnect to peers you have connected to in the past.

### Sprint 2: Reliability
*Goal: Ensure that the application is reliable and never loses messages.*

1.  **Heartbeat/Keepalive System**: Periodically check the status of connections to detect and handle drops.
2.  **Auto-Reconnection**: Automatically attempt to reconnect to peers with an exponential backoff strategy.
3.  **Message Delivery Status (✓✓)**: Provide visual feedback on the status of messages (Sent, Delivered, Read).
4.  **Offline Message Queue**: Queue messages that are sent while a peer is offline and automatically send them when the peer reconnects.

### Sprint 3: Identity & Trust
*Goal: Create a robust and persistent identity system.*

1.  **Encrypted Keystore**: Protect the user's private key with a password using a strong key derivation function like Argon2.
2.  **First-Time Setup Wizard**: Guide new users through the process of creating their identity and setting a password.
3.  **Password Management UI**: Allow users to change their password.
4.  **Device Recognition (TOFU - Trust on First Use)**: Warn users if a contact's fingerprint changes, which could indicate a security issue.

### Sprint 4: Messaging++
*Goal: Add rich messaging features.*

1.  **Message Search**: Implement full-text search within and across all chats.
2.  **Rich Text Formatting (Markdown)**: Support for **bold**, *italics*, `code blocks`, and other formatting options.
3.  **File Transfer Polish**: Show inline previews for images and provide better progress visualization for all file transfers.
4.  **Reply to Specific Message**: Allow users to reply to a specific message to provide more context.
5.  **Emoji Reactions**: React to messages with emojis.

### Sprint 5: Internet Connectivity
*Goal: Enable communication across the internet.*

1.  **STUN Client**: Implement a STUN client to discover public IP addresses.
2.  **TURN Support**: Add support for using a TURN relay server as a fallback for peers behind restrictive NATs.
3.  **NAT Traversal Logic**: Implement hole-punching techniques to establish direct connections between peers on different networks.

### Sprint 6+: Innovation
*Goal: Differentiate the application from its competitors.*

1.  **Zero-Knowledge File Sync**: A peer-to-peer, Dropbox-like feature for synchronizing files between devices.
2.  **P2P Video/Audio Calls**: Implement secure, direct video and audio calls using WebRTC.
3.  **Collaborative Editing**: Allow users to collaborate on documents in real-time without a central server.

## Future Considerations

-   **Group Admin Features**: Roles, permissions, and invite links for group chats.
-   **Mobile Apps**: Native applications for Android and iOS, sharing a common Rust core with the desktop application.
-   **Themes & Personalization**: Light/dark modes, custom colors, and chat backgrounds.
-   **Blockchain-Based Identity**: A decentralized username system (e.g., ENS) to replace manual IP address and fingerprint exchange.
