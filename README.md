# Encrypted P2P Messenger

[![Version](https://img.shields.io/badge/version-1.7.7-blue)](CHANGELOG.md)
[![License](https://img.shields.io/badge/license-MIT-orange)](LICENSE.md)
[![Security](https://img.shields.io/badge/security-medium-yellow)](SECURITY.md)
[![Rust](https://img.shields.io/badge/rust-1.86+-orange)](https://www.rust-lang.org/)

Secure peer-to-peer messaging for desktop, built with Rust. The app provides encrypted messaging, encrypted local storage, forward secrecy, signed invite links, and both GUI and TUI frontends.

## What It Does

- End-to-end encrypted messaging over direct peer connections
- X25519 + HKDF session establishment with RSA-PSS identity proofs
- Encrypted local identity and encrypted chat history at rest
- File transfer, typing indicators, invite links, QR generation, and optional LAN discovery
- Diagnostics bundle export and on-disk panic/crash logs for support
- GUI built with `egui` and terminal interface built with `ratatui`

## Current Status

The project is functional and actively maintained, but it is not a “finished product” in every area.

- Security posture: medium. See [SECURITY.md](SECURITY.md).
- Internet connectivity: still manual. NAT traversal and relay support are not implemented.
- LAN discovery: optional and privacy-sensitive. Disabled by default.
- Packaging: Windows-first. Linux and macOS are supported from source; distribution polish is lighter.

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

### First Launch

On first launch, the app creates an identity and requires password protection before normal use. The unlock/set-password screen is blocking by design.

### Trust Model

The app uses TOFU: trust on first use.

1. Connect to a peer.
2. Verify the displayed fingerprint over a separate trusted channel.
3. Accept only if it matches exactly.

## Platform Notes

| Platform | Status | Notes |
|---|---|---|
| Windows | Supported | Best packaging support. Bonjour may be needed for mDNS discovery. |
| Linux | Supported from source | `avahi-daemon` and GUI system libraries may be required. |
| macOS | Supported from source | Native Bonjour works; packaging is less automated. |

## Documentation

Start with [docs/README.md](docs/README.md).

- [docs/USER_GUIDE.md](docs/USER_GUIDE.md): installation, usage, GUI/TUI flows, troubleshooting
- [DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md): contributor workflow, build/test/release process
- [docs/03_architecture.md](docs/03_architecture.md): codebase architecture and runtime responsibilities
- [docs/04_protocol.md](docs/04_protocol.md): wire protocol and handshake details
- [SECURITY.md](SECURITY.md): security posture, controls, open risks, disclosure
- [THREAT_MODEL.md](THREAT_MODEL.md): assumptions, assets, attack surfaces, limitations
- [DEVELOPMENT_PLAN.md](DEVELOPMENT_PLAN.md): roadmap and backlog
- [docs/AUDITS.md](docs/AUDITS.md): consolidated audit history and findings

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT. See [LICENSE.md](LICENSE.md).
