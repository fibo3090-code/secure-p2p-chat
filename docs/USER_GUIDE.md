# User Guide

This guide covers installation, launching, core workflows, and troubleshooting for both GUI and TUI use.

## Installation

### Pre-built Binaries (Recommended)

The easiest way to get the app is to download the latest release for your platform from the [GitHub Releases](https://github.com/fibo3090/secure-p2p-chat/releases) page.

- **Windows**: Download `Messenger-Setup-v*.exe` and run the installer.
- **macOS**: Download `Messenger-macOS-*.dmg`, open it, and drag the app to your Applications folder.
- **Linux**: Download the `.tar.gz` bundle, extract it, and run the `messenger` binary.

### Build from source

```bash
git clone <repository-url>
cd secure-p2p-chat
cargo build --release
```

Binary location:

- Windows: `target\\release\\encodeur_rsa_rust.exe`
- Linux/macOS: `target/release/encodeur_rsa_rust`

## Launching

### GUI

```bash
cargo run --release
```

### TUI

```bash
cargo run --release -- --tui
```

### Helpful flags

- `--host`
- `--connect <host:port>`
- `--port <port>`

Examples:

```bash
cargo run --release -- --tui --host --port 9000
cargo run --release -- --tui --connect 127.0.0.1:12345
cargo run --release -- --relay-server --port 23456
cargo run --release -- --tui --host-relay 127.0.0.1:23456
```

## First Run

- The app creates an identity.
- You must set a password before normal use.
- If the identity is already encrypted, you must unlock it before using the app.

Removing password protection is not supported.

## Core Workflows

### Connect to someone

Option 1:

- one peer hosts
- one peer connects to the host address and port

Option 2:

- host generates an invite link
- other peer imports the invite

### Verify fingerprints

Always verify fingerprints over a separate trusted channel.

- phone call
- video call
- in person

Do not trust first contact blindly unless you explicitly accept the TOFU tradeoff.

### Share invite links

Current UI flows emit signed v2 invites.

Legacy unsigned invites may still import, but they should be treated as compatibility-only.

Relay-capable signed invites can also carry a self-hosted relay endpoint and one-time rendezvous token.

### Send files

- choose a file from the GUI or send through the active session
- large text messages are chunked automatically and still appear as one message on the peer side
- files larger than `10 GiB` are rejected
- outgoing files can come from any folder, but cloud-only placeholders must be downloaded locally first
- received files go to the configured download directory

## TUI Basics

### Main controls

- `:` enters command mode
- `Enter` sends a message
- `Ctrl+J` inserts a newline
- `Tab` cycles focus
- `q` quits when the input is not focused

### Common commands

- `:host 9000`
- `:connect 192.168.1.10:12345`
- `:disconnect`
- `:diagnostics`
- `:rename Team Chat`
- `:help`
- `:quit`

## Troubleshooting

### Cannot connect

- verify the host is actually listening
- verify the address and port
- check local firewall rules
- for internet use, you can either use direct TCP or a self-hosted relay server

### Messages not delivering

- check connection status
- if you are talking to an older build, very large messages may not be understood by that peer
- reconnect manually if needed
- inspect the log terminal for errors
- export a diagnostics bundle from Settings or with `:diagnostics` in the TUI when reporting a bug

### Fingerprint changed

- do not ignore it
- treat it as a security event until verified out of band

### History does not load

- verify you used the correct password
- confirm the encrypted history file still exists
- inspect the diagnostics/crash files in the app data directory if the app terminated unexpectedly

### TUI rendering looks wrong

- use a modern terminal with a monospace font
- enlarge the terminal window

## Platform Notes

### Windows

- best packaging support
- Bonjour may be needed for mDNS discovery

### Linux

- may require `avahi-daemon`
- GUI builds may need platform libraries such as `libgtk-3-dev`

### macOS

- Bonjour is built in
- packaging automation is lighter than on Windows
