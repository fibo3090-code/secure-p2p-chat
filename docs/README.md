# Documentation Map

This is the canonical index for project documentation.

## Recommended Reading Order

- Project overview: [../README.md](../README.md)
- Guided first session: [TUTORIAL.md](TUTORIAL.md)
- User setup and usage: [USER_GUIDE.md](USER_GUIDE.md)
- Contribution process: [../CONTRIBUTING.md](../CONTRIBUTING.md)
- Technical developer guide: [../DEVELOPER_GUIDE.md](../DEVELOPER_GUIDE.md)

## Technical Docs

- Architecture: [architecture.md](architecture.md)
- Protocol: [protocol.md](protocol.md)
- Platform plan & roadmap: [platform_spec.md](platform_spec.md)
- Security posture: [../SECURITY.md](../SECURITY.md)
- Threat model: [../THREAT_MODEL.md](../THREAT_MODEL.md)

## Planning and History

- Release history: [../CHANGELOG.md](../CHANGELOG.md)
- Audit history: [AUDITS.md](AUDITS.md)

## What Each Document Owns

- `TUTORIAL.md`: the quickest path from install to a verified chat session
- `USER_GUIDE.md`: day-to-day usage, commands, storage, troubleshooting — the user reference
- `architecture.md`: code layout and runtime responsibility boundaries (today)
- `protocol.md`: shipped wire behavior and compatibility notes
- `platform_spec.md`: the single canonical forward-looking plan — vision, architecture, trust tiers, the Party server, the Tauri/React desktop UI, the phased roadmap, and the backlog
- `SECURITY.md`: current security posture and disclosure guidance
- `THREAT_MODEL.md`: assumptions, assets, attack surfaces, and limits
- `CONTRIBUTING.md`: contribution process, local checks, PR checklist
- `DEVELOPER_GUIDE.md`: toolchain, build/run, code map, release checklist
- `DESIGN_NOTES.md`: UI/UX principles, brand, and theming constraints

`architecture.md` and `protocol.md` describe only what ships today; `platform_spec.md` owns everything forward-looking.

## Naming

The product name is **Encrypted P2P Messenger**. Several internal identifiers
predate the current name and are kept for compatibility and continuity:

| Identifier | Where it appears | Meaning |
|---|---|---|
| `P2PEM` | Tauri desktop app (`p2pem-desktop`, window title, data dir) | Short product identifier for the new desktop app |
| `encodeur_rsa_rust` | Client crate/binary name | Historical crate name; the protocol moved to X25519 session establishment long ago (RSA remains for identity signatures only) |
| `messenger-core` / `messenger-server` | Core and server crate names | Descriptive crate names |
| `chat-p2p://` | Invite-link URI scheme | Wire-compatible URI scheme; renaming it would break existing invites |

## Project Meta

- Contributing: [../CONTRIBUTING.md](../CONTRIBUTING.md)
- Design notes: [../DESIGN_NOTES.md](../DESIGN_NOTES.md)
- Code of conduct: [../CODE_OF_CONDUCT.md](../CODE_OF_CONDUCT.md)
- License: [../LICENSE.md](../LICENSE.md)
