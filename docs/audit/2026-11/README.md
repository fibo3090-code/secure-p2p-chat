# Full-codebase audit — November 2026

A whole-repository audit of P2PEM at `main` 62b952f (v1.16.2, after PR #188),
run on 2026-11-02 → 2026-11-03. Thirteen auditors each read one area of the
codebase in full and wrote a standalone report; a lead pass deduplicated the
results, re-checked every Critical and the most consequential High findings
against the code (two of them by running code), and produced the prioritised
plan below. Two independent review threads run elsewhere were folded in
([reports/14-external-review.md](reports/14-external-review.md)).

**Start here, in this order:** this page (what to fix, in what order) →
[lead-verification.md](lead-verification.md) (what was re-checked and how) →
the area report for whatever you pick up → [findings-index.md](findings-index.md)
(all 348 findings in one sortable table).

## At a glance

| | Count |
|---|---|
| Area reports | 13 (+1 external) |
| Findings | 348 (3 Critical, 43 High, 126 Medium, 143 Low, 33 Info) |
| Status | ~95 % VERIFIED by tracing the code; the rest SUSPECTED and labelled |
| Already fixed | CRYPTO-01 and QUAL-01 (see §1) |
| Not covered | No dynamic testing of installers, native dialogs, real NATs, or Windows/macOS behaviour; nothing here proves the absence of other defects |

The numbers overcount: the same root cause is often reported by several
auditors from different sides (the community first-join flaw appears five
times). The prioritised list below is deduplicated.

**Overall picture.** The cryptographic core is carefully built and heavily
commented, and most past bug classes are pinned by tests. The serious problems
cluster in four places: (1) *trust bootstrap* — places where what the user
verifies is not what the code then trusts; (2) *data durability* — failure
paths that silently destroy the user's history; (3) *the community server*,
which has no resource limits and can be made permanently unusable by one
member; and (4) *honesty of the product surface* — docs and UI that promise
end-to-end encryption, data deletion and backups that do not exist.

## Reports

| # | Area | Findings (C/H) | Report |
|---|---|---|---|
| 01 | Cryptography, protocol, identity | 18 (1/1) | [01-crypto-protocol.md](reports/01-crypto-protocol.md) |
| 02 | Network layer (session, relay, punch, mDNS) | 22 (0/2) | [02-network.md](reports/02-network.md) |
| 03 | Community ("Party") server | 24 (1/5) | [03-party-server.md](reports/03-party-server.md) |
| 04 | Client app core (ChatManager, transfers, persistence) | 24 (0/1) | [04-client-core.md](reports/04-client-core.md) |
| 05 | Terminal client (TUI) | 37 (0/8) | [05-tui.md](reports/05-tui.md) |
| 06 | Tauri desktop shell (Rust bridge, config) | 34 (0/2) | [06-tauri-bridge.md](reports/06-tauri-bridge.md) |
| 07 | React frontend | 37 (0/2) | [07-react-frontend.md](reports/07-react-frontend.md) |
| 08 | User journey: download → use → delete, disk/network footprint | 23 (1/4) | [08-user-journey-footprint.md](reports/08-user-journey-footprint.md) |
| 09 | Documentation | 40 (0/7) | [09-documentation.md](reports/09-documentation.md) |
| 10 | Developer experience, CI, tests (measured) | 17 (0/3) | [10-dx-ci-tests.md](reports/10-dx-ci-tests.md) |
| 11 | Performance and resources | 23 (0/5) | [11-performance.md](reports/11-performance.md) |
| 12 | Code quality and best practices | 29 (0/2) | [12-code-quality.md](reports/12-code-quality.md) |
| 13 | Privacy, metadata, supply chain | 22 (0/1) | [13-privacy-supply-chain.md](reports/13-privacy-supply-chain.md) |
| 14 | External review threads (lifecycle, backup, docs, CI) | 14 | [14-external-review.md](reports/14-external-review.md) |

Each report has the same shape: scope read, summary, findings (severity,
category, VERIFIED/SUSPECTED, file:line, failure scenario, concrete fix,
effort), what is done well, open questions.

## 1. Fix first (P0) — deduplicated

Ordered by harm × likelihood. "Seen in" lists every report that found it.

| # | Problem | Seen in | Effort |
|---|---|---|---|
| P0-1 | **Identity spoofing via a two-key PEM.** The fingerprint was read from the first `BEGIN` block in the text while signatures were verified against the key the PEM parser returned; a preamble could carry a victim's key. Anyone holding a contact's public key (it is in every invite) could impersonate them with no SAS prompt, take over a community Owner seat, or forge a "signed" invite that redirects a verified contact. **Confirmed by running code. Fixed** on branch `claude/code-quality-pass`: fingerprints are computed from the parsed key, the PEM parser refuses anything but one block, regression tests on every path. **Still to do: a GitHub security advisory and a release** — every shipped version is affected. | CRYPTO-01 | done / S |
| P0-2 | **A crafted v1 invite link crashes the app** (slice panic under `panic = "abort"`). Same root cause, fixed with P0-1. | QUAL-01 | done |
| P0-3 | **An unreadable history file is replaced with an empty one** right after unlock — every conversation, contact, trust state and setting, silently and atomically. Triggered by a downgrade, an unknown enum variant, a OneDrive placeholder, bit-rot or a mismatched restored identity. Fix: move the unreadable file aside (never delete), set a sticky "history load failed" flag that makes `history_snapshot()` refuse, surface it in `auth_status`, test that a failed load never writes. | JOURNEY-01, TAURI-30, APP-01, QUAL-02 | M |
| P0-4 | **Community first-join verification protects nothing.** The SAS shown belongs to a connection that is then closed; the second connection — the one that sends username and password — accepts any key; the server never displays a code to compare; the TUI does not even keep the fingerprint. Fix: return the fingerprint from `NeedsVerification`, pass it back as `expected_fingerprint` on the trust call (desktop: new single-word `fingerprint` arg; TUI: keep it in `PendingPartyJoin`); show the server *fingerprint* (which the operator can read from the server log) instead of a per-connection SAS. | SRV-05, TAURI-01, TUI-02, UI-01, DOC-01 | S |
| P0-5 | **Frames can be reflected back to their sender** and are accepted as the peer's (one key and one AAD for both directions; the nonce role bit is never checked on decrypt). Any on-path party — the bridging relay always is — can echo your own messages back as the peer's, forge delivery receipts, or kill the session by echoing the host's `Rekey`. Fix: reject frames carrying our own role bit (cheap, check wire compat with old peers), then directional keys in protocol v4. | NET-01, CRYPTO-03 | S → M |
| P0-6 | **SAS can be ground by a MITM**: no commitment to the ephemeral key, so an attacker searches its last ephemeral until both SAS values match. Needs a protocol change (commit-then-reveal, as ZRTP/Signal-style SAS do). Ship together with directional keys and signed scheme negotiation (CRYPTO-07) as protocol v4. | CRYPTO-02 | M |
| P0-7 | **One member can make a community permanently unjoinable**: history pages are capped by count (200) not bytes, so 200 × 64 KiB exceeds the 8 MiB frame and the connection drops on every join; the unvalidated `mime` field can do it with one message. Survives restarts. Fix: page by bytes, validate `mime`/`name`, page `ListFiles`/`ListMembers`. | SRV-01 | M |
| P0-8 | **A message can go to the wrong conversation** in the desktop app: a late `getConversation` response shows A while the composer sends to B. Fix: drop responses that no longer match the active id; send to the id actually displayed. | UI-02 | S |
| P0-9 | **TUI accepts an unverified peer by accident**: the fingerprint overlay opens under the user's fingers and `Enter`/`y` accepts. Fix: require an explicit, non-default confirmation (type the SAS or a distinct key), never Enter. | TUI-01 | S |
| P0-10 | **TUI paste executes commands**: bracketed paste is off, so a pasted `:send ~/.ssh/id_rsa` line runs. Fix: enable bracketed paste, treat pasted text as text. | TUI-05 | S |
| P0-11 | **TUI first run: Esc on "set password" means the identity is never saved** — every launch is a new peer and old history is overwritten. Fix: make the prompt non-dismissable (CLAUDE.md already says the UI must block until set-password), or persist encrypted immediately. | TUI-04, JOURNEY-02 | S |
| P0-12 | **The product says "end-to-end encrypted, no server copy", but community servers read everything** (channel messages, DMs and files in plaintext in `party.db`, world-readable). The documented "operator can read messages" badge does not exist. Fix: show the trust tier in the community UI, correct README/Settings copy, 0600 the server data. | JOURNEY-04, PRIV-01, DOC-03, SRV-11 | S |

## 2. Fix next (P1)

**Community server robustness** (SRV-02, SRV-03, SRV-04, SRV-06, SRV-07, SRV-08):
evicted slow clients keep a deaf socket; no idle/pre-join/write timeouts and
no connection cap (pre-auth FD exhaustion); the first identity to connect to
an open server becomes Owner forever; no per-member rate limits, all history
in RAM forever; upload spools in RAM outside quotas; guests can DM; no
kick/ban/delete although `server_tiers.md` describes them.

**P2P host availability** (NET-02, NET-05, NET-10): a single silent TCP
connection makes the one-shot host unreachable, repeatedly; anyone can create
TOFU prompts and persisted chats at ~30/min/IP; confirming the SAS more than
5 minutes apart kills the session.

**Hosting lifecycle** (EXT-12, EXT-13, EXT-14, EXT-11 — confirmed in
[lead-verification.md](lead-verification.md)): deleting the host placeholder
re-hosts within 1.5 s; lock → unlock never resumes hosting; `clear_history`
keeps hosting state; a self-connection is not refused and creates two odd
conversations.

**Data you can't get back or can't remove** (JOURNEY-03, DOC-02, TAURI-31,
TAURI-32, TAURI-33, TAURI-34, JOURNEY-06): no in-app "delete everything" and
uninstall leaves identity and history behind (the user guide claims both
exist); the identity backup does not include history/contacts and the UI never
says so; no restore path; exporting onto the live `identity.json` truncates it
to 0 bytes; the "backed up" flag survives a password change.

**Password and credential handling** (TUI-03, TAURI-02, TAURI-03,
CRYPTO-05): `set_password` on an unlocked identity skips the current-password
check in both shells; the host's connection password is sent to any dialled
peer that asks, before TOFU.

**File transfer failure paths** (APP-02…APP-07, TAURI-06): a stalled peer
freezes the desktop UI (bounded lane awaited under the manager lock); failed
finalize leaves `tmp_` spools in Downloads; disk errors don't send
`FileCancel`; `FileCancel` has no transfer id; received files get no
Mark-of-the-Web and the risky-extension list misses `.iso`, `.vhd`, HTML,
macro documents…

**UI blocking on network I/O** (TAURI-04, APP-10, TUI-08): community connect
has no TCP timeout and holds the party lock (desktop) or the UI loop (TUI) for
up to ~2 minutes.

**Release pipeline** (JOURNEY-05, DX-11, SUPPLY-02 — SUSPECTED, confirm with a
dry run on a test tag): `tauri-action` is given `tagName`/`releaseName` under a
read-only token, so desktop builds either fail or bypass the
attest-then-publish design.

**Rekey robustness** (CRYPTO-06, NET-03, EXT-01): only one previous key is
kept, so two host rekeys during crossing bulk transfers desync the session; a
client may initiate a rekey the protocol reserves to the host.

## 3. Performance (top items from [11-performance.md](reports/11-performance.md))

1. Desktop refresh re-serializes the whole open conversation and re-formats every timestamp in JS on every `state-updated` — 4 Hz during any transfer (PERF-01, TAURI-09, UI-06). Send deltas or a windowed slice.
2. One 24 MiB peer message is re-processed on every frame/refresh/save (PERF-02). Store large texts out of line; render a preview.
3. Whole-history persistence: clone + serialize + encrypt + fsync of everything; the TUI does it every 30 s unconditionally (PERF-03). Per-conversation segments or an append log.
4. Server SQLite: autocommit, `journal_mode=DELETE`, `synchronous=FULL`, no WAL, no statement cache, all under the global lock (PERF-04, SRV-12). WAL + `synchronous=NORMAL` + batched transactions is an S-sized change.
5. Client community history merge is O(n²) (PERF-05); server paging scans from message 1 (PERF-06).
6. No receive-side backpressure for P2P file chunks (unbounded channel) (PERF-07).
7. No `TCP_NODELAY` and a separate 4-byte header write: up to 40 ms (Linux) / 200 ms (Windows) per small frame (NET-04, PERF-10).

## 4. Developer experience and CI (measured — [10-dx-ci-tests.md](reports/10-dx-ci-tests.md))

- **DX-01 — biggest single win.** 95 % of test time is unoptimised RSA keygen and Argon2. Compiling eight crypto crates at `opt-level = 3` in the dev/test profile took the core unit tests from **133 s to 8 s** (16×). A `[profile.dev.package.*]` block, no code change.
- **DX-03.** `desktop` builds a staticlib and a cdylib it never uses (~3.6 GB per build+test); one debug build + test build is 17–18 GB, which is how `target/` reached 30 GB and filled the disk during this audit. Drop the unused crate types; `debug = "line-tables-only"` for deps.
- **DX-02.** The CI change classifier skips the frontend contract tests when only `desktop/src-tauri/` changes — exactly the arg-name footgun those tests exist to catch.
- **DX-05 / SUPPLY-01.** New RustSec advisories against already-pinned deps turn every unrelated Rust PR red (it happened during this audit), while frontend-only PRs merge green. Keep advisories hard on `main` and the schedule, not on PRs that did not change the lockfile; let the daily audit open the `cargo update` PR.
- **DX-07, DX-06, DX-08, DX-13.** Frontend suite runs in full on three OSes; `build-verification` recompiles what the Linux test leg already built; ~12 rust-cache entries; a 20-line apt block duplicated five times and drifting.
- **DX-09.** No single local command equals CI; four documents give four different lists. Add one script (`scripts/ci-local.sh`) and point every doc at it.
- Measured baseline: clean `cargo build --workspace` 5 min (4 cores); `cargo test --workspace --no-run` +3 m 54 s; sequential test run 10 m 48 s, 664 passed; vitest 27 s.

## 5. Documentation ([09-documentation.md](reports/09-documentation.md))

Seven High: verification instructions that cannot work (DOC-01), a
"Delete Everything" feature and uninstaller wipe that do not exist (DOC-02), a
THREAT_MODEL control that is not built (DOC-03), `protocol.md` missing nine
shipped community message variants and claiming chunked transfer is
unimplemented (DOC-04), a wrong server binary path (DOC-05), user docs that
never mention the six-digit code both UIs lead with (DOC-06), and THREAT_MODEL
saying invite expiry is unenforced when it is (DOC-07). Also: CLAUDE.md is
56 KB against its own "keep this short" rule and carries several claims the
code contradicts (relay fallback, upload ids, spool location); stale notes in
`desktop/src-tauri/Cargo.toml` and `design/tokens.json`; `platform_spec.md`
"current state" pinned at 1.15.0. The report proposes a target structure by
audience (users / operators / contributors / security reviewers / agents) and
lists the missing guides: community server operator guide, privacy/metadata
page, backup & restore, uninstall/data removal.

## 6. Privacy and supply chain ([13-privacy-supply-chain.md](reports/13-privacy-supply-chain.md))

The report opens with a metadata-exposure matrix (peer, relay, community
operator, LAN, network, local disk, OS). Key items: one long-term identity key
authenticates the user to every community (cross-community linkability,
PRIV-02); hole punching discloses public and LAN addresses before verification
(PRIV-03); the TUI diagnostics bundle captures relay tokens, IPs and paths
under a README that reads as safe to share (PRIV-04); the desktop logs every
frame at debug level to stdout (PRIV-05, NET-19, TAURI-16); notifications show
message text with no "hide content" option (PRIV-06). Supply chain is in good
shape (SHA-pinned actions, attest-then-publish design, stale-advisory checker,
no committed secrets); gaps are Dependabot not covering Actions or the fuzz
workspace, an unverified gitleaks download in CI, and an unreviewable
committed `desktop/dist`.

## 7. Code quality ([12-code-quality.md](reports/12-code-quality.md))

The report ends with a 12-step refactoring roadmap. Headlines: a
`SessionParams`/`PeerRoute` struct to retire eight `too_many_arguments`
session runners (QUAL-03); typed errors at module boundaries instead of
`Result<_, String>` (59 `map_err(|e| e.to_string())` in the desktop commands,
error classification by matching English text) (QUAL-04); server persistence
errors logged and ignored, so clients get `MessagePosted` for writes that
failed (QUAL-05); the desktop links the whole TUI crate (QUAL-08); 36 public
functions with no production caller, including one that exports a private key
as a plain `String` (QUAL-09); a dead async file receiver that has already
drifted (QUAL-10); no `[workspace.lints]` (QUAL-27).

## 8. Suggested roadmap

| Phase | Content | Rough size |
|---|---|---|
| 0 — now | Advisory + release for P0-1/P0-2 (already fixed in code). | release work |
| 1 — data safety | P0-3 (history), TAURI-31 (export onto source), JOURNEY-03/DOC-02 (delete-everything, or remove the claims), backup scope disclosed in UI. | ~2 days |
| 2 — trust bootstrap | P0-4 (community pin), P0-9/P0-10/P0-11 (TUI), P0-8 (misdirected send), P0-12 (operator-reads badge + copy), EXT-11 (refuse self-connection). | ~2 days |
| 3 — protocol v4 | P0-5 (role bit now, directional keys in v4), P0-6 (SAS commitment), CRYPTO-07 (sign the negotiation), rekey robustness (CRYPTO-06/NET-03/EXT-01). Version-gated, with interop tests against v3. | ~1 week |
| 4 — community server | P0-7, then SRV-02/03/04/06/07/08, SQLite pragmas and transactions (PERF-04, SRV-10/12), retention. | ~1 week |
| 5 — DX quick wins | DX-01 (opt-level for crypto deps), DX-03, DX-02, DX-05/SUPPLY-01, DX-09 single local CI script. | ~1 day |
| 6 — performance | PERF-01…PERF-07. | ~1 week |
| 7 — docs | Correct the seven High doc findings, then restructure by audience; shrink CLAUDE.md. | ~2 days |
| 8 — quality | The code-quality roadmap (QUAL-03/04/05/08/09/10/27), TUI split leftovers. | ongoing |

## Method and limits

- Static reading of every file in each area, by thirteen auditors in parallel
  (at most three at a time), each producing file:line evidence. The DX
  auditor alone ran builds and tests to measure.
- Lead verification: every Critical and the high-impact High findings were
  re-traced; CRYPTO-01 was proven by a throwaway test (crafted PEM →
  victim's fingerprint, attacker's key), and the TAURI-31 truncation by a
  standalone `std::fs::copy(p, p)` check. See [lead-verification.md](lead-verification.md).
- Severity is the auditor's, adjusted by the lead where two reports disagreed
  (noted in lead-verification.md). Line numbers refer to 62b952f.
- Not done: running installers or uninstallers, native dialogs, real NAT
  traversal, Windows/macOS-specific behaviour, fuzzing campaigns, load tests.
