# 🗺️ Project Roadmap

> "Design is not just what it looks like and feels like. Design is how it works." - Steve Jobs

This document outlines the development roadmap for the Encrypted P2P Messenger, focusing on simplicity, user experience, and innovative features that make secure messaging accessible to everyone.

---

## 🎨 Design Philosophy

- **Simplicity**: Remove technical jargon from the UI and provide smart defaults.
- **User Experience First**: Every feature should have a clear "why" and "how". Delight users with polish.
- **Integration**: Features should feel like they belong together with a consistent design language.
- **Innovation**: Leverage the P2P nature of the app for unique features.

---

## 📊 Current Status (v1.3.1)

### ✅ What Works
- **Core Security**: RSA-2048-OAEP + AES-256-GCM encryption with X25519 ECDH Forward Secrecy.
- **Messaging**: Bidirectional text messaging, file transfer, typing indicators, and desktop notifications.
- **UI/UX**: Modern interface with avatars, timestamps, emoji picker, and drag & drop.
- **Persistence**: Reliable history storage and a persistent identity system.

### 🆕 Recent (v1.3.1)

- Auto-rehost now shows a success toast: "Host relancé" after a listener is restarted.
- Added a minimal guard to prevent multiple concurrent listeners on the same port, avoiding duplicate hosts during auto-rehost.
- Added unit test to validate placeholder-host detection used by the listener guard.

### ⚠️ Known Limitations
1.  **LAN-only**: No NAT traversal for WAN connectivity.
2.  **Manual Fingerprint Verification**: No automated certificate authority.
3.  **Some unit tests need updates**.
4.  **Some security vulnerabilities are still open**.

---

## 🚀 High-Level Roadmap

- **v1.3: The Usability Release**: Delivered reliability fixes for chat creation and session sync.
- **v2.0: The Professional Release**: Enterprise-ready features like moderation, NAT traversal, and message search.
- **v3.0: The Next Generation**: Advanced features like post-quantum cryptography, mobile apps, and voice/video calls.

---

## 🎯 Detailed Feature Roadmap

### 🔥 Sprint 1: Connection UX (Week 1-2)
*Goal: Make connection "just work".*

1.  **Smart Connection Discovery (mDNS/Bonjour)**: Auto-discover other users on the same network.
2.  **QR Code Connection**: Scan a QR code to connect to a peer.
3.  **Visual Fingerprint Verification**: Use colored grids or memorable words instead of hex strings.
4.  **Intelligent Error Messages**: Provide actionable advice when connections fail.
5.  **Connection History**: Easily reconnect to previous peers.

### 🏃 Sprint 2: Reliability (Week 3-4)
*Goal: The app never loses messages.*

1.  **Heartbeat/Keepalive System**: Detect and handle dropped connections.
2.  **Auto-Reconnection**: Automatically retry connections with exponential backoff.
3.  **Message Delivery Status (✓✓)**: Provide feedback on message state (Sent, Delivered, Read).
4.  **Offline Message Queue**: Queue messages sent while offline and send upon reconnection.
5.  **Session/Chat Sync Improvements**: Delivered in v1.3.0 (see above).

### 🏃 Sprint 3: Identity & Trust (Week 5-6)
*Goal: A robust and persistent identity system.*

1.  **Encrypted Keystore**: Protect the user's private key with a password (Argon2 + AES-256).
2.  **First-Time Setup Wizard**: Guide users through creating their identity.
3.  **Password Management UI**: Allow users to change their password.
4.  **Device Recognition (TOFU)**: Warn users if a contact's fingerprint changes.

### 🏃 Sprint 4: Messaging++ (Week 7-8)
*Goal: Rich messaging features.*

1.  **Message Search**: Full-text search within and across chats.
2.  **Rich Text Formatting (Markdown)**: Support for bold, italics, code blocks, etc.
3.  **File Transfer Polish**: Inline previews for images and progress visualization.
4.  **Reply to Specific Message**: Add context to replies.
5.  **Emoji Reactions**: React to messages with emojis.

### 🏃 Sprint 5: Internet Connectivity (Week 9-12)
*Goal: Work across the internet, not just LAN.*

1.  **STUN Client**: Discover public IP addresses.
2.  **TURN Support**: Use a relay server as a fallback for difficult NATs.
3.  **NAT Traversal Logic**: Implement hole-punching techniques.

### 🏃 Sprint 6+: Innovation (Month 4+)
*Goal: Differentiate from competitors.*

1.  **Zero-Knowledge File Sync**: A P2P Dropbox-like feature.
2.  **P2P Video/Audio Calls**: Using WebRTC for secure, direct calls.
3.  **Collaborative Editing**: Real-time, serverless document collaboration.

---

## 🔮 Future Considerations

- **Group Admin Features**: Roles, permissions, and invite links.
- **Mobile Apps**: Native apps for Android and iOS with a shared Rust core.
- **Themes & Personalization**: Light/dark modes, custom colors, and chat backgrounds.
- **Blockchain-Based Identity**: A decentralized username system (e.g., ENS).
