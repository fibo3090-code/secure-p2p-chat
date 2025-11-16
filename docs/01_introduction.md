# 1. Introduction to Encrypted P2P Messenger

[![Version](https://img.shields.io/badge/version-1.3.1-blue)](#)
[![License](https://img.shields.io/badge/license-MIT-orange)](#-license)
[![Security](https://img.shields.io/badge/security-audited-success)](#)
[![Rust](https://img.shields.io/badge/rust-1.70+-orange)](https://www.rust-lang.org/)

> **Secure, private, peer-to-peer messaging with end-to-end encryption and forward secrecy.**

Welcome to the Encrypted P2P Messenger project. This document provides an introduction to the application, its core principles, and its features.

## What is this?

Encrypted P2P Messenger is a **desktop application** for secure messaging built with these principles:

- **Privacy First**: No central server, no data collection, no tracking. Your conversations are your own.
- **End-to-End Encryption**: Using a combination of RSA and AES-256-GCM, messages are encrypted from the moment they are sent until they are received.
- **Forward Secrecy**: With the X25519 ECDH key exchange, past messages remain secure even if your long-term keys are compromised.
- **Peer-to-Peer**: The application facilitates direct connections between devices on your local network (LAN) or VPN, removing the need for a centralized server that could be a single point of failure or attack.
- **Open Source**: The entire codebase is open for anyone to inspect, audit, and contribute to. This transparency is a cornerstone of the project's security philosophy.

## Features

The application comes with a rich set of features designed to provide a secure and user-friendly messaging experience:

- **Secure Messaging**: The core of the application is its secure messaging capability, which uses state-of-the-art encryption to protect your conversations.
- **Peer Discovery Ready**: While currently requiring manual connection, the application is designed to support automatic peer discovery in the future using technologies like mDNS.
- **File Transfer**: Securely send and receive files of any size, with chunking and progress indicators.
- **Rich User Experience**:
    - **Typing Indicators**: See when the other person is typing.
    - **Desktop Notifications**: Get notified of new messages even when the application is in the background.
    - **Emoji Picker**: Easily add emojis to your messages.
    - **Drag & Drop**: Drag and drop files directly into the chat window to send them.
- **Invite Links & QR Codes**: Quickly and easily add new contacts by sharing an invite link or QR code.
- **Local Persistence**: All your chat history and your unique identity are stored locally on your computer, giving you full control over your data.
- **Auto-Host & Auto-Rehost**: The application can be configured to automatically start listening for connections on launch and to automatically re-listen after a connection is established, ensuring you are always available to your contacts.
