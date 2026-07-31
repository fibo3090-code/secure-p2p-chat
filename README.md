# P2PEM — Encrypted P2P Messenger

[![Version](https://img.shields.io/badge/version-1.14.0-blue)](CHANGELOG.md)
[![License](https://img.shields.io/badge/license-MIT-orange)](LICENSE.md)
[![Security](https://img.shields.io/badge/security-self--assessed-yellow)](SECURITY.md)
[![Rust](https://img.shields.io/badge/rust-1.86+-orange)](https://www.rust-lang.org/)

Messages that go straight from your device to your friend's, end-to-end
encrypted, with no account and no server holding your history. Built in Rust.

## Install

**[Download P2PEM Desktop](https://github.com/fibo3090-code/secure-p2p-chat/releases/latest)** — Windows, macOS, or Linux.

| System | File |
|---|---|
| Windows | `P2PEM_<version>_x64-setup.exe` |
| macOS (Apple silicon) | `P2PEM_<version>_aarch64.dmg` |
| macOS (Intel) | `P2PEM_<version>_x64.dmg` |
| Linux | `p2pem_<version>_amd64.AppImage` or `.deb` |

That is the app. There is exactly one — earlier releases also shipped a second,
older desktop GUI, which has been retired so nobody has to guess which to
install.

Running a headless box? `P2PEM-Tools_<version>_<os>` contains the terminal
client (which also runs a relay) and the community server. You do not need it
to use the desktop app.

## What you get

- **End-to-end encryption** on every conversation — X25519 key agreement with
  forward secrecy, AES-256-GCM, RSA-PSS identity proofs, automatic key rotation.
- **Verification you can actually do.** On first contact both sides see the same
  six digits and three emoji. Read them aloud over a call; if they match, nobody
  is in the middle. (The 64-character fingerprint and a colour grid are there
  too, one click away, for people who want them.)
- **Nothing stored anywhere but your device.** No account, no sign-up, no server
  copy of your messages. Identity and history are encrypted at rest with
  Argon2id + ChaCha20-Poly1305.
- **Direct connections**, with an optional self-hosted relay when NATs get in
  the way. The relay coordinates a hole punch first and only forwards bytes as a
  fallback — and it only ever sees ciphertext.
- **Files, large messages, delivery receipts, typing indicators**, and optional
  LAN peer discovery (off by default — it announces your presence).
- **Communities**: a self-hosted server for channels, DMs, and shared files when
  you want a group that works while people are offline.

## Honest limitations

Worth knowing before you switch to it:

- **No mobile client.** Desktop only.
- **Direct conversations have no offline delivery.** Both people have to be
  online at the same time. Only the community server buffers messages.
- **Reaching someone across the internet needs setup** — a port forward, UPnP,
  or someone running a relay. There is no infrastructure operated for you.
- **No third-party security audit.** The "medium" posture in
  [SECURITY.md](SECURITY.md) is a self-assessment.
- **Your password cannot be reset.** It is what decrypts your identity. The app
  offers a backup at first run; take it.

## First launch

The app creates an identity and asks for a password that encrypts it (minimum 12
characters — that password is the whole at-rest story). It then offers to save a
backup of your identity file, which is worth doing immediately: there is no
account recovery, so a lost disk without a backup means a lost identity.

Then connect: one side hosts (or opens a relay session) and shares an address,
invite link, or relay token; the other dials it. Compare the six-digit code, and
you're talking.

## Build from source

```bash
# Desktop app (needs Node 20+ and the Tauri prerequisites)
cd desktop && npm ci && npx tauri build

# Terminal client / relay
cargo build --release -p p2pem-classic

# Community server
cargo build --release -p messenger-server

# Everything, with tests
cargo test --workspace && (cd desktop && npm test)
```

The terminal UI is fully featured and keyboard-driven: press `:` for an
autocomplete command menu, or `:help` for the full reference. Every action —
connecting, verification, contacts, invites, file transfer, settings — is
reachable without leaving the terminal. See
[docs/USER_GUIDE.md](docs/USER_GUIDE.md#tui-reference).

## Platform notes

| Platform | Notes |
|---|---|
| Windows | Installer (`.exe`/`.msi`). Bonjour may be needed for mDNS discovery. |
| Linux | `.AppImage` (portable) and `.deb`. `avahi-daemon` for mDNS discovery. |
| macOS | Universal coverage via separate Intel and Apple-silicon DMGs. Native Bonjour works. |

## Documentation

Start with [docs/README.md](docs/README.md).

- [docs/TUTORIAL.md](docs/TUTORIAL.md): step-by-step first session
- [docs/USER_GUIDE.md](docs/USER_GUIDE.md): installation, usage, troubleshooting
- [DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md): contributor workflow, build/test/release
- [docs/architecture.md](docs/architecture.md): codebase architecture
- [docs/protocol.md](docs/protocol.md): wire protocol and handshake
- [SECURITY.md](SECURITY.md): posture, controls, open risks, disclosure
- [THREAT_MODEL.md](THREAT_MODEL.md): assumptions, assets, attack surfaces
- [docs/platform_spec.md](docs/platform_spec.md): platform plan and roadmap
- [docs/AUDITS.md](docs/AUDITS.md): audit history and findings
- [DESIGN_NOTES.md](DESIGN_NOTES.md): UI/UX principles, brand, theming
- [CHANGELOG.md](CHANGELOG.md): release history

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) and [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

## License

MIT. See [LICENSE.md](LICENSE.md).
