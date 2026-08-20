# Audit History

This document consolidates the repository’s audit-oriented notes so that stale one-off reports do not drift away from the current implementation.

## Purpose

- preserve historical audit findings
- keep a record of important quality/security reviews
- avoid leaving stale standalone reports that contradict current code

## Consolidated Historical Notes

### Earlier repository state

Earlier audit notes praised several areas correctly:

- modular Rust structure
- strong cryptographic direction
- good test coverage for core logic
- serious attention to secure storage and replay protection

They also overstated some things that were true only temporarily or were later found to drift:

- documentation consistency
- CI/CD presence in the checked-in repo
- protocol claims around Ed25519 support
- lint/test guarantees as a permanent repo property

Those claims have now been normalized into the maintained docs instead of preserved as “frozen praise.”

### Key issues identified and since addressed

- false Ed25519 negotiation in the runtime handshake
- undeployed AAD usage in critical protocol paths
- broken remove-password flow
- destructive clear-data flow not matching its label
- inconsistent address parsing
- stale and contradictory docs
- missing checked-in CI
- duplicate low-level transfer code path
- UI-thread autosave behavior

### 2026 — desktop-integration & Party audit

Findings from auditing the Tauri desktop app and the Party server against the
shared core (all fixed unless noted; verified against current code):

- **Shared identity between egui and the Tauri app.** The desktop bridge resolved
  its data dir from egui's `ProjectDirs("com","chat-p2p","EncryptedMessenger")`, so
  both loaded the same `identity.json` and became the same peer — connecting them on
  one machine was a self-connection. Fixed: the desktop app uses `ProjectDirs
  "P2PEM"` with a `P2PEM_DATA_DIR` override.
- **Host auto-trusted every incoming peer.** An incoming chat pre-filled its
  `peer_fingerprint`, so the TOFU check trivially matched and silently trusted every
  caller. Fixed: incoming chats start `peer_fingerprint: None`; returning peers are
  recognized by fingerprint across chats/contacts. (See `SECURITY.md` /
  `protocol.md` TOFU notes; regression tests added.)
- **Desktop bridge never persisted history.** Unlock loaded history but nothing saved
  it. Fixed with autosave (poll loop + per-mutation + on-close).
- **Party `DownloadFile` had no access control** — any member could fetch any blob by
  hash. Fixed with `blob_bytes_for(member, hash)` (channel-member / DM-participant
  check); covered by server `state.rs` tests.

### 2026-08 — medium/low sweep

A 23-finding pass over tests, transport and the frontend. Fixed unless noted;
every entry below was checked against the code before being acted on, and three
turned out not to say what they claimed.

**Fixed**

- **mDNS removed the wrong peers, or none.** A `ServiceRemoved` event carries the
  full service name; the code tested `fullname.contains(peer.name)`, where `name`
  holds the *hostname* and the fullname is built from the *instance* name. It
  usually matched nothing, and when it did match it could take "laptop" out along
  with "laptop-alice". Peers now carry the fullname and are removed by exact match.
- **mDNS advertised the identity fingerprint** in its TXT record, and nothing read
  it — discovery supplies an address to dial, and TOFU decides trust. It told every
  device on the network which long-term identity sat at which address, for no
  functional gain. No longer advertised; still parsed, so mixed-version LANs work.
- **Blob reads and writes blocked the async runtime.** Up to 100 MiB of
  `std::fs` under the state mutex inside `serve_connection`'s task, so one member's
  upload stalled unrelated members on that worker. Routed through `block_in_place`,
  with a runtime-flavor check because it panics on the current-thread runtime that
  `#[tokio::test]` provides.
- **A table name was interpolated into SQL** behind a comment promising callers
  would only pass literals. SQLite cannot bind an identifier, so the only real
  defence is a closed set; it takes an enum now.
- **`finalize` had a check-then-act window.** `exists()` followed by `rename`
  lets anything able to write to the download directory create the file in
  between and have it silently replaced. Both receivers reserve the name with
  `create_new` (O_EXCL) first.
- **IPv6 was rate-limited per address.** A /64 is the smallest block anyone is
  routinely delegated, so the limit was bypassed by incrementing a number. Counted
  per /64 now — deliberately not wider, since /48 would bucket unrelated customers
  of one ISP together.
