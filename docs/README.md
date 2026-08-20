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
- `async_delivery.md`: design sketch — asynchronous delivery without giving up
  end-to-end encryption. **Not implemented**; a proposal, not a description
- `SECURITY.md`: current security posture and disclosure guidance
- `THREAT_MODEL.md`: assumptions, assets, attack surfaces, and limits
- `CONTRIBUTING.md`: contribution process, local checks, PR checklist
- `DEVELOPER_GUIDE.md`: toolchain, build/run, code map, release checklist
- `DESIGN_NOTES.md`: UI/UX principles, brand, and theming constraints

`architecture.md` and `protocol.md` describe only what ships today; `platform_spec.md` owns everything forward-looking.

## Naming

The product is **P2PEM**. What users download:

| Artifact | What it is |
|---|---|
| **P2PEM Desktop** | *The app.* Tauri + React installers for Windows, macOS, Linux |
| **P2PEM Tools** | Secondary archive: the terminal client (`p2pem`, also runs a relay) and the community server (`p2pem-server`) |

Several internal identifiers predate the current name and are kept for
compatibility and continuity:

| Identifier | Where it appears | Meaning |
|---|---|---|
| `P2PEM` | Desktop app (`p2pem-desktop`, window title, data dir) | The product identifier |
| `p2pem-classic` | Client crate/binary name | Renamed from the historical `encodeur_rsa_rust` (an RSA-era name; the protocol moved to X25519 session establishment long ago, RSA remains for identity signatures only). The "classic" once distinguished the egui GUI, which has been deleted — the crate is now the app core plus the terminal UI, and renaming it is deferred to avoid churning packaging again |
| `messenger-core` / `messenger-server` | Core and server crate names | Descriptive crate names |
| `chat-p2p://` | Invite-link URI scheme | Wire-compatible URI scheme; renaming it would break existing invites |
| `EncryptedMessenger` | Terminal client's data dir (`ProjectDirs`) | Historical; deliberately **not** shared with the desktop app's `P2PEM` dir, so the two are distinct peers |

## Project Meta

- Contributing: [../CONTRIBUTING.md](../CONTRIBUTING.md)
- Design notes: [../DESIGN_NOTES.md](../DESIGN_NOTES.md)
- Code of conduct: [../CODE_OF_CONDUCT.md](../CODE_OF_CONDUCT.md)
- License: [../LICENSE.md](../LICENSE.md)
