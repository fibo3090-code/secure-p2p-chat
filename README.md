# Encrypted P2P Messenger

[![Version](https://img.shields.io/badge/version-1.11.0-blue)](CHANGELOG.md)
[![License](https://img.shields.io/badge/license-MIT-orange)](LICENSE.md)
[![Security](https://img.shields.io/badge/security-medium-yellow)](SECURITY.md)
[![Rust](https://img.shields.io/badge/rust-1.86+-orange)](https://www.rust-lang.org/)

Secure peer-to-peer messaging for desktop, built with Rust. The app provides encrypted messaging, encrypted local storage, forward secrecy, signed invite links, and both GUI and TUI frontends.

## What It Does

- End-to-end encrypted messaging over direct peer connections
- X25519 + HKDF session establishment with RSA-PSS identity proofs
- Encrypted local identity and encrypted chat history at rest
- Large text messages are chunked automatically, plus file transfer, typing indicators, invite links, QR generation, and optional LAN discovery
- Diagnostics bundle export and on-disk panic/crash logs for support
- GUI built with `egui` and terminal interface built with `ratatui`

## Current Status

The project is functional and actively maintained, but it is not a “finished product” in every area.

- Security posture: medium. See [SECURITY.md](SECURITY.md).
- Internet connectivity: relay-assisted WAN connectivity is available for self-hosted deployments. Direct TCP is still the default.
- LAN discovery: optional and privacy-sensitive. Disabled by default.
- Packaging: Windows, Linux, and macOS release artifacts are published through GitHub Releases.

## Quick Start

### Build

```bash
cargo build --release
```

### Run GUI

```bash
cargo run --release
```

### Run TUI

```bash
cargo run --release -- --tui
```

The terminal UI is fully featured and keyboard-driven: press `:` and start typing for an autocomplete command menu, or `:help` for the full command and keybinding reference. Every action — connecting, fingerprint verification, contacts, invites, file transfer, settings — is reachable without leaving the terminal. See [docs/USER_GUIDE.md](docs/USER_GUIDE.md#tui-reference) for the command list.

### First Launch

On first launch, the app creates an identity and requires password protection before normal use. The unlock/set-password screen is blocking by design (a password overlay in the TUI, a blocking screen in the GUI).

### Trust Model

The app uses TOFU: trust on first use.

1. Connect to a peer.
2. Verify the displayed fingerprint over a separate trusted channel.
3. Accept only if it matches exactly.

## Platform Notes

| Platform | Status | Notes |
|---|---|---|
| Windows | Supported | Installer releases are published. Bonjour may be needed for mDNS discovery. The uninstaller can remove local identity/history data for a true reset. |
| Linux | Supported | Release tarballs are published; `avahi-daemon` and GUI system libraries may be required. |
| macOS | Supported | Intel and Apple Silicon DMGs are published; Native Bonjour works. |

## Documentation

Start with [docs/README.md](docs/README.md).

- [docs/TUTORIAL.md](docs/TUTORIAL.md): step-by-step first session tutorial for GUI, TUI, and relay-assisted setup
- [docs/USER_GUIDE.md](docs/USER_GUIDE.md): installation, usage, GUI/TUI flows, troubleshooting
- [DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md): contributor workflow, build/test/release process
- [docs/03_architecture.md](docs/03_architecture.md): codebase architecture and runtime responsibilities
- [docs/04_protocol.md](docs/04_protocol.md): wire protocol and handshake details
- [SECURITY.md](SECURITY.md): security posture, controls, open risks, disclosure
- [THREAT_MODEL.md](THREAT_MODEL.md): assumptions, assets, attack surfaces, limitations
- [docs/05_platform_spec.md](docs/05_platform_spec.md): platform plan, roadmap, and backlog
- [docs/AUDITS.md](docs/AUDITS.md): consolidated audit history and findings

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT. See [LICENSE.md](LICENSE.md).