- **A half-open peer held a bridged relay session forever.** `copy_bidirectional`
  cannot tell a quiet conversation from a peer that vanished without a FIN. TCP
  keepalive was the right tool; an idle timeout would disconnect real chats.
- **`fmtTime` rendered the string "Invalid Date"** into message rows — its
  try/catch never fired, because `toLocaleTimeString` does not throw on bad input
  — and `new Date(null)` stamped undated messages 1 January 1970.
- **Password fields carried no `autocomplete`.** The wrong guess is not neutral: a
  manager offered "current-password" on a new-password field fills the old one.
- **`import_contact` believed its caller's `trust_state`.** Not live —
  `parse_invite_link` sets `Unverified` on both paths — but the invariant was
  enforced in one place and relied on in another. Now enforced in the store, on the
  new-contact path only, so re-pasting a link cannot un-verify someone.
- **Test and CI gaps**: contact fuzzing covered only add/remove with random names
  (now the trust lifecycle); nothing tested concurrent `ChatManager` access, the
  pattern both front-ends actually use; the accept-and-continue path in both
  listeners had no test; `npm audit` had no equivalent of `cargo-deny`; the
  frontend was tested only on Ubuntu; and `deny.toml` had no protection against
  its own accept-list going stale.

**Assessed, deliberately not changed**

- **RSA-2048 for identity keys.** Real, and already the roadmap's
  "Ed25519 fingerprint cutover" (platform_spec §12). Changing the identity key
  type changes every fingerprint, so every verified contact would need re-verifying
  — that is a designed migration with its own release, not an audit fix. Proof
  *signatures* already use Ed25519 where both peers support it.
- **`rsa_sign_pss` clones the private key.** True: `SigningKey::new` takes
  ownership and `rsa` exposes no borrowing variant. The clone is `ZeroizeOnDrop`
  like the original, and it happens once per handshake — not per message — on what
  is now the fallback signature path. Avoiding it means caching a `SigningKey`,
  which adds state to buy back one allocation per connection.
- **`PunchOutcome` is unauthenticated.** So is the whole relay control channel,
  by design: the relay is untrusted, and confidentiality comes from the v3
  handshake that runs end-to-end through it. Forging one downgrades a direct
  connection to a bridged one — a routing effect, not a disclosure — and both UIs
  already show which transport won (`p2p:` vs `relay:`), so a forced downgrade is
  visible rather than silent.
- **A rogue UPnP gateway could supply a false external address.** Partly mitigated
  already: `check_routable` rejects private, loopback, link-local, CGNAT and
  documentation ranges on both the UPnP and NAT-PMP paths, so a bogus *unroutable*
  address cannot reach an invite. The residual — a gateway reporting a routable
  address it controls — cannot be validated without an external echo service this
  project deliberately does not operate. `enable_upnp` is off by default, and TOFU
  means whoever answers at that address still cannot impersonate anyone.
- **`bincode` remains an accepted advisory.** It is the only entry naming a direct
  dependency, and retiring it changes the wire format — breaking every deployed
  client and server. A migration with its own release.

**Not reproducible**

- **"Stale gitleaks allowlist entry."** The allowlisted test vector was removed
  from the tree but is still present in five commits, and `gitleaks detect` scans
  full history. Removing the entry would turn CI red. The entry is correct.
- **"CI skips Tauri bridge tests on Windows, so arg-name regressions ship
  undetected."** The skip is real and unavoidable — a Rust test harness linking
  tauri aborts at startup on Windows for want of the manifest `tauri-build` embeds
  into the real binary. But the *conclusion* did not follow: argument binding is
  not OS-dependent. The genuine gap was that nothing checked the JS keys against
  the Rust parameter names at all, on any platform, without linking tauri.
  `invokeContract` now does exactly that, in `npm test`, on all three platforms.
- **"14 accepted advisories in deny.toml."** There are 21, and all 21 still fire —
  none were stale. The list was accurate; what was missing was anything to keep it
  that way.

## Remaining Larger Gaps

These are still roadmap items rather than resolved findings:

- internet-grade connectivity
- stronger discovery privacy model
- richer diagnostics and support tooling

## How to Use This File

Add new audit summaries here only after they have been checked against the current codebase and linked back to maintained docs:

- [SECURITY.md](../SECURITY.md)
- [THREAT_MODEL.md](../THREAT_MODEL.md)
- [docs/platform_spec.md](platform_spec.md)
