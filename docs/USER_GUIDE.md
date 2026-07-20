# User Guide

This guide is the reference manual for installation, launch modes, everyday workflows, storage behavior, and troubleshooting.

If you want the fastest onboarding path instead of the full reference, start with [TUTORIAL.md](TUTORIAL.md).

## Installation

### Pre-built binaries

Download the latest platform asset from the [GitHub Releases](https://github.com/fibo3090-code/secure-p2p-chat/releases) page.

Classic egui app:

- Windows: `P2PEM-Classic_<version>_x64-setup.exe`
- macOS Intel: `P2PEM-Classic_<version>_macos-x64.dmg`
- macOS Apple Silicon: `P2PEM-Classic_<version>_macos-aarch64.dmg`
- Linux: `P2PEM-Classic_<version>_linux-x64.tar.gz`

New Tauri desktop app (P2PEM):

- Windows: `P2PEM_<version>_x64-setup.exe` (NSIS) or `P2PEM_<version>_x64_en-US.msi`
- macOS: `P2PEM_<version>_aarch64.dmg` or `P2PEM_<version>_x64.dmg`
- Linux: `P2PEM_<version>_amd64.deb`, `P2PEM_<version>_amd64.AppImage`, or `P2PEM-<version>-1.x86_64.rpm`

Releases older than 1.13 used different asset names (`Messenger-Setup-v*.exe`,
`messenger-*.dmg`, `messenger-linux-v*.tar.gz`) for the classic app.

### Build from source

```bash
git clone <repository-url>
cd secure-p2p-chat
cargo build --release
```

Binary location:

- Windows: `target\release\p2pem-classic.exe`
- Linux/macOS: `target/release/p2pem-classic`

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

### Tauri desktop app (preview)

A newer desktop app (Tauri 2 + React) is in development and will replace the egui
GUI. It is not yet published in releases, so run it from source:

```bash
cd desktop && npx tauri dev
```

It drives the same core as the GUI/TUI (same identity, history, and P2P/Party
behavior) but uses its own data directory, so it appears as a distinct peer/identity
from the egui app on the same machine.

### Relay server

```bash
cargo run --release -- --relay-server --port 23456
```

When both peers support it, the relay only plays matchmaker: it gives each
side the other's addresses and the peers **hole punch** a direct TCP
connection, so chat traffic never crosses the relay. If punching fails
(strict/symmetric NAT, CGNAT), the relay transparently falls back to
forwarding the encrypted traffic. Set `P2PEM_NO_HOLEPUNCH=1` to always use
the forwarding path.

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
- signed invites expire 30 days after they were generated; an expired invite
  is rejected at import with a clear error - ask the sender for a fresh one
- legacy unsigned invites may still import for compatibility
- invalid addresses embedded in invites are dropped during import rather than trusted blindly

Use invite links when:

- you want fewer manual fields to copy
- you want the connector to avoid typing the address and key material by hand
- you want relay route information bundled into the invite

### Reach hosts across the internet with UPnP

If your router supports UPnP (or NAT-PMP), the app can ask it to forward your
listening port while you host, and your invite will carry the external address
— no manual port-forwarding.

- Enable it in Settings ("UPnP port mapping") or in the TUI with `:set upnp on`.
- Invites carry **both** addresses when available — the internet-reachable one
  first, your LAN one second — and the connecting side tries them in order.
  One invite therefore works for a friend across the internet *and* for a
  laptop on your own network.
- It is **off by default**: turning it on opens a port on your router and
  embeds your public IP (alongside your LAN IP) in the invites you share.
- Two protocols are tried automatically: UPnP/IGD first, then NAT-PMP (common
  on Apple and newer routers).
- It is best-effort: no gateway, the service disabled, or carrier-grade NAT all
  make it fail — you get a warning toast and LAN/relay keep working. If your
  router reports a *private* external IP (your ISP is double-NATing you), the
  app detects it and tells you to use a relay rather than handing out an
  unreachable address.
- The mapping uses a 1-hour lease that the app renews automatically while you
  host, and it removes the mapping when you stop hosting (the lease also
  guarantees the router reclaims the port even after an unclean exit).

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

### Host a community (Party server)

Communities are self-hosted, multi-user rooms with channels, direct messages,
file sharing, and durable history — members who were offline catch up when they
reconnect. One person runs the server; friends join with just an address, an
optional password, and a username.

Start a server:

```bash
cargo run --release -p messenger-server -- --name "Game Night" --port 12345
```

Options (also settable via `PARTY_NAME`, `PARTY_PORT`, `PARTY_PASSWORD`,
`PARTY_DATA_DIR` environment variables):

- `--name <NAME>` — the community name everyone sees (default: "Encrypted Messenger Party")
- `--port <PORT>` — TCP port to listen on (default: 12345)
- `--password <PASSWORD>` — require a password to join (omit for an open server)
- `--data-dir <DIR>` — where the database, file store, and server identity live (default: `party-data`)

On startup the server prints its **fingerprint**. Share your address
(`your-ip:port`) and that fingerprint with the people you invite; their app pins
the fingerprint on first join and warns them if it ever changes.

Joining from the app:

- **Desktop app**: Communities tab → enter the address, a username, and the
  password if one is set. Rejoining later is one click (communities are
  remembered).
- **TUI**: `:party-connect <host[:port]> <username> [password]`
- **GUI (egui)**: the Party window → join form.

Operator notes:

- state persists under the data dir (`party.db` + `blobs/`); back it up to keep
  history
- the server can read message contents (this is the "Administered" trust tier —
  it is what enables offline delivery and history); tell your members
- members are remembered by identity fingerprint, so someone who reconnects
  keeps their username and history

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

The TUI is fully usable without the mouse, and every action also has a typed
command — so you can drive the whole app from the keyboard (or script it).

### Keys

- `:` — enter command mode (when the message box is empty or unfocused)
- `Enter` — send the message / run the command
- `Ctrl+J` — newline inside the message
- `Tab` — cycle focus: chat list → messages → input
- `↑` / `↓` — select a chat, scroll messages, or recall command history,
  depending on focus
- `PgUp` / `PgDn` — scroll the message view
- `Ctrl+L` — copy the event log to the clipboard
- `Esc` — close an overlay / leave command mode / focus the chat list
- `q` — quit (asks to confirm) when the input is not focused
- `?` — open the help overlay
- The message view auto-scrolls to the newest message; scroll up to read
  back, and it re-sticks to the bottom when you return there.

Chat-list markers: `H` hosting · `●` connected · `○` offline · `*` unread.

### Command autocomplete

In command mode, start typing a command name and a popup of matching commands
appears above the input:

- `↑` / `↓` — move through the matches
- `Tab` — complete to the highlighted command
- `Enter` — run it · `Esc` — cancel

On an empty `:` prompt the menu is hidden, so `↑`/`↓` recall previous commands.

### Overlays

Some commands open a focused, keyboard-driven panel:

- **Fingerprint verification** — appears automatically on a new connection.
  Compare the safety grid / 64-char fingerprint with your peer out of band,
  then press `y` to accept or `n` to reject (or `:verify accept|reject`).
- **Password** — on startup, unlock an encrypted identity or set a password on
  a new one. Reachable later via `:unlock` / `:set-password`.
- **Contacts** (`:contacts`) — `↑`/`↓` to pick, `Enter` to connect.
- **Settings** (`:settings`) — `Enter` toggles the selected option.
- **Identity** (`:identity`), **Transfers** (`:transfers`), **Help** (`:help`),
  and the invite link panel are read-only; press `Esc` to close.

### Commands

```text
Connections
  :host [port]                              listen for an incoming peer
  :connect <host[:port]>                    connect to a hosting peer
  :host-relay <relay[:port]> [token]        host via a relay (copies an invite)
  :connect-relay <relay[:port]> <token>     connect through a relay
  :disconnect                               remove / disconnect the selected chat
  :stop-host                                stop listening

Contacts & invites
  :contacts                                 open the contacts list
  :contact-add <name> <host:port> [fp]      save a contact
  :contact-connect <name|#>                 connect to a saved contact
  :contact-remove <name|#>                  delete a contact
  :contact-rename <name|#> <new name>       rename a contact
  :invite [host:port]                       generate a signed invite link
  :invite-relay <relay[:port]>              host via relay + generate an invite
  :import <invite-link>                     import an invite as a contact

Messaging & files
  :send <path>                              send a file to the selected chat
  :transfers                                show active file transfers
  :rename <title>                           rename the selected chat
  :delete                                   delete the selected chat
  :clear-history                            erase all chats and contacts

Party servers
  :party-connect <host[:port]> <username> [password]
                                             join a Party server
  :party-post <message>                     post to the current Party channel
  :party-dm <username|#> <message>          direct-message a Party member
  :party-create-channel <name>              create a Party channel
  :party-status                             show joined Party servers

Identity & security
  :identity                                 show your fingerprint + safety grid
  :verify <accept|reject>                   answer a pending fingerprint check
  :unlock [password]                        unlock a password-protected identity
  :set-password <password>                  set or change your password

Settings & app
  :settings                                 open the settings panel
  :set <key> <value>                        change a setting (see below)
  :diagnostics                              export a diagnostics bundle
  :logs                                     copy the event log to the clipboard
  :help [command]                           show help
  :quit                                     save and exit (alias :q, :q! to skip confirm)
```

`:set` keys: `download-dir`, `listen-port`, `notifications`, `typing`,
`auto-accept`, `auto-host`, `mdns`, `theme` (`light|dark|midnight|forest|rose`).
Booleans accept `on`/`off`. Example: `:set notifications off`.

History is saved automatically (encrypted) on a timer and on exit.

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
- diagnostics and crash logs under the app data area

### Resetting local state

- `Settings -> Delete Everything` removes the messenger app data directory, including the encrypted identity, history, diagnostics, and password-protected local state.
- On Windows, the uninstaller also offers the same full-data cleanup so a reinstall starts from a fresh identity.

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
