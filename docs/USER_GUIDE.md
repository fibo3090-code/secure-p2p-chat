# User Guide

This guide covers installation, launching, core workflows, and troubleshooting for both GUI and TUI use.

## Installation

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

### Send files

- choose a file from the GUI or send through the active session
- files larger than `10 GiB` are rejected
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
- for internet use, remember there is no built-in NAT traversal or relay support

### Messages not delivering

- check connection status
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
