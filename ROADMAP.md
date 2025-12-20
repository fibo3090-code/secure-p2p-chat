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

### ⚠️ Known Limitations & Areas for Improvement

1. **Manual IP Exchange**: The largest point of friction is the need to manually share IP addresses.
2. **Trust Management**: The user must manually verify fingerprints on every new session, and the identity key is not password-protected by default.
3. **Robustness**: The application can still crash under certain error conditions due to remaining `.unwrap()` calls.
4. **DoS Vulnerability**: No protection against simple DoS attacks via connection flooding.

---

## 🚀 High-Level Roadmap

The immediate focus is on solidifying the application's security foundation and dramatically improving the core user experience of connecting with peers.

- **v1.6: The Trust & Usability Release**: Implement a "Trust on First Use" model and automatic peer discovery to make the app both more secure and vastly easier to use.
- **v1.7: The Hardening Release**: Focus on stability and resilience by refactoring all error handling and adding protection against simple network attacks.
- **v2.0: The Ecosystem Release**: Expand capabilities with features like a command palette, session key rotation, and preparations for internet connectivity (NAT traversal).

---

## 🎯 Detailed Feature Roadmap

The roadmap is organized into prioritized sprints, focusing on delivering the most impactful changes first.

### 🔥 Sprint 1: Foundational Security & Trust (Immediate Priority)

*Goal: Make the app secure by default and simplify the trust process.*

1. **Trust on First Use (TOFU) & Mandatory Identity Encryption**:
    - **Task**: On first launch, force the user to create a password for their identity. On subsequent launches, require the password to unlock the app.
    - **Task**: When connecting to a new peer, automatically save and trust their fingerprint. On future connections, raise a severe, blocking warning if the fingerprint changes.
    - **Why**: This drastically improves baseline security for all users and removes the user-fatigue of constant manual verification.

2. **Refine Fingerprint Verification UX**:
    - **Task**: Overhaul the fingerprint verification dialog to be clearer and more user-friendly, as outlined in the Design Roadmap.
    - **Why**: A better UX encourages users to engage with this critical security step when it's needed (e.g., on a TOFU warning).

### 🏃 Sprint 2: Core User Experience (High Priority)

*Goal: Eliminate the biggest friction point: manual IP address exchange.*

1. **Local Peer Discovery (mDNS/Bonjour)**:
    - **Task**: Integrate a library to automatically discover and display other users on the same local network.
    - **Why**: This is the single largest usability improvement. Users will be able to see and connect to peers with a single click, without ever needing to know what an IP address is.

2. **Enhance Empty States with Quick Actions**:
    - **Task**: (Already Implemented) The UI now shows helpful actions when no chat is open. This task will be to ensure it's fully polished.
    - **Why**: Guides new users on how to get started.

### 🏃 Sprint 3: Application Hardening (Medium Priority)

*Goal: Make the application resilient and stable.*

1. **Complete Error Handling Refactor**:
    - **Task**: Systematically eliminate all remaining `.unwrap()` and `.expect()` calls from the application logic.
    - **Why**: This will prevent the application from ever crashing due to unexpected data or network states, making it far more reliable.

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
