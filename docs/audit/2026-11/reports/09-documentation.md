# Documentation audit

Repository: `/home/user/secure-p2p-chat` at `62b952f` (merge of PR #188), workspace version `1.16.2`.
Method: every Markdown file read in full, plus issue/PR templates, `//!` crate docs of `core`, `client`, `server`, `design/tokens.json` `$comment`, and the doc-like comments the coordinator flagged. Each concrete claim was checked against the source with `rg`/`sed`. No files in the repo were modified. (`fd` is not installed in this environment, so `find` was used.)

## Scope covered

| File | Lines | Purpose / audience | Verdict |
|---|---|---|---|
| `README.md` | 130 | Front page for end users | Good first impression: one-line pitch, a screenshot that exists, a download table, a frank "Honest limitations" section. A few small errors (installer list, `cargo test` instead of nextest). |
| `CHANGELOG.md` | 1395 | Release history for users and operators | Keep-a-Changelog layout, user-facing wording. But `[Unreleased]` holds about 1,900 words, mostly security fixes, and has gone 2.5 months without a release. Some compare links are missing. |
| `CONTRIBUTING.md` | 69 | Contributor process | Accurate but partial. Its local-check list is not what CI runs (`--profile ci`, fuzz check, deny, npm audit and the dist check are missing). |
| `DEVELOPER_GUIDE.md` | 161 | Contributor technical guide | Mostly accurate. It repeats CLAUDE.md, gives a test command that differs from CONTRIBUTING, and says nothing about fuzzing or `run-codeql-local.ps1`. |
| `DESIGN_NOTES.md` | 77 | UI/UX principles, brand, tokens | Accurate on tokens. The "Current Gaps" list is stale: cancellation has shipped. |
| `SECURITY.md` | 218 | Security posture, verifying downloads, disclosure | Strong download-verification section. The accepted-advisory list and the test-coverage claims are stale, and the doc contradicts itself on Ed25519 and `rsa`. |
| `THREAT_MODEL.md` | 200 | Security reviewers | Claims a control that does not exist (an "operator can read" badge). Says invite expiry is not enforced, but it is. Misses the file-transfer, community-join and UPnP surfaces. |
| `CODE_OF_CONDUCT.md` | 46 | Community | Contributor Covenant 1.4 with a valid contact. Fine. |
| `LICENSE.md` | 21 | Legal | MIT. Fine. |
| `CLAUDE.md` | 309 (56 KB) | Coding-agent guide | Valuable, but it breaks its own "keep this file short" rule. Several claims are wrong (relay fallback, upload ids, spool location, root `src/`, `rg --include`, the command list). |
| `docs/README.md` | 69 | Doc index and naming table | Accurate. Good naming table. |
| `docs/TUTORIAL.md` | 229 | End-user first session | Mostly right. Its verification step talks about fingerprints and the grid, not the six-digit SAS that both UIs lead with. |
| `docs/USER_GUIDE.md` | 525 | End-user and operator reference | The most error-dense file. Wrong binary name, a "Delete Everything" feature that does not exist, an uninstaller cleanup that does not exist, a `:set` key list that omits `upnp`, about 16 missing TUI commands, no SAS, community pinning described wrongly, and an egui-era "log terminal". |
| `docs/architecture.md` | 313 | Contributor code map | Stale paths after PR #188 (`tui/app.rs`, `server/src/state.rs`, missing `identity/invite.rs` and `tui/downloads.rs`). Stale on frontend tests and on how often history is saved. |
| `docs/protocol.md` | 476 | Wire spec for security reviewers | Accurate on the P2P handshake, SAS, rekey, invites, relay and Ed25519. Wrong on Party files: it says chunked transfer is "not yet implemented" and leaves 7 request and 2 response variants out of its lists. |
| `docs/platform_spec.md` | 1031 | Forward-looking plan and roadmap | Mixes shipped and planned. Its "Current state" was verified at 1.15.0. It contradicts itself (file permissions, async P2P delivery), repeats the rail paragraph, has an unresolved `[FilePermissions]` reference link, and its test counts are stale. |
| `docs/server_tiers.md` | 231 | Design proposal (clearly marked) | Well framed. Says `TrustTier` has an `E2EE` variant, which is correct. Lists operator moderation powers (remove a member, delete a message) that do not exist. |
| `docs/AUDITS.md` | 175 | Audit history | Useful. Some entries are now superseded (`block_in_place`, `state.rs`, "21 advisories") without saying so. Does not mention GHSA-6q3g-734c-22jm. |
| `core/fuzz/seeds/README.md` | 26 | Fuzz seed corpus rationale | Accurate and concise. |
| `.github/PULL_REQUEST_TEMPLATE.md` | 23 | PR authors | Out of step with CONTRIBUTING and CI: no doctests, no `npm test`, lint or smoke. |
| `.github/ISSUE_TEMPLATE/bug_report.yml` | — | Bug reporters | Placeholder says "v1.8.1" and "Start the GUI". No field for which client (Desktop, TUI, server, relay) is affected. |
| `.github/ISSUE_TEMPLATE/feature_request.yml`, `config.yml` | — | Reporters | Fine. Blank issues are off and links are valid. |
| `//!` docs (`core/src/lib.rs`, `server/src/main.rs`, `server/src/lib.rs`, `client/src/main.rs`, `client/src/tui/command.rs`) | — | API readers | Present and mostly accurate. They use the old "Encrypted Messenger" name. `core::party::TrustTier` says it is "surfaced prominently in the UI", which is false. |
| `design/tokens.json` `$comment` | — | Contributors | Stale. Points to the deleted `client/src/gui/styling.rs`. |

All 71 relative links and 9 anchors in the Markdown resolve. The one referenced image (`docs/images/desktop-chat.png`) exists. The docs are all in English; nothing is in French.

## Summary

The documentation is unusually candid. "Honest limitations", "this is a self-assessment" and "Counts drift — re-measure" are good habits. The security and protocol material is detailed and mostly correct for the P2P core.

The problems cluster in four places:

1. **The user-facing reference (`USER_GUIDE.md`)** has drifted from the product:
   - It describes data-deletion features that do not exist.
   - It gets the server binary name wrong.
   - It documents roughly two thirds of the TUI command set.
   - It never mentions the SAS short code that the product now leads with.
2. **Security docs claim controls that do not exist.**
   - THREAT_MODEL relies on an "operator can read messages" badge that is not in either UI.
   - Docs present the community-join flow as verified by a code the operator can read out. The server never computes or shows that code.
   - The second, credential-carrying connection is not bound to the identity the user just checked.
3. **The Party protocol spec is behind.** Chunked upload and download, `ShareFile` and `SetFilePermissions` shipped but are missing from `protocol.md`, which says chunking is not implemented.
4. **Structure.**
   - Key knowledge lives only in a 56 KB `CLAUDE.md`, a file the project's own rule says should be short.
   - Build and test instructions are repeated in four places that disagree with each other.
   - There is no operator guide, backup and restore guide, data-removal guide or privacy/metadata page.

Totals: 40 findings (High 7, Medium 17, Low 13, Info 3).

## Proposed target documentation structure

| Audience | Doc | Source material and what to change |
|---|---|---|
| **Everyone** | `README.md` | Keep as is. Fix the installer table (add `.msi` and `.rpm`, or drop `rpm` from bundle targets). Link a new FAQ. |
| **End users** | `docs/user/getting-started.md` (now `TUTORIAL.md`) | Rewrite step 4 around the SAS. Keep it short. |
| | `docs/user/guide.md` (now `USER_GUIDE.md`, desktop half) | Desktop reference: connect, contacts and blocking, the file acceptance gate and executable warning, communities (roles, Drive, permissions), settings. |
| | `docs/user/tui.md` (TUI half of `USER_GUIDE.md`) | Generate the command table from `client/src/tui/command.rs::COMMANDS`, or test it against that table. |
| | `docs/user/data-and-privacy.md` (**new**) | Data directory per OS for each app, what is stored, what metadata leaks to peers, relays, LAN, UPnP gateways and community operators. Backup **and restore** (identity *and* history). Moving from Classic. Uninstall and full data removal. |
| | `docs/user/troubleshooting.md` (**new**, from USER_GUIDE §Troubleshooting) | Add a FAQ, unsigned-installer prompts (SmartScreen, Gatekeeper), port conflicts, a changed fingerprint. |
| **Operators** | `docs/operators/community-server.md` (**new**) | Merge the USER_GUIDE "Host a community" section with the server's `--help`. Cover the binary name, `PARTY_PASSWORD` via environment rather than argv, firewall, systemd unit, backing up `party-data/` (`party.db`, `blobs/`, server key), upgrades (client and server must match), quotas and limits, and how members verify the server (fingerprint from the log). State plainly that the operator can read everything. |
| | `docs/operators/relay.md` (**new**) | `--relay-server`, the default port clash with 12345, rate limits, the rendezvous timeout, what the relay learns. |
| **Contributors** | `CONTRIBUTING.md` | Process only. Add per-OS setup (Node 20+, GTK/WebKit packages, nextest) and **one** CI-equivalent script, for example `scripts/ci-local.sh`. |
| | `DEVELOPER_GUIDE.md` | Build, test, fuzz (`scripts/fuzz.sh`), CodeQL (`run-codeql-local.ps1`), release. Remove the duplicated invariants and link to architecture instead. |
| | `docs/architecture.md` | The code map. Fix the paths, and absorb the "gotchas" from CLAUDE.md as a "Design invariants and why" section. |
| | `docs/release.md` (optional, split from DEVELOPER_GUIDE) | Release checklist, attestation, cadence for security fixes. |
| **Security reviewers** | `SECURITY.md`, `THREAT_MODEL.md`, `docs/protocol.md`, `docs/AUDITS.md` | Keep the split. Add a **published advisories** table (GHSA-6q3g-734c-22jm) to SECURITY.md. Add the Party chunked and permission messages to protocol.md. Add surfaces to THREAT_MODEL: file receipt, community join, UPnP. |
| **Planning** | `docs/platform_spec.md`, `docs/server_tiers.md` | Forward-looking only. Remove the "Current state" restatements and point at architecture.md. |
| **Agents** | `CLAUDE.md` | Trim to about 100–150 lines: layout, commands, footguns as one-liners with a link each. Move the long "why" paragraphs to architecture.md, protocol.md and SECURITY.md. |

## Findings

### DOC-01 — Community verification asks the user to compare a code the operator cannot see, and step 2 is not bound to step 1
- Severity: High
- Category: wrong
- Status: VERIFIED
- Location:
  - Docs: `CLAUDE.md:262`; `docs/architecture.md:248-249`; `docs/USER_GUIDE.md:233-235`
  - UI copy: `desktop/src/components/Parties.jsx:92-95` ("Ask the operator to read out their code"); `client/src/tui/app/cmd_party.rs:49` ("Compare it with the operator")
  - Server: `server/src/main.rs:78-85` (logs the fingerprint only); `rg -i sas server/src` finds nothing
  - Client: `core/src/party/mod.rs:857` (SAS is computed on the client side only)
  - Step 2: `client/src/tui/app/cmd_party.rs:79-86` and `desktop/src-tauri/src/commands/party.rs:340-346` call `connect_and_join(..., None/pinned, true)`; `client/src/app/party_manager.rs:427` (`None => {}` accepts any key)
- Problem:
  - The SAS is bound to each connection's transcript. The server never computes, logs or displays it, so an operator has nothing to "read out". The only out-of-band value an operator has is the fingerprint in the startup log.
  - After the user confirms, both shells **reconnect** with no pin and `trust_new_identity: true`. That second connection, which carries the username and password, accepts whatever key answers. The identity the user just checked is never compared.
  - CLAUDE.md's "no credential moves before step two" and architecture.md's "server-identity verification before credentials are sent" therefore overstate what happens.
- Recommendation:
  - Docs: tell users to compare the **full fingerprint** printed by `p2pem-server` at startup, and say so in an operator guide. Correct CLAUDE.md and architecture.md.
  - Code (hand to the security auditor): pass the fingerprint from step 1 as `expected_fingerprint` in step 2, and either log a per-connection SAS on the server or drop the SAS from the community prompt.
- Effort: S for docs, M for code.

### DOC-02 — "Settings → Delete Everything" and a Windows uninstaller data wipe are documented but do not exist
- Severity: High
- Category: wrong
- Status: VERIFIED
- Location:
  - Docs: `docs/USER_GUIDE.md:447-448`
  - Code: `desktop/src/components/Settings.jsx` has no such control (`rg -i "delete|erase|wipe"` finds only the password copy). `ChatManager::delete_all_data` (`client/src/app/chat_manager/mod.rs:799`) has **no non-test caller**. `tauri.conf.json` has no NSIS/WiX uninstall hook, and `setup.iss` was deleted.
- Problem: Users are told they can wipe their identity, history and diagnostics from Settings, and that the Windows uninstaller offers the same. Neither exists. The TUI's `:clear-history` only clears chats and contacts. This is a privacy promise the product does not keep.
- Recommendation: Replace the section with a manual removal procedure that lists the data directory per OS for Desktop (`ProjectDirs com/chat-p2p/P2PEM`) and for the TUI (`EncryptedMessenger`). Either wire `delete_all_data` to a command or delete it.
- Effort: S (docs), M (feature).

### DOC-03 — THREAT_MODEL relies on an "operator can read messages" badge that is not implemented
- Severity: High
- Category: wrong
- Status: VERIFIED
- Location:
  - Docs: `THREAT_MODEL.md:97` and `:181` (controls); `docs/platform_spec.md:232`, `:239` ("wears an honest … badge", "shown prominently")
  - Code: rustdoc `core/src/party/mod.rs:23-24` ("surfaced prominently in the UI"). No `tier` or "can read" text in `desktop/src` or `client/src/tui`. `TrustTier` appears only as a hard-coded `Administered` in `client/src/app/party_manager.rs`.
  - Corroboration: `docs/platform_spec.md` §11 Phase 3 lists "trust-tier labeling" as **remaining**.
- Problem: A security document lists consent through labelling as a mitigation for the operator reading plaintext. The UI only says "administered", which does not tell a user the operator can read their DMs.
- Recommendation: Move this to "Residual limitation" until it ships. Fix the `TrustTier` rustdoc. Ideally add a one-line warning to the join card and the TUI toast.
- Effort: S.

### DOC-04 — `protocol.md` says Party chunked transfer is not implemented and omits 9 shipped message variants
- Severity: High
- Category: stale
- Status: VERIFIED
- Location:
  - Docs: `docs/protocol.md:454-455`, `:422-432`
  - Code: `core/src/party/mod.rs:388-430` (`StartUpload`, `UploadChunk`, `FinishUpload`, `CancelUpload`, `DownloadChunk`, `ShareFile`, `SetFilePermissions`), `:640` (`UploadReady`, `FileChunk`), `:54` (`PARTY_CHUNK_BYTES` = 256 KiB), `:61` (`MAX_PARTY_FILE_BYTES` = 100 MiB)
  - Contradicted by: `docs/platform_spec.md:388-398` ("Chunked transfer (shipped)")
- Problem: The wire spec, which is what a security reviewer reads, is missing the chunked upload and download state machine. That includes the server-minted upload id, the per-connection concurrency cap, the quota checks done before any bytes move, and the per-file permission model.
- Recommendation: Add a "Chunked files" and "Per-file permissions" subsection. Move the text over from platform_spec §8 and trim it there.
- Effort: M.

### DOC-05 — Wrong server binary path in the build instructions
- Severity: High
- Category: wrong
- Status: VERIFIED
- Location:
  - Docs: `docs/USER_GUIDE.md:43` (`target/release/messenger-server[.exe]`)
  - Code: `server/Cargo.toml:14-16` (`[[bin]] name = "server"`); `.github/workflows/release.yml:368` (`cp target/release/server … p2pem-server`)
- Problem:
  - Building from source produces `target/release/server`, not the documented path.
  - The same program has three names in docs and artifacts: `messenger-server` (crate), `server` (binary), `p2pem-server` (Tools archive).
  - `Parties.jsx:122` also tells users communities are "served by the `messenger-server` crate".
- Recommendation: Rename the bin to `p2pem-server`, or document `target/release/server`. Use one name everywhere.
- Effort: S.

### DOC-06 — The SAS short code is missing from the user verification instructions
- Severity: High
- Category: stale
- Status: VERIFIED
- Location:
  - Docs: `docs/TUTORIAL.md:117-129`; `docs/USER_GUIDE.md:254-268`, `:359-361`; `SECURITY.md:191-193`
  - Code: `client/src/tui/overlays.rs:224-270` (SAS rendered first, "the low-friction check"); `CLAUDE.md` § SAS ("Both UIs lead the verification prompt with it")
- Problem: The README promises "six digits and three emoji, read them aloud". The tutorial and guide then tell users to compare the 64-character fingerprint or safety grid, and never mention the SAS. That is the step that actually defeats a MITM, and the docs make it harder than it is.
- Recommendation: Lead both docs with "compare the six-digit code and emoji over a call", and move the fingerprint/grid to an "advanced" note. Add a line on what to do when the codes differ.
- Effort: S.

### DOC-07 — THREAT_MODEL says invite expiry is not enforced
- Severity: High
- Category: wrong
- Status: VERIFIED
- Location:
  - Docs: `THREAT_MODEL.md:192` ("no invite revocation or expiry enforcement")
  - Code: `core/src/lib.rs:66` (`INVITE_MAX_AGE_SECS` = 30 days)
  - Contradicted by: `SECURITY.md:144-147`, `docs/USER_GUIDE.md:153-154`
- Problem: The security docs contradict each other on a trust-bootstrap control.
- Recommendation: Change the line to "no invite revocation before the 30-day expiry".
- Effort: S.

### DOC-08 — The TUI `:set` key list, `settings_keys()` and help text all omit `upnp`, which the guide uses
- Severity: Medium
- Category: wrong
- Status: VERIFIED
- Location:
  - Docs: `docs/USER_GUIDE.md:170` (`:set upnp on`) vs `:420-421` (key list without `upnp`)
  - Code: `client/src/tui/command.rs:510-521` (`settings_keys`), `:271` (help string); `client/src/tui/app/mod.rs:1325-1328` (handler exists)
- Problem: The setting works, but neither the doc list nor the in-app `:help` or error message shows it.
- Recommendation: Add `upnp` to `settings_keys()`, the help string and the USER_GUIDE list. Add a test that `apply_setting`'s keys equal `settings_keys()`.
- Effort: S.

### DOC-09 — The USER_GUIDE TUI command reference omits about 16 commands
- Severity: Medium
- Category: missing
- Status: VERIFIED
- Location:
  - Docs: `docs/USER_GUIDE.md:369-418`
  - Code: `client/src/tui/command.rs:136-276` (`COMMANDS`)
- Problem: These commands are missing from the guide:
  - Connections: `:connection-password`, `:lock`
  - Files: `:accept`, `:decline` (needed because auto-accept is **off** by default)
  - Identity: `:name`, `:backup`
  - Communities: `:party-trust` (needed to join any community at all), `:party-send-file`, `:party-download`, `:party`, `:party-files`, `:party-delete-file`, `:party-role`, `:party-channel-access`, `:party-delete-channel`, `:party-audit`
  - Undocumented CLI flags: `-H`, `--tui` (a no-op), `--version`
  - Separately, the examples at `:103-107` use `--tui`, which does nothing (`client/src/main.rs:53-55`).
- Recommendation: Generate this section from `COMMANDS`, or add a test that every `COMMANDS` name appears in USER_GUIDE.md, the same way `invokeContract` guards bridge keys.
- Effort: S.

### DOC-10 — USER_GUIDE describes community pinning wrongly, and the TUI never pins
- Severity: Medium
- Category: wrong
- Status: VERIFIED
- Location:
  - Docs: `docs/USER_GUIDE.md:233-235` ("their app pins the fingerprint on first join and warns them if it ever changes"), `:242-243` (TUI join documented without `:party-trust`; the desktop bullet appears twice)
  - Code: `client/src/tui/app/cmd_party.rs:19-27` ("the TUI has no saved-communities file … Passing None")
- Problem:
  - Only the desktop app pins, through `parties.json`. A TUI user is never warned about a changed server key.
  - The first-join flow, which stops for verification and needs `:party-trust`, is not described at all.
- Recommendation: Document the two-step join for each UI and state the TUI's limitation. Remove the duplicate bullet.
- Effort: S.

### DOC-11 — `SECURITY.md`'s accepted-advisory list is stale
- Severity: Medium
- Category: stale
- Status: VERIFIED
- Location:
  - Docs: `SECURITY.md:179-181` (GTK3 bindings listed as accepted), `:182-184`
  - Code: `deny.toml:23-34` (GTK3 advisories withdrawn upstream and removed 2026-09-09), `:21` (paste), `:35` (proc-macro-error), `:49-57` (zvariant/zbus RUSTSEC-2026-0370/71/72, accepted 2026-11-03)
- Problem:
  - Readers see an acceptance that no longer exists and miss five that do.
  - The `bincode` entry says it is used "for the relay control protocol". It is also used for Party frames and `IdentityProof`/`Ed25519Proof` (`util::decode_exact`, CLAUDE.md).
- Recommendation: Generate this list from `deny.toml`, or link to it as the single source. `scripts/check-stale-advisories.sh` already exists to keep it honest.
- Effort: S.

### DOC-12 — SECURITY.md and architecture.md understate frontend testing and miscount bridge tests
- Severity: Medium
- Category: stale
- Status: VERIFIED
- Location:
  - Docs: `SECURITY.md:123` ("16 tests"), `:124` ("unit tests … over its pure logic", "rendering … build-verified rather than test-covered"); `docs/architecture.md:305`; `docs/platform_spec.md:930-935`, `:983-988` (457 / 17 / 37 tests)
  - Code: `desktop/src-tauri/src/tests.rs` has 17 test attributes. About 284 vitest `it`/`test` cases. jsdom component tests (`*.test.jsx`). Playwright `desktop/smoke/*.spec.js`. CLAUDE.md cites "844 other tests".
- Problem: The security posture doc says the React layer has no component or launch testing, which has not been true since jsdom and Playwright were added. The counts disagree everywhere.
- Recommendation: Remove hard-coded counts, or say "about, as of X". Describe the three frontend layers once, in DEVELOPER_GUIDE, and link to it.
- Effort: S.

### DOC-13 — `architecture.md` code map is stale after PR #188
- Severity: Medium
- Category: stale
- Status: VERIFIED
- Location:
  - Docs: `docs/architecture.md:99-104` (`tui/app.rs`), `:111` (`server/src/state.rs`), `:63-79` (no `identity/invite.rs`, `core/mod.rs`, `network/mod.rs`, `transfer/mod.rs`), `:94` (invites "v1/v2/v3", though v4 exists); `docs/AUDITS.md:62` (`state.rs` tests)
  - Code: `client/src/tui/app/{mod,cmd_connect,cmd_contacts,cmd_misc,cmd_party}.rs`, `client/src/tui/{mod,downloads}.rs`, `server/src/state/{mod,blobs,db,tests}.rs`, `core/src/identity/invite.rs`, `client/src/app/chat_manager/adversarial_tests.rs`
- Problem: The canonical code map points at files that no longer exist.
- Recommendation: Update the tree. Consider a CI check that every path in backticks in `docs/*.md` exists.
- Effort: S.

### DOC-14 — `architecture.md` says history is rewritten once per message
- Severity: Medium
- Category: stale
- Status: VERIFIED
- Location:
  - Docs: `docs/architecture.md:307-312`
  - Code: `desktop/src-tauri/src/lib.rs:113` (`HISTORY_SAVE_MIN_INTERVAL` = 3 s); CLAUDE.md "History writes are coalesced"
- Problem: Saves are now coalesced to at most one per interval. The O(total) cost per save still holds; "once per message" does not.
- Recommendation: Reword to "the whole history is re-encrypted on each save (coalesced to ≤1 per 3 s on desktop)".
- Effort: S.

### DOC-15 — `CLAUDE.md` claims `connect_to_contact` has a relay fallback
- Severity: Medium
- Category: wrong
- Status: VERIFIED
- Location:
  - Docs: `CLAUDE.md:264`
  - Code: `client/src/app/chat_manager/connect.rs:497-505` (relay is used **only** when there are no direct candidates; a failed direct attempt returns `Err`); the comment at `:488-490` is misleading too
- Problem: An agent or contributor would assume a contact with both addresses and relay details falls back to the relay when direct fails. It does not.
- Recommendation: Fix the wording (and the code comment), or implement the fallback.
- Effort: S.

### DOC-16 — `CLAUDE.md` says chunked upload ids are client-chosen
- Severity: Medium
- Category: wrong
- Status: VERIFIED
- Location:
  - Docs: `CLAUDE.md:265` ("reservation is keyed by the client's own upload id")
  - Code: `core/src/party/mod.rs:388-393` (`StartUpload` carries no id); `server/src/dispatch.rs:548-552` (`state.start_upload` returns the id, sent back in `UploadReady { upload }`)
- Problem: The server mints the id. Getting this wrong matters for reasoning about cross-connection id collisions and spoofing.
- Recommendation: Correct it. The Party chunk size (`PARTY_CHUNK_BYTES` = 256 KiB) belongs in protocol.md, not only platform_spec.
- Effort: S.

### DOC-17 — `CLAUDE.md` says unaccepted files do not reach the download directory
- Severity: Medium
- Category: wrong
- Status: VERIFIED
- Location:
  - Docs: `CLAUDE.md:140` ("nothing reaches the download dir … until accept") vs `CLAUDE.md:228` ("the spool lives *in* the download dir")
  - Code: `client/src/app/chat_manager/events.rs:628` (`IncomingFileSync::new(&self.config.download_dir, …)`); `core/src/transfer/receiver.rs:135` (`tmp_<uuid>_<sanitized peer name>`)
- Problem: The doc contradicts itself. Up to 64 MiB per conversation of unaccepted, peer-named temp files do sit in the user's Downloads folder. That is user-visible and matters to a privacy or security reader.
- Recommendation: Say "the spool is a `tmp_`-prefixed file in the download dir; it is renamed into place only on accept". Mention it in the user guide too.
- Effort: S.

### DOC-18 — `server_tiers.md` lists moderation powers that do not exist
- Severity: Medium
- Category: wrong
- Status: VERIFIED
- Location:
  - Docs: `docs/server_tiers.md:159-162`, `:164-167`
  - Code: `PartyRequest` (`core/src/party/mod.rs`) has `DeleteChannel`, `DeleteFile`, `SetRole` and `SetChannelAccess`, but no message deletion and no member removal, kick or ban
- Problem: "Kept at every tier: … the ability to delete a message" and "An admin can still remove a member … or delete a message" describe current operator capabilities that do not exist. Readers will assume moderation tools exist today.
- Recommendation: Mark these as planned, or list only the real ones (delete channel, delete file, demote to Guest).
- Effort: S.

### DOC-19 — No single local command set matches CI; CONTRIBUTING, DEVELOPER_GUIDE, the PR template and README disagree
- Severity: Medium
- Category: duplicated
- Status: VERIFIED
- Location:
  - `CONTRIBUTING.md:19-35`
  - `DEVELOPER_GUIDE.md:71` ("`cargo nextest run --workspace` (or `cargo test --workspace`)"), `:139-141`
  - `.github/PULL_REQUEST_TEMPLATE.md:9-13` (no doctests; `cargo check -p p2pem-desktop` + `npm run build` only; no lint, `npm test` or smoke)
  - `README.md:90` (`cargo test --workspace`)
  - CI: `.github/workflows/ci.yml:208-225, 388-393, 431-583, 803`
- Problem:
  - CI runs `nextest --profile ci`, the fuzz `cargo check`, `cargo deny`, `npm audit`, the dist-staleness check and `smoke:install` + `smoke`. No doc gives that set.
  - The `cargo test --workspace` alternative runs tests in one process. CLAUDE.md says the suite is written for process-per-test (relay tests read a process-global env var), so the alternative can be flaky.
  - None of them says `--workspace` needs the GTK/WebKit dev packages.
- Recommendation: Add `scripts/ci-local.sh`. Make CONTRIBUTING, the PR template and README point to it. Drop the `cargo test --workspace` alternative.
- Effort: S.

### DOC-20 — `platform_spec.md` "Current State" was verified at 1.15.0 and contradicts itself
- Severity: Medium
- Category: stale
- Status: VERIFIED
- Location:
  - Version: `docs/platform_spec.md:44-45` (1.15.0, workspace is 1.16.2)
  - `:338` "per-file permission matrix … remains" vs `:399-414` "Per-file permissions (shipped)"
  - `:1020-1022` "store-and-forward … is the interesting design problem" vs `:717-724` "Asynchronous P2P delivery is deliberately not pursued"
  - `:86-89` and `:255` say files are desktop-only, a "trio" of requests; the TUI has `:party-send-file`/`:party-download`
  - `:288-293` data model has no `role`
  - Rail paragraph repeated at `:133-135` and `:152-156`
  - `:400` uses a reference-style `[FilePermissions]` link with no definition (renders literally)
  - `:850-851` "promoting a compared SAS to a persisted verified state" listed as open, though `promote_contact_verified` exists (`client/src/app/chat_manager/contacts.rs:278`)
- Problem: The forward-looking doc restates present state and gets it wrong. Its own rule is that architecture.md owns "today".
- Recommendation: Delete §2 and the "What is built" parts and link to architecture.md and protocol.md. Resolve the two contradictions.
- Effort: M.

### DOC-21 — `CLAUDE.md` is 56 KB and breaks its own size rule
- Severity: Medium
- Category: structure
- Status: VERIFIED
- Location:
  - `CLAUDE.md` (309 lines, many over 1,000 characters); rule at `:308`
- Problem:
  - Roughly 14k tokens are loaded into every agent session.
  - About 60 % is design rationale ("Non-obvious gotchas", Party server internals, fuzzing history) that contributors also need but can only find here. Examples: `scripts/fuzz.sh` and the CI gating rules exist only in CLAUDE.md.
  - Several entries have already drifted (DOC-15, 16, 17, 22, 23).
- Recommendation: Keep commands, layout and a bullet list of footguns, each with a one-line rule and a link. Move the rationale into architecture.md ("Design invariants"), protocol.md and SECURITY.md.
- Effort: M.

### DOC-22 — `CLAUDE.md` tool rules give commands that do not work, and its layout note is stale
- Severity: Low
- Category: wrong
- Status: VERIFIED
- Location:
  - `CLAUDE.md:299` (`rg "pattern" --include "*.rs"`; ripgrep has no `--include`, use `-g '*.rs'` or `-t rust`)
  - `:300` (`fd "*.rs"`; fd takes a regex, so this is an invalid-regex error; use `fd -e rs`; also `fd` is not installed in this environment)
  - `:293` heading "Preferred Tools (opencode)" names a different agent
  - `:7` "root `src/` and `tests/` are vestigial — ignore them": neither directory exists
- Recommendation: Fix the commands and remove the vestigial-directory note.
- Effort: S.

### DOC-23 — `CLAUDE.md` Tauri bridge section names the wrong file and lists about half the commands
- Severity: Low
- Category: stale
- Status: VERIFIED
- Location:
  - Docs: `CLAUDE.md:110-114`
  - Code: `desktop/src-tauri/src/lib.rs:252-320` registers 67 commands, implemented in `desktop/src-tauri/src/commands/{auth,chats,connect,contacts,party}.rs`
- Problem:
  - The heading says the commands are in `lib.rs`.
  - The list leaves out among others: `set_display_name`, `export_identity`, `export_diagnostics`, `open_data_dir`, `lock_state`/`set_locked`, `mark_read`, `set_presence`, `list_transfers`, `accept_`/`decline_`/`cancel_transfer`, `get_`/`update_settings`, `pick_download_dir`, `open_url`, `remove_`/`block_`/`unblock_contact`, `list_discovered_peers`, `my_addresses`, and 15 `party_*` commands.
- Recommendation: Point to `invoke_handler()` as the list instead of copying it.
- Effort: S.

### DOC-24 — `design/tokens.json` points to the deleted egui module
- Severity: Low
- Category: stale
- Status: VERIFIED
- Location:
  - Docs: `design/tokens.json:2`
  - Code: `client/src/tui/overlays.rs:825` (`mod token_drift_tests`); `desktop/src/lib/tokens.test.js`
- Problem: The comment cites `client/src/gui/styling.rs` and "egui's Visuals". The guards that actually exist are the two above (as DESIGN_NOTES correctly says).
- Recommendation: Rewrite the `$comment`.
- Effort: S.

### DOC-25 — `desktop/src-tauri/Cargo.toml` tech-debt note is stale
- Severity: Low
- Category: stale
- Status: VERIFIED
- Location: `desktop/src-tauri/Cargo.toml:26-29`
- Problem: It says depending on `p2pem-classic` "pulls in egui/ratatui transitively … compile the legacy GUIs". egui is gone. ratatui is still pulled in, because the client crate contains the TUI.
- Recommendation: Reword to "pulls in ratatui; extracting the app core into its own crate would drop it", or do the split.
- Effort: S.

### DOC-26 — Backup and restore is incomplete, and migrating from Classic is undocumented
- Severity: Medium
- Category: missing
- Status: VERIFIED
- Location:
  - Docs: `README.md:63-71`; `docs/TUTORIAL.md:44-51`; `docs/USER_GUIDE.md:24-28`
  - Code: `desktop/src/components/Onboarding.jsx:48-51` (restore is "copy it over the file above"); no import or restore command exists
- Problem:
  - The docs push users to back up their identity but never explain how to **restore**, or where the file goes.
  - The backup covers `identity.json` only, so chat history (`history.json.enc`) is not backed up. A lost disk loses history even with an identity backup.
  - USER_GUIDE tells Classic users to "export your old identity" but not how to put it into the desktop app's `P2PEM` directory.
  - TUI `:backup` is undocumented.
- Recommendation: Write a backup/restore/migrate section: paths per OS, which files, and in what order.
- Effort: S.

### DOC-27 — No operator guide for the community server or relay
- Severity: Medium
- Category: missing
- Status: VERIFIED
- Location:
  - Docs: `docs/USER_GUIDE.md:212-252` and `:77-88` are the only coverage
  - Code: `server/src/main.rs`; `core/src/network/relay.rs:35-46`
- Problem: Nothing covers:
  - Running as a service, or the firewall.
  - That `--password` on the command line is visible in `ps`; use `PARTY_PASSWORD` instead.
  - That the default port (12345) clashes with the P2P default on the same host.
  - Backing up the server key (`party-data/`): losing it changes the fingerprint everyone pinned.
  - That upgrades must be in lockstep (`docs/protocol.md:466-471`).
  - Storage limits (1 GiB server-wide, 128 MiB per member, 128 channels, 5 per member), the owner bootstrap ("first to join is Owner"), the audit log, or relay timeouts and rate limits.
- Recommendation: Create `docs/operators/community-server.md` and `docs/operators/relay.md`.
- Effort: M.

### DOC-28 — No privacy / metadata page
- Severity: Medium
- Category: missing
- Status: VERIFIED
- Location: the material is scattered across `SECURITY.md:128-142`, `THREAT_MODEL.md:55-72`, `docs/USER_GUIDE.md:175-176`, `docs/server_tiers.md:171-172`
- Problem: There is no single user-facing answer to "who learns what":
  - Peers see LAN and public IPs (punching, multi-address invites).
  - A relay sees both endpoints and timing.
  - mDNS reveals the device name.
  - The UPnP gateway is involved.
  - The community operator reads all content, DMs included.
  - Diagnostics bundle contents are not described.
  - There is no privacy policy (none is needed with no hosted service, but say so).
- Recommendation: Add `docs/user/data-and-privacy.md`.
- Effort: M.

### DOC-29 — THREAT_MODEL is missing major attack surfaces
- Severity: Medium
- Category: missing
- Status: VERIFIED
- Location: `THREAT_MODEL.md:21-116`
- Problem: Implemented, security-relevant controls appear in CLAUDE.md and the CHANGELOG but not in the threat model:
  - **Received files**: path traversal, bidi and Windows-device filenames, executable-open gating (`OpenOutcome`), the 64 MiB unaccepted-spool cap.
  - **Community join credentials**: the two-step trust and constant-time password compare.
  - **UPnP gateway trust** (covered only in AUDITS).
  - **Relay server DoS**: hello timeout, pending-rendezvous cap.
  - **Hostile community server** (CHANGELOG 1–2: memory exhaustion, planted messages).
  - "Malicious peer" lists only "trust verification, rate limiting, session validation".
  - Also: `:3` uses the old project name.
- Recommendation: Add these surfaces with their controls and residual risks.
- Effort: M.

### DOC-30 — `SECURITY.md` contradicts itself on Ed25519
- Severity: Low
- Category: clarity
- Status: VERIFIED
- Location: `SECURITY.md:103`, `:143` (Ed25519 negotiated) vs `:161-166` ("Migrating identity signatures to Ed25519 … would drop `rsa`") and `:201` ("does not claim Ed25519 identity-key support")
- Problem: All three are technically consistent: proofs use an Ed25519 *subkey*, while the identity and binding stay RSA. A reader can't tell.
- Recommendation: One sentence: "Proof signatures use an Ed25519 subkey bound by an RSA-PSS signature; the identity, fingerprint and binding remain RSA-2048, so `rsa` stays on the path until the fingerprint cutover (platform_spec §12 step 4)."
- Effort: S.

### DOC-31 — USER_GUIDE leaves out user-visible desktop features and safety behaviour
- Severity: Low
- Category: missing
- Status: VERIFIED
- Location:
  - Docs: `docs/USER_GUIDE.md:296-319`
  - Code: `desktop/src-tauri/src/lib.rs:252-320`; `desktop/src/components/Settings.jsx:231-261`
- Problem: Undocumented:
  - The file acceptance gate (default off, accept/decline in the conversation).
  - The warning before opening a received executable.
  - Contact block and unblock.
  - The connection password.
  - Community roles, channel kinds, the Drive tab, per-file permissions, audit log and leaving a community.
  - Display name, change password, the "Reconnect contacts on startup" toggle.
- Recommendation: Add short sections.
- Effort: M.

### DOC-32 — Smaller USER_GUIDE inaccuracies
- Severity: Low
- Category: stale
- Status: VERIFIED
- Location: `docs/USER_GUIDE.md`
- Problem:
  - `:478` "inspect the log terminal" is an egui-era feature. Today's equivalents are TUI `Ctrl+L`/`:logs` and desktop Export diagnostics.
  - `:479` mentions chunking compatibility with "much older" peers, but v3 is the minimum anyway.
  - `:15-18` lists `.msi`, but `tauri.conf.json:30` also builds `rpm`, which is mentioned nowhere.
  - `README.md:17-22` omits `.msi`, while `:103` mentions it.
- Recommendation: Fix these, and make the installer list match `release.yml:96-102`.
- Effort: S.

### DOC-33 — DESIGN_NOTES "Current Gaps" is stale
- Severity: Low
- Category: stale
- Status: VERIFIED
- Location:
  - Docs: `DESIGN_NOTES.md:249`
  - Code: cancellation and progress shipped (`docs/platform_spec.md:899-907`; `cancel_transfer` command)
- Problem: The gap list still says file-transfer progress and cancellation are limited.
- Recommendation: Remove the item, or narrow it.
- Effort: S.

### DOC-34 — Superseded entries in AUDITS.md are not marked
- Severity: Low
- Category: stale
- Status: VERIFIED
- Location: `docs/AUDITS.md:81-85` (`block_in_place`, now `spawn_blocking` off-lock: `server/src/connection.rs:53-74`), `:157` ("21 advisories"; deny.toml now has 9), `:62`
- Problem:
  - Historical entries describe fixes that have since been replaced, with no note saying so.
  - GHSA-6q3g-734c-22jm, a published advisory, is mentioned only in CLAUDE.md.
- Recommendation: Add "superseded by …" notes. Add a published-advisories table to SECURITY.md or AUDITS.md.
- Effort: S.

### DOC-35 — Bug-report template is out of date
- Severity: Low
- Category: stale
- Status: VERIFIED
- Location: `.github/ISSUE_TEMPLATE/bug_report.yml:25` ("Start the GUI"), `:47` (placeholder `v1.8.1`)
- Problem: There is no field for which component is affected (Desktop, TUI, server, relay). That matters because the desktop app and TUI keep separate identities.
- Recommendation: Add a component dropdown, and point to the diagnostics export.
- Effort: S.

### DOC-36 — CHANGELOG: security fixes have sat unreleased for 2.5 months
- Severity: Low
- Category: structure
- Status: VERIFIED
- Location:
  - `CHANGELOG.md:12-202` (`[Unreleased]` since 1.16.2 on 2026-08-20, about 1,900 words, about 12 Security entries)
  - Formatting: empty line inside `### Security` at `:26`; versions 1.10.0, 1.10.1, 1.8.x and earlier have bracketed headings with no link definitions (`:1384-1395`)
- Problem: `SECURITY.md:16-21` supports only "the latest release" and has no auto-updater, so users stay exposed to fixed issues. That is a release-process issue the docs do not address.
- Recommendation: Add a release cadence and security-release policy to DEVELOPER_GUIDE. Drop the brackets on untagged versions.
- Effort: S.

### DOC-37 — Contributor docs miss per-OS setup, fuzzing and CodeQL
- Severity: Low
- Category: missing
- Status: VERIFIED
- Location:
  - `DEVELOPER_GUIDE.md`, `CONTRIBUTING.md`: no `scripts/fuzz.sh` and no `run-codeql-local.ps1` (the latter is documented nowhere except `CHANGELOG.md:1190`)
  - The Linux GTK/WebKit packages are listed only in `docs/USER_GUIDE.md:46-53`
  - No macOS or Windows prerequisites, no nextest install, no Node version anywhere but README
- Recommendation: Add a "Setup per OS" and "Fuzzing / static analysis" section to DEVELOPER_GUIDE.
- Effort: S.

### DOC-38 — The project is called five different things
- Severity: Info
- Category: clarity
- Status: VERIFIED
- Location:
  - "Encrypted P2P Messenger": `THREAT_MODEL.md:3`, `docs/platform_spec.md:1`
  - "Encrypted Messenger": `core/src/lib.rs:1`, `server/src/main.rs:1`, `server/Cargo.toml:3`, server default name `"Encrypted Messenger Party"`
  - "secure-p2p-chat": `design/tokens.json:2`
  - "P2PEM": README
- Recommendation: Use "P2PEM" in prose. Keep the historical identifiers only in the docs/README naming table.
- Effort: S.

### DOC-39 — Public constants in `core` have no rustdoc
- Severity: Info
- Category: missing
- Status: VERIFIED (spot check)
- Location: `core/src/lib.rs:30-68`
- Problem: 11 of 18 `pub` items, mostly constants (`PORT_DEFAULT`, `MAX_PACKET_SIZE`, `AES_*`, `RSA_KEY_BITS`…), use trailing `//` comments rather than `///`. No crate enables `#![warn(missing_docs)]`. Module-level `//!` docs elsewhere (crypto, session, identity, util, types) are thorough.
- Recommendation: Convert the comments to `///`, and consider `missing_docs` on `messenger-core`.
- Effort: S.

### DOC-40 — DEVELOPER_GUIDE uses `cargo tauri dev` where every other doc uses `npx tauri dev`
- Severity: Info
- Category: clarity
- Status: VERIFIED
- Location: `DEVELOPER_GUIDE.md:64`; `desktop/src-tauri/src/lib.rs:386`
- Problem: The extra-peer example needs the `cargo-tauri` CLI, which no setup step installs.
- Recommendation: Use `npx tauri dev`.
- Effort: S.

### Checked and not reproduced
- **TUI `:password` command documented.** No doc in the current tree mentions it. The real commands are `:unlock` and `:set-password`, and USER_GUIDE lists those correctly.
- **CLAUDE.md states a 64 KiB Party upload chunk.** The current CLAUDE.md does not say this. Its 64 KiB figures refer to `FILE_CHUNK_SIZE` (P2P) and framing reads, both correct. The real gap is that `PARTY_CHUNK_BYTES` (256 KiB) is documented only in platform_spec (see DOC-04 and DOC-16).
- **Broken links or anchors.** None. All 71 relative links and 9 anchors resolve, and the image exists.

## Things done well
- README states its limitations plainly: no mobile client, no offline P2P delivery, self-hosting required, no third-party audit, no password reset.
- SECURITY.md's download-verification section is excellent. It explains `gh attestation verify` with `--signer-workflow` and why the flag matters, explains the limits of checksums, and admits there is no auto-updater. It matches `release.yml` (SHA256SUMS is generated and attested).
- protocol.md is precise on the handshake order, SAS derivation, rekey (host-only initiator, dual-key window, no PCS), invite payload versions, the relay punch hello format and Ed25519 subkey binding. Spot checks against `session.rs`, `protocol.rs` and `crypto.rs` all matched: `REKEY_TIME_INTERVAL` 300 s, `REKEY_MESSAGE_COUNT` 100, u32 version, `ProtocolMessage` variant list.
- The constants in CLAUDE.md, DEVELOPER_GUIDE and protocol.md match `core/src/lib.rs`, `session.rs`, `server/src/state/mod.rs`, `hub.rs` and `dispatch.rs` exactly.
- Server CLI docs (`--name`, `--port`, `--password`, `--data-dir`, env vars, defaults) match `server/src/main.rs`. Client CLI flags match `client/src/main.rs`, and the retired `--gui` fails with a helpful pointer.
- docs/README.md has a clear ownership table and a naming table that explains the historical identifiers.
- server_tiers.md is a model design doc: marked "not implemented" up front, explicit about what each tier hides and does not hide, with MUST-level requirements and open questions.
- `core/fuzz/seeds/README.md` and the `command.rs` docs (help and usage errors derived from one table) show good single-source discipline that USER_GUIDE could copy.
- The CHANGELOG is written for users ("what changed for you"), follows Keep a Changelog, and has compare links for tagged versions.
- Security reporting goes through GitHub private advisories, and the issue-template config disables blank issues and links to SECURITY.md.
