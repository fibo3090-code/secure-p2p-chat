# Security

**Last Updated:** July 13, 2026

This document describes the current security posture of the project, what protections are implemented today, what risks remain open, and how to report issues responsibly.

The division of labor across security documents:

- this file: posture summary, open risks, supported versions, and disclosure process
- [THREAT_MODEL.md](THREAT_MODEL.md): assumptions, assets, attack surfaces, and per-surface controls
- [docs/protocol.md](docs/protocol.md): the cryptographic mechanics of the shipped wire protocol
- [docs/AUDITS.md](docs/AUDITS.md): audit history and resolved findings

## Supported Versions

Only the latest released version receives security fixes.

| Version | Supported |
|---|---|
| Latest release (see [CHANGELOG.md](CHANGELOG.md)) | Yes |
| Older releases | No — upgrade to the latest release |

## Verifying a Download

Releases are **not** signed with an OS code-signing certificate, so Windows
SmartScreen and macOS Gatekeeper will warn about the installers. That warning is
about a missing certificate, not about the contents. What a certificate would
prove — that a file came from this project and has not been altered — is proved
two other ways instead, both free and both checkable by you:

**Build provenance** (the stronger one). Every release artifact is attested with
[Sigstore](https://www.sigstore.dev/) via GitHub's `attest-build-provenance`,
binding the file's digest to this repository, the exact commit, and the workflow
run that produced it, and recording it in a public transparency log:

```bash
gh attestation verify <downloaded-file> --repo <owner>/<repo>
```

This is a claim a code-signing certificate cannot make. A certificate says "some
holder of this key produced this file"; the attestation says "this file was
built by this workflow, from source you can read, at this commit".

**Checksums.** Each release carries a `SHA256SUMS` asset:

```bash
sha256sum --ignore-missing -c SHA256SUMS      # Linux
shasum -a 256 --ignore-missing -c SHA256SUMS  # macOS
```

Note the limit: a checksum only proves the file matches the release page. If the
release page itself were the thing under an attacker's control, the checksum
would match and prove nothing. Provenance does not have that weakness — verify
that one if you verify only one.

There is **no auto-updater**. Security fixes reach you only if you come back and
download them, so watch the repository for releases if you rely on this.

## Security Posture

Current overall risk assessment: **medium**.

> This is a **self-assessment**, not an audit. No independent third party has
> reviewed this code. Treat the rating as the maintainers' honest read of their
> own work, which is exactly the kind of claim you should discount. If your
> threat model involves a well-resourced adversary, use something that has been
> audited. Independent review is welcome — see [Responsible Disclosure](#responsible-disclosure).

Reasoning:

- strong modern transport/session primitives are in place
- identity and history are encrypted at rest, behind an Argon2id-stretched
  password with an enforced minimum length (`MIN_PASSWORD_LEN`, checked in
  `Identity::encrypt` so no front-end can set a weaker one)
- protocol correctness and replay protection have been hardened
- some product-grade gaps remain, especially around discovery privacy, relay operational hardening, and dependency posture
- there is **no offline delivery for direct peer-to-peer conversations**: if
  both peers are not online at the same time (or connected through a relay),
  a message is not delivered. Only the community server buffers history. This
  is a consequence of having no always-on infrastructure holding your
  messages, and it is a real product limitation, not just a design note

## Implemented Protections

In transit, sessions are established with X25519 ECDH and HKDF-SHA256 (providing forward secrecy), encrypted with AES-256-GCM under transcript-bound AAD, replay-protected by per-session sequence validation, and automatically rekeyed every 100 messages (or 5 minutes). The X25519 exchange rejects all-zero peer keys and non-contributory (low-order-point) shared secrets, so a peer cannot force a predictable session key. At verification time both peers derive an identical transcript-bound **Short Authentication String** (six digits + three emoji); users compare that short code out-of-band to catch an active MITM without reading a 64-character fingerprint (an interposed attacker's two handshakes produce two different codes). Rekeying is initiated by a single deterministic side (the host) so the two peers never rotate simultaneously, and the receiver keeps the previous key for a bounded window (until the first frame decrypts under the new key) so peer frames still in flight under the old key are not lost — together these keep a rotation from desyncing the keys and dropping the session. Identity is a long-term RSA-2048 key used **only for RSA-PSS signatures** (identity proofs and signed invites) — the product performs **no RSA encryption/decryption at all** (those functions were removed from the codebase). At rest, the identity keystore is encrypted with Argon2 + ChaCha20-Poly1305 and chat history is encrypted, with zeroization applied to sensitive in-memory material where implemented. Full mechanics: [docs/protocol.md](docs/protocol.md).

Operational hardening includes handshake timeouts, rate limiting, DoS-hardened framing (bounded length prefix, chunked reads, oversized-packet rejection), signed invite links, a self-hosted relay mode that forwards only already-encrypted session traffic, and server-side access checks on Party file downloads (`blob_bytes_for`: only channel members or DM participants can fetch a blob). CI enforces formatting, lints, cross-platform tests, and locked build verification.

### At-rest durability

The identity file is the **only** key to the message history (`history_key` is derived from the private key), so losing or corrupting it is unrecoverable by design. Two rules protect it:

- **Every write is atomic** (`messenger_core::util::write_file_atomic`): the content goes to a uniquely-named temporary file in the same directory, is `fsync`ed, and is then renamed over the destination; on Unix the directory is `fsync`ed too, and the file is created `0600` rather than widened and narrowed. An interrupted write therefore leaves the *old* identity, never a truncated one. The same path is used for the encrypted history.
- **An unreadable identity is a hard error, never replaced.** If `identity.json` exists but cannot be parsed, the app refuses to start and says so, leaving the file untouched for restoration from a backup. Generating a replacement would look like a graceful fallback while silently making all stored messages undecryptable and changing the fingerprint every contact had verified — presented to the user as a blank, freshly-installed app. Absence of the file is still a normal first run.

Migrating a legacy plaintext history escalates rather than shrugs: the plaintext file is deleted; if it cannot be unlinked it is truncated to zero bytes so the content is gone anyway; and if even that fails the user gets an error naming the file, instead of a log line saying their messages are still readable on disk.

### Desktop app (Tauri) attack surface

The Tauri 2 desktop app (`p2pem-desktop`) adds a system-webview + IPC surface:

- all cryptography stays in Rust; the React webview only calls `#[tauri::command]`s and never handles key material
- `tauri.conf.json` sets a restrictive **CSP** (`default-src 'self'`; no external hosts, scripts, or fonts) and disables the global Tauri injection (`withGlobalTauri: false`)
- the desktop app uses its **own** data directory (`ProjectDirs "P2PEM"`), distinct from the terminal client's, so the two are separate identities rather than sharing one keystore
- the bridge is covered by an IPC-level integration suite (`desktop/src-tauri/src/tests.rs`, 16 tests) that drives the real command handlers through Tauri's mock runtime. It asserts the **auth barrier** holds — no state-mutating command runs before unlock/set-password — that the password floor is enforced by the core rather than the UI, and that every frontend payload key still binds to its command parameter. CI runs it on Linux and macOS; it is skipped on Windows, where a Rust test-harness executable linking Tauri aborts at startup (`STATUS_ENTRYPOINT_NOT_FOUND`), not because the suite is platform-specific
- the React layer is covered by unit tests (`npm test`) over its pure logic — password policy, safety-grid rendering, unread accounting, design-token drift. Behaviour that only exists inside a live webview is **not** automatically tested; treat rendering as build-verified rather than test-covered

## Current Limits and Open Risks

- WAN support relies on optional UPnP/NAT-PMP port mapping (off by default —
  it opens a router port and embeds the external IP in invites) or a
  self-hosted relay. The relay first coordinates a **TCP hole punch** (both
  peers learn each other's relay-observed public endpoint and attempt a
  simultaneous open from the reused source port), so most sessions go direct
  and the relay only bridges when punching fails (symmetric NAT, CGNAT,
  filtered networks). The punch hello tag is derived from the rendezvous
  token and is pairing hygiene only — authentication is still the v3
  handshake + TOFU on whichever socket wins. `P2PEM_NO_HOLEPUNCH=1` forces
  the bridged path. Punching also reveals each peer's public *and* LAN
  endpoint to the other (the relay already saw both source addresses)
- Multi-address invites embed **both** the external and the LAN address when
  UPnP is enabled, so a shared invite reveals the private LAN IP alongside the
  public one — share invites only with people you intend to reach you
- LAN discovery exposes metadata tradeoffs when enabled
- The runtime keeps a signature-scheme field on the wire, but currently only supports RSA-PSS identity proofs
- Signed invites expire 30 days after issuance (the signature covers the
  timestamp); legacy v1 unsigned invites carry no timestamp and are not
  subject to expiry
- Invite revocation (before expiry) is not supported
- **No post-compromise security (no double ratchet).** Forward secrecy comes
  from ephemeral-DH session keys plus symmetric rekeying (`next = HKDF(current,
  nonce)`). Because rekeying folds in no *new* DH material, an attacker who
  captures a live session key can follow the ratchet forward from that point
  (the nonces travel on the wire) until the session ends and a fresh ephemeral
  handshake runs. A Signal-style Double Ratchet (a DH step per ratchet) would
  add self-healing after key compromise; it is a deliberate, larger protocol
  change, not yet implemented.
- **TOFU has no key transparency.** Trust is pinned on first use and a later
  key change is surfaced loudly, but there is no external auditable log
  (CONIKS/Keybase-style) to detect a malicious key swap presented consistently
  to a victim from the start. Out-of-band fingerprint comparison remains the
  backstop.
- `rsa` is still a dependency (for RSA-PSS **signatures**), so the upstream
  timing advisory `RUSTSEC-2023-0071` still appears in audits — but the attack
  it describes targets RSA **decryption**, which this product does not perform
  (the RSA encrypt/decrypt functions were removed). Migrating identity
  signatures to Ed25519 (the wire already negotiates a scheme field) would drop
  `rsa` from the security-critical path entirely.
- `bincode` remains a tracked dependency migration concern
- Known-accepted `cargo audit` findings beyond `rsa` above, each with the exact
  path that pulls it (verified with `cargo tree --target all -i`, since a
  "transitively via Tauri" hand-wave was wrong here once already):
  - **`quick-xml` 0.39.4** (RUSTSEC-2026-0194/0195, both DoS-on-hostile-XML):
    reached **only** on Linux/Wayland via
    `rfd → wayland-client/wayland-protocols → wayland-scanner`.
    `wayland-scanner` is a **proc macro**: it parses the Wayland protocol XML
    that ships inside the crate, at build time, and is never linked into the
    shipped binary. The input is neither peer-controlled nor present at
    runtime. The app's *own* Tauri path resolves `quick-xml` 0.41.0, which is
    already fixed.
  - **Unmaintained GTK3 bindings** (`gtk`, `gdk`, `atk`, `glib`, … 0.18):
    Tauri's Linux backend (`muda`, `tao` → `tauri-runtime-wry`) until it moves
    off GTK3. Not something this project can resolve independently.
  - **`bincode` 1.3.3 unmaintained** (RUSTSEC-2025-0141): a direct `core`
    dependency used for the relay control protocol. Migration is tracked; the
    frames it decodes are length-bounded by `MAX_PACKET_SIZE` before parsing.

  Re-evaluate on every Tauri upgrade; everything else in the tree is kept
  current via in-semver `cargo update`.

## Trust Model

The app uses TOFU (trust on first use).

Users are responsible for verifying fingerprints on first contact using a separate trusted channel. Without that step, first-contact MITM remains possible.

## What This App Does Not Claim

It does not currently claim:

- anonymity against traffic analysis
- managed relay infrastructure or anonymous global connectivity
- Ed25519 identity-key support in the shipped runtime
- invite revocation before the 30-day expiry
- full protection against a compromised local machine

## Responsible Disclosure

Security issues must **not** be reported in public issues.

Report vulnerabilities privately through [GitHub's private vulnerability reporting](https://github.com/fibo3090-code/secure-p2p-chat/security/advisories/new) for this repository.

Preferred report contents:

- affected version
- reproduction steps
- impact
- affected components
- proof of concept if available
- mitigation ideas if known
