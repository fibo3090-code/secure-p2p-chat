# Changelog

All notable changes to this project will be documented in this file.

## [1.3.0] - 2025-11-12

### 🐛 Bug Fixes

- **Fixed Chat Creation Synchronization Issue**: When creating a new chat from the contacts list, the chat was created locally but not propagated to the peer instance, causing "all recipients offline" errors when sending messages.
  - Added `SessionEvent::NewConnection` to properly notify the receiving peer about new incoming connections
  - Enhanced handshake to exchange chat IDs between client and host
  - Modified UI flow to create local chat immediately for responsiveness, then connect in background
  - Updated `connect_to_host()` and `connect_to_contact()` to accept optional `existing_chat_id` parameter

### 🔧 Technical Changes

- Modified `src/network/session.rs`: Client now sends chat_id to host during handshake (step 7)
- Enhanced `src/app/chat_manager.rs`: Added handler for `SessionEvent::NewConnection` to create chats on incoming connections
- Improved `src/gui/dialogs.rs`: "Open chat" button now creates chat locally, then connects asynchronously
- Updated `src/types.rs`: Added `NewConnection` variant to `SessionEvent` enum

### ✅ Improvements

- Chats now sync immediately across both peer instances
- Messages are reliably routed to correct sessions
- Better user experience with instant UI feedback during chat creation
- Backward compatible with existing connection methods

## [1.2.0] - 2025-10-31

### ✨ New Features & Enhancements

- **🎨 Emoji Picker**: Quick access to 32 common emojis with a dedicated picker button.
- **📁 Drag & Drop File Transfer**: Drag files directly into the chat window to send them.
- **🔔 Desktop Notifications**: Get notified when new messages arrive (configurable).
- **✍️ Typing Indicators**: See when your peer is typing in real-time.
- **💾 Auto-Save**: Conversations automatically saved every 30 seconds.
- **🗑️ Delete Chat**: Right-click or button to delete individual conversations.
- **⌨️ Keyboard Shortcuts**: `Ctrl+Enter` to send, `Escape` to clear input.
- **🔌 Connection Status**: Visual indicators for connected/disconnected state.

### 🎨 UI/UX Improvements

- Improved chat header with dynamic status display.
- Better visual feedback for typing state.
- Enhanced Settings panel with new toggles for notifications and typing indicators.
- Clickable chat rows for better usability.
- Delete confirmation dialog to prevent accidental deletion.
- Toast notifications for all errors.

### 🔧 Technical Changes

- Added `notify-rust` for desktop notifications.
- Added `emojis` for emoji support.
- Extended protocol with `TypingStart` and `TypingStop` message types.
- Updated `Config` struct with `enable_notifications` and `enable_typing_indicators` fields.

## [1.1.0] - 2025-10-31

### 🔐 Major Security Enhancement: Forward Secrecy

- **Critical Security Improvement**: Implemented forward secrecy using X25519 Elliptic Curve Diffie-Hellman (ECDH).
- **Past messages are now secure** even if long-term RSA keys are compromised.
- **Ephemeral keys** are generated for each session and discarded after use.
- **HKDF-SHA256** is used for key derivation from the shared secret.
- **Protocol version 2** is introduced with version negotiation to prevent downgrade attacks.

### 🔧 Technical Changes

- Added `x25519-dalek` and `hkdf` dependencies.
- Extended the protocol with `Version` and `EphemeralKey` messages.
- Updated the handshake sequence to include ephemeral key exchange.

### ⚠️ Breaking Changes

```

- Protocol v2 is incompatible with v1. Both parties must upgrade to communicate.

## [1.0.2] - 2025-10-23

### 🐛 Critical Bug Fix: Messages Not Being Received

- **Issue**: Messages were sent successfully but never appeared in the receiver's chat.
- **Root Cause**: Session events were being logged but never processed by the `ChatManager`.
- **Fix**: Implemented a proper event polling and processing system in the UI update loop.

### ✨ Improvements

- **Enhanced Logging**: Added comprehensive trace/debug logging throughout the network layer.
- **Event Processing**: All session events (`Listening`, `Connected`, `MessageReceived`, etc.) are now properly handled.

## [1.0.0] - 2025-10-23

### 🎉 Major Release - Complete UI/UX Overhaul

This release transformed the application from a functional prototype into a polished, user-friendly messaging app.

### ✨ Added Features

- **Welcome Screen**: Onboarding guide for new users.
- **Settings Panel**: Configure download folder, file size limits, etc.
- **Enhanced Chat Experience**: Multiline text input, colorful avatars, smart timestamps, and visual feedback.
- **User Experience**: Smart send button, hover tooltips, and consistent layout.

### 🔧 Improvements

- Consolidated and improved documentation.
- Fixed various borrow checker issues and warnings.

## [0.9.0] - Previous Version

- Basic chat functionality.
- End-to-end encryption (RSA + AES-GCM).
- File transfer support.
- Simple GUI interface.
- Message history persistence.