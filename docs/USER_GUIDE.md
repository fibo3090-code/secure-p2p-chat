# User Guide

This guide is the reference manual for installation, launch modes, everyday workflows, storage behavior, and troubleshooting.

If you want the fastest onboarding path instead of the full reference, start with [TUTORIAL.md](TUTORIAL.md).

## Installation

### Pre-built binaries

Download the latest platform asset from the [GitHub Releases](https://github.com/fibo3090-code/secure-p2p-chat/releases) page.

- Windows: `Messenger-Setup-v*.exe`
- macOS Intel: `messenger-x86_64-apple-darwin.dmg`
- macOS Apple Silicon: `messenger-aarch64-apple-darwin.dmg`
- Linux: `messenger-linux-v*.tar.gz`

### Build from source

```bash
git clone <repository-url>
cd secure-p2p-chat
cargo build --release
```

Binary location:

- Windows: `target\release\encodeur_rsa_rust.exe`
- Linux/macOS: `target/release/encodeur_rsa_rust`

### Linux build prerequisites

The GUI build may require system libraries such as:

```bash
sudo apt-get install -y libgtk-3-dev libwayland-dev libxkbcommon-dev
```

## Launch Modes

### GUI

```bash
cargo run --release
```

### TUI

```bash
cargo run --release -- --tui
```

### Relay server

```bash
cargo run --release -- --relay-server --port 23456
```

### Useful CLI flags

- `--host`
- `--connect <host[:port]>`
- `--relay-server`
- `--host-relay <relay[:port]>`
- `--connect-relay <relay[:port]>`
- `--relay-token <token>`
- `--port <port>`

Examples:

```bash
cargo run --release -- --tui --host --port 9000
cargo run --release -- --tui --connect 127.0.0.1:12345
cargo run --release -- --relay-server --port 23456
cargo run --release -- --tui --host-relay relay.example.com:23456
cargo run --release -- --tui --connect-relay relay.example.com:23456 --relay-token mytoken
```

## First Run

On first launch the app:

- creates a local identity
- asks you to set a password
- stores the identity and history in the app data directory

Important limits:

- the password is required to unlock the identity later
- removing password protection is not supported
- if the password is lost, the identity cannot be recovered

## Everyday Workflows

### Start a direct chat on the same network

Use this when both devices can reach each other directly.

1. Host starts listening on a port.
2. Connector enters the host address and port.
3. Both sides verify fingerprints.
4. Chat normally.

GUI:

- use `+` to host or connect

TUI:

```text
:host 12345
:connect 192.168.1.40:12345
```

### Use invite links

Invite links are the easiest way to hand a contact your identity details.

Current behavior:

- the app emits signed invite links
- legacy unsigned invites may still import for compatibility
- invalid addresses embedded in invites are dropped during import rather than trusted blindly

Use invite links when:

- you want fewer manual fields to copy
- you want the connector to avoid typing the address and key material by hand
- you want relay route information bundled into the invite

### Use a self-hosted relay

Use this when direct TCP is inconvenient across NAT or the public internet.

Flow:

1. Start a relay server on a reachable machine.
2. Host starts a session via that relay.
3. Host shares the generated invite or relay token.
4. Connector joins through the same relay.

TUI:

```text
:host-relay relay.example.com:23456
:connect-relay relay.example.com:23456 <token>
```

Relay notes:

- the relay is self-hosted, not managed by the app project
- it forwards already encrypted session traffic
- it improves reachability, not anonymity

### Verify fingerprints

The app uses TOFU. First contact is only meaningful if you verify the fingerprint out of band.

Good verification channels:

- phone call
- video call
- in person

If the fingerprint changes unexpectedly:

- stop
- do not click through casually
- verify again before trusting the peer

### Send messages

Supported messaging behavior:

- direct encrypted text messages
- typing indicators
- large text chunking at the transport layer

The receiving peer still sees one logical message even when the sender’s transport split it into chunks.

### Send files

File transfers support files up to `10 GiB`.

Rules:

- outgoing files must exist locally
- cloud-only placeholders must be downloaded first
- received files are stored in the configured download directory

If you see transfer problems:

- check disk space
- check read permission on the source path
- check write permission on the destination directory

## GUI Reference

### Main areas

- Sidebar: select chats and open connection actions
- Chat view: read history, send messages, transfer files
- Contacts and invite flows: add peers manually or by invite link
- Settings: preferences, auto-host, diagnostics export, log terminal
- Help: in-app FAQ and troubleshooting

### Useful GUI actions

- create or paste invite links
- export diagnostics
- change download directory
- enable auto-host on startup
- enable auto-connect to known contacts
- toggle the log terminal

## TUI Reference

### Main controls

- `:` enters command mode
- `Enter` sends a message
- `Ctrl+J` inserts a newline
- `Tab` cycles focus
- `Esc` returns focus to the chat list
- `q` quits when the input is not focused

### Commands

```text
:host [port]
:connect <host[:port]>
:host-relay <relay[:port]> [token]
:connect-relay <relay[:port]> <token>
:disconnect
:diagnostics
:rename <title>
:help
:quit
```

### When to prefer the TUI

Use the TUI when you want:

- keyboard-first operation
- a lightweight terminal session
- quick relay or diagnostics commands
- remote shell usage on a server or dev box

## Storage and Diagnostics

### What is stored locally

- encrypted identity material
- encrypted chat history
- contact trust state
- configuration

### Where diagnostics go

Diagnostics bundles are written under the app data area. Export them before reporting bugs.

Do not share:

- private keys
- raw identity secrets
- passwords

### Crash and support data

The app also supports panic/crash logging to help support and debugging.

## Troubleshooting

### Cannot connect

- verify the host is actually listening
- verify the address and port
- check local firewall rules
- confirm both peers can route to each other
- for WAN use, prefer a self-hosted relay or a VPN/overlay network if direct TCP is difficult

### Messages are not delivering

- check the connection status
- reconnect manually if needed
- inspect the log terminal or TUI diagnostics output
- if one side is much older, large-message chunking compatibility may be involved

### Fingerprint changed

- treat it as a real security event
- do not accept it automatically
- verify the new fingerprint through a trusted external channel

### History does not load

- verify you used the correct password
- confirm the encrypted history file still exists
- inspect diagnostics and crash logs if the app terminated unexpectedly

### TUI looks wrong

- use a modern terminal
- ensure a monospace font is active
- enlarge the terminal window

### File transfer fails

- confirm the source file is fully local
- confirm the download directory is writable
- retry with a smaller file to separate permission issues from long-transfer issues

## Platform Notes

### Windows

- published installer releases are available
- Bonjour may be required for mDNS discovery support

### Linux

- published tarball releases are available
- GUI builds may require GTK and Wayland/XKB packages
- `avahi-daemon` may be required for mDNS discovery workflows

### macOS

- separate Intel and Apple Silicon DMGs are published
- Bonjour is built in
- Gatekeeper and local macOS trust prompts may still apply depending on how the app is distributed and opened
