# Tutorial

This tutorial walks through a real first session with Encrypted P2P Messenger. It assumes two people, two devices, and a goal of getting from install to verified secure chat with the least confusion.

## Goal

By the end of this tutorial you will have:

- installed the app
- created and protected an identity
- connected to another person
- verified fingerprints
- exchanged messages
- shared a file
- exported diagnostics if something goes wrong

## 1. Install the app

Use the latest release from GitHub:

- Windows: download `Messenger-Setup-v*.exe`
- macOS Intel: download `messenger-x86_64-apple-darwin.dmg`
- macOS Apple Silicon: download `messenger-aarch64-apple-darwin.dmg`
- Linux: download `messenger-linux-v*.tar.gz`

If you prefer to build from source:

```bash
git clone <repository-url>
cd secure-p2p-chat
cargo build --release
```

## 2. Launch and create your identity

Start the GUI:

```bash
cargo run --release
```

Or launch the TUI:

```bash
cargo run --release -- --tui
```

On first launch the app creates a local identity. Set a password immediately. That password protects the private key and encrypted history on disk.

Important:

- there is no password reset flow
- removing password protection is not supported
- if you lose the password, you lose access to that identity

## 3. Pick a connection method

You have three practical ways to connect:

### Option A: Direct host/connect on the same LAN

Recommended for a first test on the same Wi-Fi or home network.

1. One person starts hosting.
2. The other person connects to that host’s address and port.

GUI path:

1. Host clicks `+` and chooses host mode.
2. Connector clicks `+` and chooses connect mode.
3. Connector enters the host address, for example `192.168.1.40:12345`.

TUI path:

```text
:host 12345
:connect 192.168.1.40:12345
```

### Option B: Invite link

Recommended when you want to avoid manually copying the fingerprint, public key, and address fields.

1. Host generates an invite link from the GUI.
2. Host sends that link through some other channel.
3. Connector pastes it into the Invite Link flow.

Current UI flows generate signed invite links. Legacy unsigned invites may still import for compatibility, but they should not be preferred.

### Option C: Self-hosted relay

Use this when direct TCP is inconvenient across the internet or through NAT.

1. Start a relay server on a reachable machine:

```bash
cargo run --release -- --relay-server --port 23456
```

2. Host a chat through the relay.
3. Share the generated relay invite or token with the other peer.
4. The other peer connects through the same relay.

TUI examples:

```text
:host-relay relay.example.com:23456
:connect-relay relay.example.com:23456 <token>
```

The relay forwards already encrypted session traffic. It does not terminate chat encryption for you.

## 4. Verify fingerprints before trusting the connection

This is the most important manual security step.

Each peer has a fingerprint. Verify that fingerprint over a separate trusted channel:

- phone call
- video call
- in person

Do not skip this just because the app connected successfully. The app uses TOFU, so first contact only becomes meaningful if you verify the other side out of band.

In the TUI a verification overlay opens automatically on a new connection: compare the safety grid / fingerprint, then press `y` to accept or `n` to reject (equivalently `:verify accept` / `:verify reject`).

If the fingerprint changes unexpectedly later:

- stop
- treat it as a security event
- verify again before continuing

## 5. Exchange messages

After the connection is up and fingerprints are verified:

- select the chat
- type a message
- send it

Large text messages are chunked automatically by the transport and reassembled on the receiving side.

GUI:

- type in the message box
- press `Enter` or click the send action

TUI:

- `Enter` sends
- `Ctrl+J` inserts a newline
- `Tab` cycles focus
- the view auto-scrolls to the newest message

## 6. Share a file

You can transfer files up to `10 GiB`.

Rules to remember:

- the source file must be a real local file
- cloud-only placeholders from OneDrive, iCloud, or Dropbox must be downloaded first
- received files are written into the configured download directory

If a transfer fails:

- check free disk space
- check read permission on the source path
- check write permission on the download directory

In the TUI, send a file with `:send <path>` and watch progress with `:transfers`.

## 7. Learn the TUI command set

On first launch the TUI prompts you to set or unlock your identity password.
After that, press `:` and start typing — an autocomplete menu lists matching
commands (`Tab` completes, `↑`/`↓` choose). Press `:help` any time for the full
list and key reference. Common commands:

```text
:host [port]
:connect <host[:port]>
:host-relay <relay[:port]> [token]
:connect-relay <relay[:port]> <token>
:contacts                 :contact-add <name> <host:port> [fp]
:invite                   :import <invite-link>
:send <path>              :transfers
:identity                 :settings        :set <key> <value>
:party-connect <host[:port]> <username>    :party-create-channel <name>
:disconnect   :rename <title>   :diagnostics   :help   :quit
```

Recommended first TUI session:

```text
:host 12345
```

On the second machine:

```text
:connect 192.168.1.40:12345
```

Then accept the fingerprint overlay (`y`) on both sides and start chatting.

## 8. Export diagnostics when something breaks

If the app misbehaves, export diagnostics before reporting the bug.

GUI:

- open `Settings`
- use the diagnostics export action

TUI:

```text
:diagnostics
```

The diagnostics bundle is the best starting point for support. Do not share private keys.

## 9. Next steps

Once the first session works, move on to:

- [USER_GUIDE.md](USER_GUIDE.md) for the full reference
- [../SECURITY.md](../SECURITY.md) for the app’s actual security posture
- [../THREAT_MODEL.md](../THREAT_MODEL.md) for assumptions and limits

If you build from source, you can also try the in-development Tauri + React desktop
app (the future replacement for the egui GUI) with `cd desktop && npx tauri dev` —
see [USER_GUIDE.md](USER_GUIDE.md#tauri-desktop-app-preview).
