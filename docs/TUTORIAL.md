# Tutorial

This tutorial walks through a real first session with P2PEM. It assumes two people, two devices, and a goal of getting from install to verified secure chat with the least confusion.

## Goal

By the end of this tutorial you will have:

- installed the app
- created an identity, protected it, and backed it up
- connected to another person
- verified the connection with a short code
- exchanged messages
- shared a file
- exported diagnostics if something goes wrong

## 1. Install the app

Download **P2PEM Desktop** from the [latest release](https://github.com/fibo3090-code/secure-p2p-chat/releases/latest):

- Windows: `P2PEM_<version>_x64-setup.exe`
- macOS Apple silicon: `P2PEM_<version>_aarch64.dmg`
- macOS Intel: `P2PEM_<version>_x64.dmg`
- Linux: `p2pem_<version>_amd64.AppImage` or `.deb`

The `P2PEM-Tools_*` archive on the same page is for running a server or working
from a terminal — you don't need it here.

If you prefer to build from source:

```bash
git clone <repository-url>
cd secure-p2p-chat/desktop
npm ci && npx tauri build
```

## 2. Launch and create your identity

Open the app. It creates a local identity and asks for a password, which
encrypts the private key and your history on disk. Use at least 12 characters —
that password is the only thing protecting your identity if someone gets the
disk.

**Then take the backup it offers.** This is the one step people skip and regret:

- there is no password reset and no account recovery
- if you lose the identity file without a backup, you lose that identity, and
  every contact who verified you has to verify a new one

You can redo it any time from Settings → Your identity → Identity backup, which
also tells you whether you have ever made one.

Prefer the terminal? `cargo run --release -p p2pem-classic` opens the TUI, which
does everything the desktop app does.

## 3. Pick a connection method

You have three practical ways to connect:

### Option A: Direct host/connect on the same LAN

Recommended for a first test on the same Wi-Fi or home network.

1. One person starts hosting.
2. The other person connects to that host’s address and port.

Desktop app:

1. Host clicks **New connection** and chooses host mode.
2. Connector clicks **New connection** and chooses connect mode.
3. Connector enters the host address, for example `192.168.1.40:12345`.

TUI path:

```text
:host 12345
:connect 192.168.1.40:12345
```

### Option B: Invite link

Recommended when you want to avoid manually copying the fingerprint, public key, and address fields.

1. Host generates an invite link from Contacts.
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

When both peers support it, the relay first coordinates a direct TCP hole
punch, so after pairing your traffic flows peer-to-peer and the relay drops
out of the path. If the punch fails, the relay forwards the already encrypted
session traffic instead. Either way it never terminates chat encryption for
you (set `P2PEM_NO_HOLEPUNCH=1` to always use forwarding).

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

Desktop app:

- type in the message box
- press `Enter` or click the send action
- a ✓ next to your message means the peer's device confirmed receipt

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
list and key reference. The authoritative command reference lives in
[USER_GUIDE.md](USER_GUIDE.md#tui-reference); this tutorial deliberately does
not repeat it.

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

Desktop app:

- open **Settings** → **Support**
- use **Export diagnostics**

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

Prefer working in a terminal? The TUI is fully featured and drives the same core
— see [USER_GUIDE.md](USER_GUIDE.md#tui-reference).

