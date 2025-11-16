# 2. Getting Started

This guide will walk you through the process of setting up, building, and running the Encrypted P2P Messenger application.

## Prerequisites

Before you begin, ensure you have the following installed on your system:

- **Rust 1.70+**: The application is built with Rust. You can install it from [rustup.rs](https://rustup.rs/).
- **Network Access**: To communicate with other users, you must be on the same local area network (LAN) or connected to the same VPN.

## Build & Run

Follow these steps to get the application running on your machine:

1.  **Clone the Repository**:
    ```bash
    git clone <repository-url>
    cd chat-p2p
    ```

2.  **Build the Application**:
    For the best performance, it is recommended to build the application in release mode.
    ```bash
    cargo build --release
    ```

3.  **Run the Application**:
    Once the build is complete, you can run the application with the following command:
    ```bash
    cargo run --release
    ```

### Platform-Specific Instructions

#### Windows

-   **Recommended Shell**: It is recommended to use PowerShell or the Windows Terminal for the best experience.
-   **SmartScreen Warning**: When running a packaged binary for the first time, Windows SmartScreen may show a warning. If you trust the source of the application, you can bypass this by clicking "More info" and then "Run anyway".
-   **Packaging Script**: For developers, a PowerShell script is provided to build and package the application for distribution:
    ```powershell
    ./build-and-package.ps1
    ```

## Verifying Fingerprints (Critical for Security!)

The most critical step to ensure your communication is secure is to **verify the fingerprints** of your contacts. A fingerprint is a unique identifier for each user's public key.

**Why is this important?**
Verifying fingerprints prevents **Man-in-the-Middle (MITM)** attacks, where an attacker could impersonate one of your contacts and intercept your messages.

**How to Verify:**
When you connect to another user for the first time, the application will display their 64-character fingerprint. You must verify this fingerprint through a **separate, secure channel**.

-   **Good methods**: A phone call, a video call, or in-person verification.
-   **Bad methods**: Verifying over an unencrypted chat or email.

**What to do:**
1.  One person reads their fingerprint aloud while the other person checks it against the fingerprint displayed in the application.
2.  If the fingerprints match exactly, you can trust the connection.
3.  **If the fingerprints do not match, do not proceed.** Disconnect immediately and investigate the cause.

You should re-verify fingerprints whenever a contact's device changes or if you have any reason to be suspicious.
