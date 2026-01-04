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

## 📊 Current Status (v1.5.0)

### ✅ What Works

- **Core Security**: End-to-end encryption with Forward Secrecy (X25519 ECDH).
- **Messaging**: All core messaging features including text, file transfer, and real-time indicators.
- **UI/UX**: Modern, polished interface with toasts, themes, and improved layout.
- **Persistence**: Encrypted-at-rest chat history and a persistent identity system.
- **Trust & Discovery**: Trust-on-First-Use (TOFU) and mDNS Local Peer Discovery.
- **Robustness**: Hardened codebase with robust error handling.

### ⚠️ Known Limitations & Areas for Improvement

1. **Manual IP Exchange**: (Solved by mDNS for local network, still relevant for WAN).
2. **DoS Vulnerability**: No protection against simple DoS attacks via connection flooding.

---

## 🚀 High-Level Roadmap

The immediate focus is on solidifying the application's security foundation and dramatically improving the core user experience of connecting with peers.

- **v1.6: The Trust & Usability Release**: Implement a "Trust on First Use" model and automatic peer discovery to make the app both more secure and vastly easier to use.
- **v1.7: The Hardening Release**: Focus on stability and resilience by refactoring all error handling and adding protection against simple network attacks.
- **v2.0: The Ecosystem Release**: Expand capabilities with features like a command palette, session key rotation, and preparations for internet connectivity (NAT traversal).

---

## 🎯 Detailed Feature Roadmap

The roadmap is organized into prioritized sprints, focusing on delivering the most impactful changes first.

### 🔥 Sprint 1: Foundational Security & Trust (Completed)

*Goal: Make the app secure by default and simplify the trust process.*

1. **Trust on First Use (TOFU)**: ✅ **Completed**
    - Automatic fingerprint saving on first connection.
    - Blocking warning on fingerprint mismatch.

### 🏃 Sprint 2: Core User Experience (Completed)

*Goal: Eliminate the biggest friction point: manual IP address exchange.*

1. **Local Peer Discovery (mDNS/Bonjour)**: ✅ **Completed**
    - Automatically discovers peers on the local network.
    - "Nearby Users" list in Connect dialog.

### 🏃 Sprint 3: Application Hardening (In Progress)

*Goal: Make the application resilient and stable.*

1. **Error Handling Refactor**: ✅ **Completed**
    - Removed unsafe `.unwrap()` calls from critical paths.

2. **Connection Rate Limiting**:
    - **Task**: Implement a mechanism to limit the number of incoming connection attempts from a single IP address.
    - **Why**: Provides basic protection against simple Denial of Service (DoS) attacks.

### 🏃 Sprint 4: Power-User Features & Long-Term Security (Future)

*Goal: Add quality-of-life features and long-term cryptographic hygiene.*

1. **"Quick Switcher" Command Palette (Ctrl+K)**:
    - **Task**: Implement a floating search bar to instantly find and switch between chats and contacts.
    - **Why**: A modern power-user feature that dramatically speeds up navigation.

2. **Session Key Rotation**:
    - **Task**: Automatically re-negotiate the AES session key periodically.
    - **Why**: Improves long-term security by limiting the amount of data exposed if a single session key is ever compromised.

---

## 🔮 Future Considerations

- **Group Admin Features**: Roles, permissions, and invite links.
- **Mobile Apps**: Native apps for Android and iOS with a shared Rust core.
- **Themes & Personalization**: Light/dark modes, custom colors, and chat backgrounds.
- **Blockchain-Based Identity**: A decentralized username system (e.g., ENS).
