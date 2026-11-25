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
| Already fixed | CRYPTO-01 and QUAL-01 (see §1); CRYPTO-04 (invite fingerprint now checked on first dial); 22 more and 6 in part after the test-suite audit (see §9) |
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

## 9. Test-suite audit (2026-11-20) — bugs found and their status

The whole test suite was audited against this report (branch
`claude/sharp-cerf-5wz80l`). Every finding below was **reproduced by a test that
failed before the fix** and passes after it, unless it says otherwise. The test
named is the regression guard: if it is deleted or weakened, the bug can return
unnoticed.

### Fixed

| ID | Bug | How it showed | Regression test |
|---|---|---|---|
| CRYPTO-06 / NET-03 | With traffic crossing in both directions, the host started a second key rotation before the peer had switched to the first. Only one previous key is kept, so frames the peer sent under it became undecryptable and the session ended on "Decryption failed!" — two people messaging or sending files at once lost the connection. `rekey_due` now waits until a frame under the current key has arrived (`prev_cipher` empty). | Failed 5 runs out of 5 (500 messages each way, ~10 rotations). | `session_e2e::several_rekeys_under_crossing_traffic_lose_nothing`; the gate itself: `session::tests::a_rekey_waits_until_the_peer_has_switched_to_the_last_one` (checked to fail with the gate reverted) |
| CRYPTO-08 | A nonce of the wrong length in `identity.json` reached `GenericArray::from_slice`, which asserts, so a damaged file **panicked** the unlock instead of reporting an unreadable identity. The length is checked first now. | Panicked on the first case (empty nonce). | `identity::tests::a_malformed_nonce_or_salt_is_an_error_not_a_panic` |
| TAURI-02 | `set_password` on an unlocked app re-wrapped `identity.json` under any new password without asking for the current one — anything able to invoke a command could lock the owner out of their identity and history. A second `unlock` reloaded the history over the live state and dropped conversations not yet saved. `set_password` is now first setup only; `unlock` on an unlocked app is a no-op. | Both failed (re-key accepted; unsaved conversation dropped). | `tests::set_password_cannot_rekey_an_identity_that_already_has_one`, `tests::a_second_unlock_does_not_reload_history_over_unsaved_state` (desktop bridge) |
| UI-03 | Communities shared one composer draft across every channel, DM and server: text typed to one member stayed in the box after switching to a public channel, and Enter posted it to everyone. Drafts are per thread; a refused send returns to the thread it was written for. | Failed. | `Parties.test.jsx` › keeps a DM's unsent text in the DM, never in a channel |
| UI-04 | A slow history response for the channel just left replaced the current one's messages, showing one thread's history under another's name. Responses for a thread no longer open are dropped. | Failed. | `Parties.test.jsx` › never shows a thread's history under another thread's header |
| UI-05 | Community read marks were keyed by the session id minted on every join, so after a restart every channel came back fully badged and the old marks were pruned; pruning ran against the live list and wiped communities not yet rejoined. Marks are anchored on the pinned fingerprint and pruned against the saved list. The old persistence test "restarted" with the same id, which is why it never caught this. | Failed. | `partyUnread.test.js` › survives a restart, which gives the community a new id; › keeps the marks of a saved community that has not rejoined yet |
| UI-08 | The Modal took focus back from any `autoFocus` field, so every text dialog needed an extra Tab; and it captured the element to return focus to *after* the field had focus, so closing returned focus to nothing. | Failed. | `ui.test.jsx` › leaves focus on an autoFocus field and returns it to the opener on close; › returns focus to the opener when the dialog is mounted already open |
| UI-11 | A right-to-left override (or other invisible formatting character) inside a URL in a peer's message made the link display as a different address from the one it opened. Links end at the first `\p{Cf}` character; names and message text are bidi-isolated in CSS. | Failed for RLO, RLI and zero-width space. | `Messages.test.jsx` › cuts a link at a RLO / RLI / zero-width space |
| TUI-22 | Every TUI test read the developer's real `~/.local/share/EncryptedMessenger` and could write a `history.json.enc` there under a throwaway key that the real client then cannot decrypt (this test machine had one from earlier runs). Tests now use `TuiApp::new_in(tempdir)`. | Observed on disk. Verified by comparing the real directory before and after a full run. | all TUI tests (`tui::app::tests`, `tui::ui::tests`, `client/tests/it/tui.rs`) |
| DX-14 | The `P2PEM_TEST_*` timer overrides were read by release binaries too, so the environment could shorten a real session's idle timeout; the test that sets them never restored them. Debug builds only now, restored on drop. A flaky test now fails CI (`flaky-result = "fail"`). | Read in code. | `session_e2e::idle_session_survives_past_the_idle_timeout` |
| QUAL-20 | Startup by sleeping (relay), an upper-bound-only backpressure test that passed when the reader never ran, absence checks that passed by waiting, and an empty test. | Read in code. | relay tests via `serve_relay(listener, …)`; `outgoing_stream_is_backpressured_by_the_bounded_lane`; `client/tests/it/party.rs` (`settle` barrier) |
| DX-12 | 21 client integration-test binaries, each linking the whole tree. Now one (`client/tests/it/`). | Measured. | — |
| CRYPTO-04 | Dialling a contact imported from an invite recorded no expected fingerprint, so a different key answering got an ordinary first-use prompt. The dial now records the invite's key for the conversation; a different key raises a security warning naming the contact, still needs the user's decision, and is never auto-accepted as some *other* known peer. The invite's own key still gets the normal prompt — an invite is not verification. | Failed (no warning; and a key verified for another peer was accepted as the contact). | `a_key_that_is_not_the_invites_is_flagged_on_first_connect`, `a_mismatch_is_not_auto_accepted_as_some_other_known_peer`, `the_invites_own_key_still_gets_the_normal_prompt` (`chat_manager` tests) |
| NET-12 | A contact with a stale (parseable) address and a relay route was dialled direct-only and never reached. The direct candidates are tried first and the relay is used when none accepts a connection — into the same conversation, which now reads `Relay`. A session that connected and later ended is not redialled. | Timed out (90 s) before; connects in about 1 s after. | `client/tests/it/reconnect.rs::a_contact_whose_address_went_stale_is_reached_through_its_relay` |
| NET-10 | The first side to accept the safety code armed its 5-minute idle timer while the other was still on the prompt and sending nothing, so comparing the code over the phone for longer than that ended the session. Until the peer's first frame the limit is its whole confirmation window plus one idle window. A test also pins, for the first time, that silence *after* the peer has spoken is detected. | Failed (idle timeout at 3 s with a 6 s gap). | `session_e2e::confirming_the_safety_code_minutes_apart_keeps_the_session`, `session::tests::a_peer_that_goes_silent_after_speaking_is_detected` |
| NET-06 | The relay did not read a waiting host's stream, so a host that closed (restart, sleep) kept its token for 5 minutes and its own reconnect was refused "already in use". A waiting host sends nothing, so anything readable — EOF above all — now frees the slot. | Failed (returning host refused). | `relay::tests::a_host_that_left_while_waiting_frees_its_token_at_once` |
| NET-07 | No per-address cap on waiting slots: six addresses could fill the table and lock every honest host out. One address (IPv6 per /64) may hold 8. | Failed (a 9th slot accepted). | `relay::tests::one_address_cannot_hold_more_than_its_share_of_waiting_slots` (also checks a second address is still served) |
| NET-11 | The connection password was dropped for relay sessions: anyone holding the token reached the safety-code prompt. Both relay session functions now take it and run the same gate as the direct path. | Failed (prompt reached without the password). | `client/tests/it/reconnect.rs::a_connection_password_is_enforced_over_the_relay` |
| NET-08 | The punch hello carried one token tag for both directions, sent before the peer proved anything: a stranger who reached the host's punch port echoed it back with the role byte flipped and claimed the socket, so the real punch — and the bridge fallback after it — failed. Tags are now bound to the sender's role (hello v2); producing the reply takes the token. A v1 peer is bridged. | Failed (the reflected hello validated). | `punch::tests::a_reflected_hello_does_not_validate`, `two_holders_of_the_token_validate_each_other`; the real punch still wins in `relay_e2e` |
| NAT-01 (new) | Hosting with port mapping asked the router for the forward **first** and checked the router's external address after. Behind a double / carrier-grade NAT the call correctly failed ("use a relay") — and the router kept forwarding the port to this machine for the whole one-hour lease, a hole nobody could use and nothing removed. Both UPnP and NAT-PMP now check the address before mapping. Also: a network with no NAT-PMP gateway took the full 15 s to say so (the library waits on `recv` with no deadline, and the retry loop around it waited for an error it never returns); each reply is now bounded at 3 s. `nat.rs` had been untested beyond address classification: it now runs against a loopback IGD (SSDP + HTTP/SOAP) and a loopback NAT-PMP gateway that record what the router was asked for. | Failed (AddPortMapping / a mapping request sent on a router with a private external address). | `nat::tests::upnp_behind_a_second_nat_asks_the_router_for_nothing`, `natpmp_behind_a_second_nat_asks_the_gateway_for_nothing`; mapping contents and unmapping: `upnp_forwards_the_port_to_this_machine_and_takes_it_back`, `natpmp_reports_the_port_the_gateway_actually_opened`, `natpmp_unmapping_asks_for_a_zero_lease` |
| TUI-30 (new) | The TUI command parser dropped every word a command did not use. With the port written after a space, `:connect 10.0.0.5 9000` dialled the default port 12345 without a word; `--connect "host 9000"` likewise. A leftover word is now an error naming it, with the command's usage; free-text arguments (`:set`, messages) still take the rest of the line. | Failed (`Connect { port: 12345 }`). | `tui::command::tests::a_word_left_over_is_an_error_not_ignored` |
| TUI-31 (new) | The TUI run loop and its launch flags had no test (they read crossterm's global event queue). The loop now takes its event source and runs over a `TestBackend` and a script: quitting saves (`:q` + `y`, and `:q!`, whose only save is the loop's — the test removes the autosave first so it cannot be mistaken for it), key *releases* are ignored (Windows reports both), a paste is never run as a command, the cursor sits after the edit and hides under an overlay. `--host`, `--connect`, `--host-relay`/`--connect-relay` are run for real, the relay pair to the safety-code prompt. Each property was checked by reverting the code it guards. | Untested before. | `tui::tests::*` |
| TAURI-40 (new) | The commands behind native dialogs (`send_file`, `party_send_file[_dm]`, `party_download_file`, `export_identity`, `pick_download_dir`) had never run in a test — only their cores had. The dialog is now passed as data and answered from a script under `cfg(test)`, so the commands run whole: a cancel is a quiet no-op for every one of them and each asks for the right kind of dialog; the chosen folder and backup path are used; a file picked on one desktop is held for acceptance on the other and written intact to the folder that peer chose (the desktop file flow end to end over IPC, untested before); a community file is shared and saved, and the save dialog is pre-filled with the **sanitised** name (a U+202E override would otherwise fill it with `photo‮gnp.exe`). The rfd call itself is the one line left to a desktop session. | Mutation-checked (unsanitised name, cancel → error, folder not saved: each fails a test). | `tests::a_cancelled_dialog_changes_nothing`, `the_chosen_folder_and_backup_location_are_used`, `a_file_picked_in_the_dialog_reaches_the_peer`, `a_community_file_is_shared_and_saved_under_a_name_that_shows_what_it_is` |
| NET-15 | The punch verdict was each client's own: over an RTT above the host's 3 s selection grace, the joiner ACKed and left on its direct socket while the host's wait for that ACK timed out, and the server then bridged the host to the joiner's closed control connection — both sessions failed where a bridged one would have worked. A client whose punch succeeded now waits for the server's verdict and takes the bridge on `Paired`; the server sends the new (appended) `PunchConfirmed` only when both succeeded. Because a server from before this already answers a split with `Paired`, the fix holds against old servers too. The joiner's `select!` also prefers a finished waiter to the deadline (`biased`). | Reproduced (no longer "suspected") with a `cfg(test)` fault for one token: the joiner's ACK delayed past the host's grace — host `Relayed`, joiner `Direct` on a dead socket. | `relay::tests::a_split_punch_verdict_leaves_both_peers_on_the_same_path` |
| NET-16 | The punch candidates a peer listed were dialled from the host's punch port for the whole window, loopback and link-local included: a probe of the host's own local services. Peer-listed candidates are now LAN-reachable addresses only; the relay-observed address is unchanged. | Failed (loopback, `::1`, link-local and broadcast dialled). | `punch::tests::a_peer_cannot_aim_the_punch_at_the_hosts_own_machine` |
| NET-17 | Each connection accepted on the punch port was validated in a detached task: never aborted, unbounded, and holding a sender on the results channel — so one silent connection kept the whole attempt (and the relay pairing) open for the 3 s validation timeout past its budget. Validations now run in a `JoinSet` owned by the listener worker, at most `MAX_PENDING_VALIDATIONS` (4) at once, and die with the attempt. | Failed (40 validated at once; attempt ended ~3 s late). | `punch::tests::silent_connections_to_the_punch_port_neither_delay_nor_outlive_the_attempt` |
| CRYPTO-05 | The client answered the connection-password gate — which runs before the user has compared the safety code — with the password itself, so anyone posing as the host (a malicious relay, a spoofed invite address, a LAN attacker) collected it and used it on the real one. Protocol v5: the client sends `password_proof` (HKDF-SHA256 keyed by the handshake transcript), which cannot be relayed across a man in the middle's two handshakes and admits only offline guessing. A v5 client never answers a pre-v5 host that asks (a fake host would just claim v4); a v5 host still admits a v4 client's plaintext. | Mutation-checked: restoring the plaintext answer fails two tests, answering old hosts fails one. | `session::tests::the_password_proof_matches_its_known_answer` (KAT via Python), `a_host_nobody_has_verified_never_receives_the_password`, `an_older_host_asking_for_the_password_is_not_given_it`, `the_host_admits_the_right_password_from_old_and_new_clients_only`; end to end `session_e2e::*connection_password*` |
| CRYPTO-03 | Same root cause as NET-01 (one key and AAD both ways, so frames could be reflected); fixed with it. | — | `session::tests::a_reflected_frame_is_not_accepted_as_the_peers` |
| NET-04 | Every frame went out as two writes (header, then payload) on sockets with Nagle on, so each small message waited for a delayed ACK — ~40 ms on Linux, up to 200 ms on Windows. One write per frame, and `TCP_NODELAY` on every session, relay and community socket. | Failed (two writes). | `framing::tests::a_frame_is_one_write`, `session::tests::connect_first_reachable_disables_nagle` |
| NET-05 | Anyone completing a handshake with a throwaway key queued a safety-code prompt (no limit) and left a persisted "Peer ab12…" conversation behind when it hung up. The prompt queue is capped at `MAX_QUEUED_PROMPTS` (8), callers past it refused with a toast; an incoming conversation never verified and holding nothing is dropped with its session. | Cleanup failed before the fix; cap mutation-checked. | `chat_manager::tests::unverified_callers_cannot_queue_prompts_without_limit`, `a_caller_that_leaves_unverified_leaves_no_conversation` |
| APP-08 | The next community history page was requested from the newest seq in the *thread*; a broadcast arriving before the first page made the client skip everything between that page and the broadcast, silently. It now continues from the end of the page received. | Failed (asked after 501, not 200). | `party_manager::tests::a_live_message_arriving_first_does_not_skip_history` |
| APP-09 | Large-message reassembly was bounded only by 16 slots × 511 parts — ~384 MiB from one verified peer — and the timeout restarted with every chunk. Partial bytes are budgeted (32 MiB per chat, 64 MiB in all; one maximum-size message still fits) and a message has 10 minutes from its first chunk. | Mutation-checked (budget and age each). | `chat_manager::tests::partial_large_messages_are_held_to_a_byte_budget`, `a_maximum_size_message_still_fits_the_budget`, `a_trickled_large_message_cannot_stay_open_forever` |
| APP-03 | A failed `finalize` (a short `FileEnd`, a failed rename) left the partial spool — and on a failed rename an empty placeholder — in the download folder. Both are removed. | Failed (spool left behind). | `receiver::tests::a_failed_finalize_leaves_nothing_in_the_download_folder` |
| APP-04 | A chunk that could not be written failed the transfer locally and told the sender nothing, so it streamed the rest into nothing and reported "sent". It now sends `FileCancel` and closes the stream. | Failed (no frame sent). | `chat_manager::tests::a_chunk_that_cannot_be_written_tells_the_sender_to_stop` |
| APP-05 | A disconnect failed — and deleted — a held offer that was already complete on disk. Complete offers stay acceptable after the session is gone; incomplete ones are still failed and cleaned up. | Failed (`Failed("connection lost")`). | `a_complete_offer_survives_the_sender_hanging_up`, `an_incomplete_offer_is_still_failed_by_a_disconnect` |
| APP-02 | `send_file` awaited room on the bounded file lane while holding the manager lock; a lane full from an earlier send behind a stalled peer froze every command and the poll loop. A full lane is now an immediate "try again". | Failed (blocked past a 2 s timeout). | `chat_manager::tests::a_full_file_lane_refuses_a_send_instead_of_freezing_the_app` |
| APP-07, JOURNEY-12 | Spools in the download folder (`tmp_<uuid>_<name>`) were never removed after a crash or an exit with an offer held. Loading the history sweeps names of exactly that shape, and nothing while a receive is live. They still live in the download folder while in use — finalizing is a rename, which needs one filesystem. | Mutation-checked (the live-receive guard). | `chat_manager::tests::leftover_spools_are_swept_and_nothing_else_is` |
| SRV-07 | Chunked uploads were checked against the allowance one at a time while their bytes sat in RAM — four 100 MiB uploads each passed a 128 MiB allowance — and nothing bounded the total across members. A member's in-flight bytes count against them, and all uploads together are held to `MAX_UPLOAD_SPOOL_BYTES` (512 MiB). Spools are still in memory; moving them to disk is the larger fix the report suggests. | Failed (second upload accepted). | `state::tests::uploads_in_flight_count_against_the_allowance_and_a_server_budget` |
| SRV-08 (part) | A Guest — read-only everywhere — could still send DMs, so demotion, the only moderation there is, did not stop DM spam. Refused now. Kick, ban and message deletion remain missing (feature work). | Failed. | `state::tests::a_guest_cannot_write_anywhere` (DM case added) |
| SRV-14 | `DownloadFile` returned a whole blob of any size in one frame: a member could make the server read 100 MiB per request, and the reply broke the frame limit. Past `MAX_INLINE_FILE_BYTES` it answers "download it in chunks", after the access check. | Failed (a read was planned). | `dispatch::tests::a_whole_file_download_is_refused_past_the_inline_limit` |
| SRV-11, PRIV-09 (part) | The data directory (`party.db`: every message and DM in plaintext) and the blobs were created under the default umask — world-readable on a shared host. The directory is set to 0700 on every load, which also closes one an older version made. Server logs still record member IPs at info (PRIV-09's other half). | — | `state::tests::the_data_directory_is_owner_only` |
| SRV-10, SRV-12, SRV-13 (part) | Already fixed: multi-row mutations are one transaction, the database runs in WAL with `synchronous=NORMAL`, history is paged from a resident tail instead of scanned from message 1. `files_for`'s per-member work (the rest of SRV-13) is unchanged. | — | `state::tests::multi_row_mutations_commit_as_one_transaction`, `the_database_runs_in_wal_mode_with_normal_sync` |
| TAURI-03 | One password field served hosting and dialling: a dial without a password cleared it, so the next auto-rehost listened with none, and the hosting password was offered to every host dialled. `dial_password` is separate; the desktop sets each on its own path. | Failed (host lost its password). | desktop `tests::dialling_without_a_password_leaves_the_host_s_in_place` |
| TAURI-04 | `party_join` held the community lock across the connect and handshake, so the poll loop — which takes that lock first each tick — stopped for up to `PARTY_CONNECT_TIMEOUT`. It dials with no lock held and takes it only to attach. | Failed (lock held for the whole dial). | desktop `tests::a_join_waiting_on_a_silent_server_does_not_hold_the_community_lock` |
| TAURI-08 | Turning mDNS off changed only the flag: the daemon kept browsing and a registration kept announcing the user's name and port. The daemon is dropped (which unregisters) and the discovered list cleared. | Failed. | desktop `tests::nearby_peers_are_not_browsed_unless_the_user_opted_in` (off half added) |
| TAURI-07 | `unlock` and `set_password` ran Argon2 (64 MiB × 3 passes) holding the identity mutex on a runtime worker, so every command reading the identity — and the poll loop behind it — waited the derivation out. Argon2 now runs on a copy in `spawn_blocking` and the copy is swapped in; a static `AUTH_OP` lock serialises the three Argon2 commands so two cannot each finish against the same starting state. | Failed (1 free sample in a ~140 ms unlock). | desktop `tests::unlocking_does_not_hold_the_identity_while_the_key_is_derived` |
| TAURI-05 | Nothing stopped a second instance: both loaded the same identity and history, and each rewrote the whole history from its own copy, so the last writer destroyed what only the other had received. `init_bridge` takes an exclusive `File::try_lock` on `<data_dir>/.lock` before reading the identity; a second instance boots into the error state and writes nothing. The lock is per folder, so `P2PEM_DATA_DIR` peers still coexist, and the OS releases it on any exit. The wipe removes the lock file too. | Failed (second instance booted). | desktop `tests::a_second_instance_on_the_same_data_folder_is_refused` |
| TAURI-06 | The "executes on open" deny-list missed disk images, macro documents, HTML/SVG and many Windows/macOS launchers, and received files carried no Mark-of-the-Web. `open_file` now gates everything outside `INERT_EXTENSIONS` (images, PDF, text, audio, video), with `runs` telling the UI whether to say "this is a program" or "this type can contain code"; the deny-list grew by the audit's entries. Windows files get `Zone.Identifier` (ZoneId=3) on finalize (`util::mark_as_downloaded`), macOS bundles set `LSFileQuarantineEnabled`. `bridge.js` mirrors both lists and `bridge.test.js` compares them with `chats.rs`. A `cfg(test)` seam records what `open_path_os` would launch, so the *opening* half is tested too. | Failed (no `runs`; .docx/.iso/README opened without asking). | desktop `tests::opening_a_received_executable_needs_confirmation`; `bridge.test.js` `openGate`; `Messages.test.jsx` (two dialogs, empty kind); core `receiver::a_received_file_is_marked_as_coming_from_the_internet` (Windows CI) |
| PERF-12 (partly) | Setup ran Argon2 twice (`encrypt`, then `decrypt` only to recover the key it had discarded), the TUI re-decrypted the saved file after every set/change, and `argon2` ran its 4 lanes sequentially. `Identity::protect` encrypts and keeps the key (one run; used by the desktop `set_password` and the TUI); the TUI reloads the file only to compare it; `argon2/parallel` is on (rayon was already in the tree) — 193 → 97 ms per derivation in the test profile, output unchanged against a reference-implementation KAT. Unlock is off the identity lock (TAURI-07). **Still open:** first-run RSA keygen runs in `init_bridge` before the window exists, and `load_history_auto` decrypts under the manager lock. | Failed (`protect` absent). | core `identity::tests::protecting_an_identity_derives_once_and_stays_unlocked`, `the_wrapping_key_matches_the_reference_implementation` (argon2-cffi KAT) |
| PERF-09 (partly) | The desktop poll loop woke at 4 Hz for the life of the app and walked every message twice per tick. It now sleeps `poll_delay(focused, quiet_for)`: 250 ms while the window is focused or anything moved in the last 10 s, else 1 s (a background message is still seen within a second, and puts the loop back on the fast period). `ui_signature_from` reuses the tick's `state_signature` (one walk, not two); `PartyStatus` derives `Hash` instead of a `format!` per server per tick. **Still open:** the walk itself (a `revision` counter / cached delivered count) and an event-driven wake. | Failed (no `poll_delay`). | desktop `tests::the_poll_loop_slows_down_only_when_backgrounded_and_quiet` |
| TUI-24 (partly) | `--relay-server`, a daemon, logged nowhere — not even that it was listening — and the parsed-arguments debug line recorded `--relay-token`, which then reached clipboard copies and diagnostics bundles. The relay adds a stderr `fmt` layer (the TUI cannot: stderr is its screen), and the arguments are logged with the token redacted. The docs no longer describe a log overlay that does not exist. **Still open:** a `:logs` overlay and a log file for the TUI. | Failed (empty stderr). | client `it::cli::relay_server_mode_serves_a_rendezvous` (stderr + redaction; redaction mutation-checked) |
| TUI-13 (partly) | A failing autosave was only a log line, so the user believed history was being kept; `:party-send-file` read the whole file before the size check refused it (a 10 GB name meant a 10 GB allocation). Save failures — background, inline and the explicit save — now go through `record_save_outcome`, which toasts once per failure streak and is reset by a save that works; uploads are refused from `metadata().len()` before any read. The change-detecting background autosave was PERF-03; set/change password now runs one Argon2 pass instead of two (PERF-12). **Still open:** unlock's Argon2 still runs on the UI loop. | Failed (no toast). Size check: not distinguishable from outside without measuring allocation. | client `tui::app::tests::a_failing_autosave_is_reported_once_per_streak`, `cmd_party::upload_tests::an_oversized_file_is_refused_from_its_size_alone` |
| TUI-14 (partly) | The loop handled one input event per frame, so every key of a burst (fast typing, a held key, a paste delivered as keys) cost a full tick and redraw. After the blocking poll, `run_app` drains what is already waiting (`timeout = 0`), capped at `MAX_EVENTS_PER_FRAME` so a flood still repaints, and **stops at a submitted command** — the app holds one `pending_command`, so a second submitted in the same frame would have replaced the first (the test types two commands in one burst, and the mutation without that stop loses the first). Long texts were already capped for display (PERF-02). **Still open:** each frame still lays out every message of the open chat. | Failed (27 keys, 27 frames). | client `tui::tests::a_burst_of_keys_is_handled_in_one_frame` |
| TUI-21 (partly) | The peer and own fingerprints and every confirmation were drawn `Color::White` — white on white on a light terminal, and the fingerprint is the SAS's backstop — and `NO_COLOR` was ignored. Body text now uses the terminal's own foreground; with `NO_COLOR` set, `ui` strips colour from the finished frame (one pass, so no widget can forget it) and keeps emphasis. **Still open:** the RGB safety grid on terminals without truecolor, and the unused `theme` setting (TUI-19). | Failed (fg White; DarkGray cells under NO_COLOR). | client `tui::tests::body_text_uses_the_terminals_own_foreground`, `no_color_draws_without_colour` |
| TUI-19 (partly) | The TUI offered `auto-host`, `mdns` and `theme` and honoured none. `auto-host` now hosts on the configured port after unlock and after *first* password setup (not after a change, and never over a launch flag or a running listener); `mdns` and `theme` are gone from the panel and `:set` refuses them with the reason (the TUI's settings live in its own data folder, so they never reached the desktop either); `upnp`, which works, is listed. README/TUTORIAL no longer claim parity, and CLAUDE.md no longer says the TUI surfaces discovery. **Still open:** the missing commands (block/unblock, `party-leave`, specific file offers…). | Failed (`:set mdns` accepted; no host queued on unlock). | client `tui::app::tests::unlocking_hosts_on_startup_when_the_setting_is_on`, `settings_the_terminal_ignores_are_not_offered` |
| TUI-20 | Past three rows the composer and command box clipped the text (typing blind), and on a soft-wrapped line the hardware cursor stuck to the first row's border. `ui::layout_input` lays the text out in rows of the box width — by display width, char by char, so a wide glyph never straddles the edge — and finds the cursor on them; the box draws exactly those rows with `Paragraph::scroll` set to keep the cursor's row in view, and `set_cursor_position` uses the same `input_view`, so the two cannot drift. An end cursor on a full row moves to the next row instead of the border. The logical-row `cursor_display` is gone. | Failed (end of text not in the box). | client `tui::tests::the_composer_scrolls_to_the_cursor_on_wrapped_text`, `tui::ui::input_layout_tests::input_rows_wrap_at_the_box_width_and_carry_the_cursor` |
| PERF-07 (partly) | `finalize` — `sync_all` of up to 10 GiB, then the rename — ran inside `handle_session_event`, under the manager lock, so every command and the poll loop waited for the disk. It runs on `spawn_blocking`; the outcome is applied on a later poll (`drain_finalized`). In that window a disconnect does not fail the transfer (`finalizing` is skipped like `pending_file_end`), and a cancel or wipe removes the written file instead of orphaning it. **Still open:** chunk writes still happen under the lock, and the receive side has no backpressure (unbounded `to_app_tx`). | Failed (finalized inline). Mutation without the disconnect exemption: the file never lands. | client `chat_manager::tests::finalizing_a_received_file_happens_off_the_manager` |
| PERF-11, DX-04 | `image` was built with every default codec (AVIF/rav1e, EXR, TIFF, WebP…, ~89 CPU-s per cold build, all linked into the desktop app) to write one PNG QR code. The workspace now asks for `image` with `png` only and `qrcode` with `image` only; `rav1e`/`exr` are gone from the tree. The `paste` acceptance (RUSTSEC-2024-0436) stays — it is still reached via `natpmp → netdev → netlink-packet-utils` — and its comment now names that path instead of `image`. | `cargo tree -e features -i image` shows only `png`. | client invite/QR tests (28) |
| JOURNEY-08 | Duplicate of TAURI-05 (single-instance guard). | — | see TAURI-05 |
| DOC-08 | `upnp` was missing from `settings_keys()` and the guide's `:set` list; fixed with TUI-19. | Failed. | client `tui::app::tests::settings_the_terminal_ignores_are_not_offered` |
| APP-10 | Already fixed in code (`connect_and_join` is bounded by `PARTY_CONNECT_TIMEOUT`, and the desktop dials without the community lock — TAURI-04); the status had not been updated. | — | client `party_manager::tests::a_server_that_never_answers_is_given_up_on`; desktop `tests::a_join_waiting_on_a_silent_server_does_not_hold_the_community_lock` |
| DOC-14–17 | `architecture.md` said history is rewritten once per message (saves are coalesced); CLAUDE.md said chunked upload ids are client-chosen (the server mints them), that unaccepted files never reach the download dir (their `tmp_` spool lives there — now also in the user guide), and DOC-15's relay-fallback claim became true with NET-12 (`run_client_session_direct_or_relay`). | — | docs only |
| QUAL-06 | `LogBuffer`'s visitor kept only `message` (or the first field when there was none), so 114 structured calls reached Ctrl+L copies and diagnostics bundles without their error, path or id — and the existing test asserted exactly that. It renders `message key=value …` like `fmt`, so the style split no longer loses anything. Because lines now carry fingerprints and addresses, the clipboard copy goes through the same `shareable_event_logs` (`redact_for_sharing`) as the bundle. | Failed (fields dropped). | client `logbuf::tests::captures_events_through_a_subscriber`, `support::tests::the_shareable_log_keeps_the_error_and_drops_identifiers` |
| QUAL-07, JOURNEY-10 (partly) | The shipped desktop app logged to stdout only (a Windows release build has none), installed no panic hook (with `panic = "abort"`, a crash left nothing), and its diagnostics bundle said it contained no logs; `tauri-plugin-log`, `log` and `anyhow` were declared and unused. The subscriber now also feeds a process-wide `LogBuffer` (`app_logs()`), which the bundle carries (scrubbed by `export_diagnostics_bundle`); `support::install_panic_hook_in` writes crash reports under the desktop's own `diagnostics/crashes`; the three dependencies are gone (−76 lockfile lines). **Still open (JOURNEY-10):** no log *file* survives a restart, and crash reports are not offered in the bundle. | Failed (bundle said "no in-app log buffer"). | desktop `tests::the_diagnostics_bundle_carries_the_app_log`; client `support::tests::a_panic_leaves_a_crash_report_in_the_given_folder` |
| DOC-11 | `SECURITY.md`'s accepted-advisory list still described the GTK3 family (withdrawn upstream) and omitted five live acceptances (`proc-macro-error`, `paste`, the `zvariant` trio). It is rewritten from `cargo tree --target all -i`, and `scripts/check-stale-advisories.sh` now fails when `SECURITY.md` and `deny.toml`'s `ignore` array name different IDs — before its cargo-deny step, so it runs even where cargo-deny is absent. | Failed (5 undocumented). | `scripts/check-stale-advisories.sh` (CI) |
| DOC-09 | The user guide's TUI command reference was 15 commands behind `COMMANDS` (`:accept`, `:decline`, `:lock`, `:backup`, `:wipe`, the community file and admin commands…) and described `:disconnect` as removing the chat (TUI-15 made it keep the history). All are listed now, and a test reads the guide and fails on any command that is undocumented or documented but unknown. | Mutation: the previous guide fails with the 15 names. | client `tui::command::tests::the_user_guide_documents_every_command` |
| QUAL-05 (partly) | A message whose row could not be written was appended in memory and confirmed to the sender (`MessagePosted`), then lost on restart — the server-side "Ok + a toast". `append_channel_message`/`append_dm_message` now persist **first** and return `Err` ("the server could not store the message"), which `dispatch` answers `ActionFailed`, so the client retracts the row; memory, the sequence number and an empty new DM thread are left untouched. Already in place before this pass: `with_tx` around multi-row mutations, WAL + `synchronous=NORMAL` + `foreign_keys`. **Still open:** the bookkeeping `persist_*` rows (members, refs, audit) are still only logged, and migrations have no `user_version`. | Failed (post confirmed). | server `state::tests::a_message_the_database_refuses_is_not_confirmed` (a SQLite trigger refuses the inserts) |
| DOC-12, DOC-13, DOC-18 | `architecture.md`'s code map named files a refactor had split (`tui/app.rs`, `server/src/state.rs`) and omitted others (`identity/invite.rs`, `saved_parties.rs`, the `state/` modules); SECURITY.md/architecture.md said the React layer had no component or launch tests (jsdom and Playwright both exist) and carried test counts that were wrong everywhere; `server_tiers.md` promised message deletion and member removal, which do not exist. All corrected, counts removed. A test now fails when any doc quotes a repository path in backticks that does not exist. | Mutation: a quoted `client/src/tui/app.rs` fails it. | client `it::docs::every_path_the_docs_name_exists` |
| Duplicates closed by earlier fixes | NET-19, PERF-13 (release logs at debug) — PRIV-05's `default_log_filter`; TAURI-22 (`send_file` awaiting the lane under the lock) — APP-02's `try_send`; SUPPLY-07, TAURI-16 (dead log plugin, no logs in diagnostics) — QUAL-07. Partly: TAURI-14 = PERF-09, APP-21 = PERF-07, TAURI-21 (a panic now leaves a crash report — QUAL-07 — but `panic = "abort"` still skips the history flush). | — | see the referenced rows |
| APP-16, APP-17 | `clear_history` aborted the listener's task but left `is_hosting`/`hosting_port` set, so auto-rehost reopened the port straight after "erase everything"; it now calls `stop_hosting` (which also drops the UPnP mapping). `delete_chat` left `contact_to_chat` pointing at the deleted conversation (and its incoming twins), so the next connect aimed at a dead chat id; those links go with the chat, the contact stays. | Failed (still hosting; still linked). | client `chat_manager::tests::clearing_history_stops_hosting_for_good`, `deleting_a_chat_unlinks_its_contact` |
| APP-15 | Anyone can write an unsigned v1 link for a known peer — it carries only the public key and fingerprint — and importing one replaced that contact's name and address, even a Verified contact's, so their next dial went where the forger chose. `import_contact_from_link(contact, signed)` refuses an unsigned refresh of an existing contact with a message saying why; a signed link (only the key's owner can make one) still refreshes. Both shells pass `invite_link_is_signed`. | Failed (mutation: forged link accepted). | client `chat_manager::tests::an_unsigned_invite_cannot_change_an_existing_contact` |
| TAURI-20 | Every unlock failure read "Wrong password" — including a missing salt or nonce, a nonce of the wrong length, or unusable Argon2 parameters, which retyping can never fix. `Identity::decrypt` now returns a typed `WrongPassword` only when sound parameters simply did not open the key (a bad nonce length is checked first and reported as damage); both shells say "wrong password" for that and "your identity file is damaged — restore it from a backup" for anything else. | Failed (damaged file read as "Wrong password"). | desktop `tests::unlock_tells_a_damaged_identity_from_a_wrong_password` |
| TUI-25, TUI-27 | Raw mode swallows SIGINT and nothing handled Ctrl+C, so the universal way out did nothing; it now opens the quit confirmation from anywhere (composer and command mode included) and a second Ctrl+C confirms, saving like `y`. (Esc in the verify overlay already defers — TUI-01.) Auto-rehost wrote its `Host` into the single pending-command slot whatever was there, replacing a command the user had just submitted; it now waits a turn. | Failed (Ctrl+C ignored; command replaced). | client `tui::app::tests::ctrl_c_asks_to_quit_and_a_second_one_quits`, `auto_rehost_never_replaces_a_submitted_command` |
| CRYPTO-12, CRYPTO-13 | Names other people read refused zero-width and bidi characters but not U+061C, U+2028/2029, the Hangul fillers (a username of them looks blank), U+034F, variation selectors or tag characters (`admin` + a tag looks like `admin`); filenames kept U+2028/2029 and U+061C, which rearrange a file card. Windows device names missed `COM0`/`LPT0`, the superscript `COM¹²³`/`LPT¹²³`, `CONIN$`/`CONOUT$`, and a stem with trailing spaces (`CON .txt` is the console). All added; the stem is compared after trimming trailing spaces and dots. | Failed (U+061C passed; `COM0.txt` kept). | core `util::tests::names_hiding_these_characters_are_refused`, `every_reserved_windows_device_name_is_defused` |
| CRYPTO-14 | Any RSA key the crate parses was accepted as a peer identity — 512 or 1024 bits included — and Ed25519 used the permissive `verify`, under which the small-order identity key verifies one signature for every message. `pem_decode_public`, the parse the handshake and invites accept keys through, refuses keys under `RSA_KEY_BITS` (2048; every key the app generates is that size); fingerprints use the same strict parse without the floor (a fingerprint describes a key, and the frozen history-key vector is 1024-bit). `verify_ed25519` uses `verify_strict`. | Failed (1024-bit key accepted; forged signature verified). | core `crypto::tests::a_peer_identity_key_under_2048_bits_is_refused`, `ed25519_verification_rejects_a_small_order_key` |
| TAURI-18 | The desktop invite advertised `config.listen_port` while the app listened elsewhere (a host on a free port, or one the user chose), so peers dialled a closed port; it now uses `hosting_port` when hosting — the desktop twin of TUI-23. | Failed (link had :12345, listener elsewhere). | desktop `tests::the_invite_advertises_the_port_actually_listening` |
| TAURI-17 | `change_password` re-wraps a copy of the identity off the lock and swaps it in at the end, so a display name changed meanwhile was put back in memory and on disk (reproduced on every run). `set_display_name` now takes `AUTH_OP`, the lock the password commands hold. | Failed (3/3 runs). | desktop `tests::a_rename_during_a_password_change_is_kept` |
| TAURI-31, JOURNEY-06 (part) | `export_identity` accepted the identity file itself as the destination — copying a file onto itself truncates it on Linux, destroying the only key to the history — and recorded "backed up" without reading the copy back. The source is refused (compared canonically) and a backup is recorded only if it parses. Restoring from a backup is still not offered in the app (the rest of JOURNEY-06). | Failed (copy onto itself accepted). | desktop `tests::backing_up_onto_the_identity_itself_is_refused` |
| TAURI-09, UI-06 | Already fixed: transfer progress has its own `transfers-updated` event and no longer refetches the conversation. | — | `App.test.jsx` › a transfer progress event refetches transfers, not conversations |
| TUI-15 | `:disconnect` deleted the conversation and its history (saved at once, no confirmation). It now closes the session only (`ChatManager::disconnect_chat`, through the same path as a dropped connection); `:delete` asks before removing anything. | Mutation-checked (`:delete` without asking). | `chat_manager::tests::disconnecting_keeps_the_conversation_and_its_history`, `tui::app::tests::deleting_a_conversation_asks_first` |
| TUI-18 | The open conversation was marked read on every tick, so a message arriving while the user was away was read — and saved as read — before anyone saw it. Marked only while someone is at the terminal. | Mutation-checked. | `tui::app::tests::a_message_that_arrives_while_away_stays_unread` |
| TUI-23 | After `:host 9000`, `:invite` embedded the configured port (12345): a link nobody could connect to. It uses the port actually hosted. | Failed (`:12345` in the link). | `tui::app::tests::an_invite_names_the_port_being_hosted` |
| TUI-09 | A message was one line with its text in one span, and ratatui drops `\n` inside a span: "a\nb" rendered "ab". One screen line per line of the message, continuation lines indented, and the scroll measurement indented the same way. | Failed ("first linesecond line"). | `tui::ui::tests::a_multi_line_message_keeps_its_lines` |
| TUI-10 | (a) Launch flags ran before the unlock prompt was answered, so on every launch after the first they failed on the locked key and were lost; they now wait for the unlock and run once. (b) `--connect host --port 9000` dialled 12345; `--port` is the default for a target that names no port. | Mutation-checked (each half). | `tui::tests::launch_flags_wait_for_the_unlock`, `connect_uses_the_port_flag_when_the_target_names_none` |
| TUI-17, DOC-10 | The TUI kept no community pins, so every `:party-connect` was a first contact and `:party-trust` became a reflex — while the user guide said the app pins. The saved-communities code moved from the desktop into `p2pem_classic::app::saved_parties`, shared by both shells: the TUI pins a community once it has joined and holds a later join to that address to the pinned key, refusing a different one before anything is sent and without offering a prompt. A damaged `parties.json` stops the join, as on the desktop. | Mutation-checked (pin ignored). | `tui::app::tests::a_joined_community_is_pinned_and_held_to_its_key` |
| TUI-16 | Already fixed in code (the pane's community drives `current_party`; posts go to the selected thread) but untested. Now: with two communities joined, moving the pane moves the target, and `:party-post` reaches only the community on screen. | Mutation-checked (selection not synced). | `tui::app::tests::posting_goes_to_the_community_on_screen` |
| TUI-11 (part) | Inline passwords were cut at the first space (`:set-password correct horse battery staple` set "correct") and kept in the command history, recallable with ↑. The secret is now the rest of the line, and the history keeps such commands without it. Still shown while typed: the masked overlay is the way to avoid that, and `:change-password` takes two words. | Failed (phrase refused; no history redaction). | `tui::command::tests::a_password_is_the_rest_of_the_line_and_never_reaches_history` |
| TUI-12 | A new password was typed once, so a typo became the password of the only key to the history, and the 12-character floor was not stated. It is typed twice (a mismatch sets nothing), the floor is checked at the first entry, and the prompt says it. | Mutation-checked (confirmation not compared). | `tui::app::tests::a_new_password_must_be_typed_twice` |
| UI-07 | The invite link (address, external address, display name) was fetched once and cached for the session; it is fetched again on each open. | Failed (no second fetch). | `Creator.test.jsx` › is fetched afresh each time the invite tab opens |
| UI-09 | `.toast-region:empty { display:none }` removed both live regions from the accessibility tree while empty, so the first announcement was often lost. Removed (an empty region has no size). | Failed. | `contrast.test.js` › live regions are never removed from rendering while empty |
| UI-10 | `--faint` (timestamps, hints, small text) was 2.7–4.1:1 in every theme. Each theme's value moves toward its own text colour until it reaches 4.5:1 on `--bg`, `--s1` and `--s2`; a test reads the shipped CSS and checks every theme. | Failed (all five themes). | `contrast.test.js` › faint text meets AA on every surface, per theme |
| PRIV-06, JOURNEY-07 | Desktop notifications put the message text into the OS notification store and on the lock screen, outside the app's encryption, with no way to say less. New `Config::notification_content` (`full` / `sender_only` / `hidden`, `serde(default)` → `sender_only`): by default a notification names the sender, not the message. Settable in the desktop Settings (offered only while notifications are on) and with `:set notification-content` in the TUI; an older frontend that omits it does not reset it. | New behaviour. | `chat_manager::tests::a_notification_says_only_what_the_setting_allows`; config back-compat in `types.rs`; desktop `settings_roundtrip_and_validation`; `Settings.test.jsx` › saves the chosen level; TUI `set_command_toggles_config` |
| PRIV-04 (part) | The diagnostics bundle — made to be attached to a public issue — carried live relay tokens, peer IPs and full fingerprints in its logs, in files written 0644. Its files are written with `write_file_atomic` (0600), 32+-hex runs and IPv4 addresses in the logs become `<hex>`/`<ip>`, and the README says what may remain (file and contact names) and to read `logs.txt` first. Names are not recognised by shape, so that half stays open. | — | `support::tests::export_bundle_writes_expected_files`, `redaction_removes_secrets_and_keeps_the_rest` |
| PRIV-05 | A release desktop build logged at debug level by default (file names, peer addresses, frame metadata) to stdout, which on Linux can persist in the systemd journal. Release builds default to `warn,p2pem_desktop=info`; debug builds keep debug; `RUST_LOG` still overrides. | — | desktop `tests::a_release_build_does_not_log_at_debug_level_by_default` |
| PERF-06, PERF-10, PERF-08 (part) | Covered by fixes above: server history keeps only a resident tail and pages from SQLite; Nagle is the same defect as NET-04; upload memory is bounded server-wide by SRV-07 (spools are still in RAM). | — | see SRV-01/NET-04/SRV-07 rows |
| UI-01, UI-02, NET-01 | Found already fixed in code, each with a passing regression test. | — | `Parties.test.jsx` › binds the join to the fingerprint; `App.test.jsx` deferred `getConversation`; `session::tests::a_reflected_frame_is_not_accepted_as_the_peers` |

### Critical and High findings fixed before this pass — the test that guards each

The index still listed these as open. Each was checked against the code on this
branch and found fixed; this is the regression guard for each (tests added in
this pass are marked *new*). Medium and Low statuses were **not** reconciled
here beyond the findings named elsewhere in this section.

| ID | Guard |
|---|---|
| JOURNEY-01, APP-01, QUAL-02, TAURI-30 (one bug, four reports) | `client/tests/it/persistence.rs::an_unreadable_history_fails_cleanly_and_changes_nothing`; desktop `tests::an_unreadable_history_survives_unlock_and_can_be_set_aside` |
| SRV-01 | `server state::tests::a_channel_of_maximum_size_messages_still_pages_in_frames_that_fit`, `dm_history_of_maximum_size_messages_fits_in_a_frame`; `client/tests/it/party.rs::a_channel_of_maximum_size_messages_is_joinable_and_pages_to_the_end` |
| CRYPTO-02 | `session::tests::current_peers_commit_to_their_ephemeral_keys` |
| NET-02 | `session_e2e::silent_connections_do_not_lock_a_real_peer_out_of_the_host`; `host_listener::host_keeps_its_port_through_idle_connections_and_releases_it_for_a_peer` |
| SRV-02 | `connection::tests::an_evicted_connection_is_closed_rather_than_left_deaf` |
| SRV-03 | `connection::tests::a_connection_that_never_joins_is_closed`, `a_client_that_stops_reading_is_disconnected_by_the_write_timeout`; `server lib::tests::the_accept_loop_refuses_connections_past_its_cap`, `address_slots_are_capped_and_released` |
| SRV-04 | `state::tests::being_first_grants_nothing_and_the_claim_token_grants_ownership_once`; `party::the_owner_is_whoever_presents_the_claim_token_not_whoever_is_first`; `server/tests/cli.rs` (no password without `--open`) |
| SRV-05, TAURI-01, TUI-02, DOC-01 | `party::a_changed_server_identity_is_refused_before_any_credential_is_sent`; desktop `tests::party_join_is_two_steps_and_pins_only_after_a_real_join`; TUI `the_party_commands_work_end_to_end` (nothing reaches the server before `:party-trust`) |
| SRV-06 | `write_rate::tests::a_burst_is_allowed_then_refused_then_refilled`, `text_bytes_have_their_own_budget` |
| TUI-01 | `tui::app::tests::typing_into_the_fingerprint_prompt_does_not_answer_it` |
| TUI-03 | *new* `tui::app::tests::set_password_will_not_replace_a_password_it_was_not_given` (mutation-checked) |
| TUI-04, JOURNEY-02 | `tui::app::tests::the_first_run_password_prompt_cannot_be_dismissed` |
| TUI-05 | `tui::app::tests::a_paste_is_text_never_keystrokes`; *new* `tui::tests::a_pasted_command_is_text_not_a_command` (through the run loop) |
| TUI-06 | None: restoring a real terminal needs a TTY. `TerminalGuard` is RAII and entered before anything can fail (read in code). |
| TUI-07 | *new*: `the_party_commands_work_end_to_end` now draws the Communities pane and asserts the posted message and the operator warning are on screen |
| TUI-08 | *new* `party_manager::tests::a_server_that_never_answers_is_given_up_on` (paused clock) |
| JOURNEY-03, DOC-02 | desktop `tests::delete_all_data_needs_the_password_and_the_word_then_removes_everything`; TUI `wipe_checks_the_password_confirms_then_deletes_everything` |
| JOURNEY-04, PRIV-01, DOC-03 | `Parties.test.jsx` › says on screen that the operator can read the community; the TUI-07 render assertion |
| PERF-01 | *new* `App.test.jsx` › a transfer progress event refetches transfers, not conversations (mutation-checked) |
| PERF-02 | desktop `chats::tests::previews_and_displayed_texts_are_bounded` |
| PERF-03 | TUI `autosave_skips_an_unchanged_history`; desktop `the_poll_loop_saves_changed_history_once_and_skips_an_unchanged_one` |
| PERF-04 | `state::tests::the_database_runs_in_wal_mode_with_normal_sync` |
| PERF-05 | `party_manager::tests::history_pages_are_merged_and_continued` (correctness; the complexity itself is not asserted) |
| DX-01, DX-03 | Build configuration (`Cargo.toml` profiles, `crate-type = ["rlib"]`); measured, not testable as a unit. |
| DX-02, JOURNEY-05 | CI configuration (`ci.yml` classifier, `release.yml` permissions); not testable in the suite. |
| DOC-04 – DOC-07 | Documentation, checked against the text. |

### Partly fixed

| ID | Done | Still open |
|---|---|---|
| QUAL-19 | One shared client harness; hosts and relays bind port 0 in tests. | `punch.rs` tests still pick a free port and rebind — a hole punch needs a chosen port, so the race is inherent there. No cached RSA test keys (nextest runs a process per test, so a cache would not survive between tests). |
| TUI-22 | Tests are hermetic; the diagnostics bundle lands in the app's own data dir. | The TUI still has no data-dir override, so two TUI peers cannot run on one machine. |
| CRYPTO-17 | Tests for reflected frames, several rekeys with traffic both ways, a malformed identity nonce, and an invite-fingerprint mismatch on first connect (CRYPTO-04). | No test for a peer reusing our own ephemeral key, or for history rollback (CRYPTO-09, still open). |
| NET-22 | Reflection, crossing rekeys, silent connection in the handshake (already present), stale direct addresses reaching the relay (NET-12), re-registration after a host left (NET-06), the per-address slot cap (NET-07), the password over relay (NET-11), peers confirming far apart (NET-10). | — (split punch verdicts, NET-15, now reproduced and fixed). |
| TAURI-27 | `open_file` refusal of received executables, `file_preview` (SVG refused, 4 MiB cap), `set_password`/`unlock` on a ready app, `party_join`'s two steps against a real server (pin written only after a real join), the upload size cap and download core behind the native dialogs, `state_signature` sensitivity, save coalescing, prompt publishing, auto-rehost, the real start-up (`init_bridge`), the identity backup, diagnostics, mDNS opt-in, and the whole host → dial → verify → message → receipt flow over IPC. | The native dialogs themselves (cannot open headless). |
| UI-37 | UI-03, UI-04, UI-05, UI-08 and UI-11 each have a test (and a fix). | Smoke tests still cover first paint only, not a flow (the flow is covered over IPC by the bridge tests, and per component by vitest). |

### Confirmed still open

- **APP-06 (Medium).** `FileCancel` carries no transfer id, so with files moving
  both ways on one conversation a decline can cancel the wrong one. A real fix is
  a protocol change (a direction or transfer id on the frame, negotiated by
  version so an old peer is not sent a frame it would silently ignore); the
  one-outgoing-send-per-chat guard already limits it to that two-way case.

- **NET-09 (Medium, privacy).** The relay control channel is cleartext: the
  rendezvous token and LAN candidates are visible on path. TOFU and the SAS
  still stop silent impersonation. Fixing it means an encrypted control
  channel pinned to the relay's key — a design change, not a test.

- **SRV-09, SRV-15, SRV-16 (Medium).** The audit log can be flooded with
  self-service entries and is never pruned; usernames are unique only up to
  ASCII case, so homoglyphs (`аdmin` with a Cyrillic а) pass; the per-IP limit
  refuses a crowd behind one NAT without a word. Each needs a design decision
  (what to audit; a confusables skeleton and a new dependency; a refusal message
  before the handshake) rather than a bug fix.

- **PRIV-02, PRIV-03 (Medium, privacy).** One long-term identity key
  authenticates the user to every community, so operators can link a user
  across communities; and the relay hands each peer the other's public and LAN
  addresses before either has verified the other. Both are design decisions —
  a per-community key derived from the identity, and sending LAN candidates
  only after verification — that change what the protocol promises, not bugs a
  test can pin.

- **CRYPTO-09 (Low, security).** The history file has no rollback protection:
  an older `history.json.enc` loads normally and restores revoked trust and
  lifted blocks. The report's remedy — a generation counter kept in
  `identity.json` — would not hold against the attacker it is for: anyone able
  to write the data directory can put back an old `identity.json` with the old
  history, and the key does not change, so the pair checks out. Real
  anti-rollback needs a monotonic counter outside that directory (an OS
  keychain or TPM). Left open on purpose rather than shipped as a protection
  that only looks like one.

### Second pass, on the tree merged with `main` (2026-11-24)

After merging `main` (desktop redesign, CRYPTO-04, rsa 0.10), coverage was
re-measured **from a clean profile**: the earlier figures mixed stale
instrumented binaries and shifted line numbers. Rust is at **91.6 % of lines**
(`cargo llvm-cov`, all crates), the frontend at **82.7 %** (vitest + v8).
`server/src/main.rs` and `client/src/main.rs` read lower than what is tested,
because the CLI tests stop those binaries with `kill()` (SIGKILL) and a killed
process writes no profile.

**Bug found and fixed.** A peer offering a second file while the first was
still streaming made the first vanish from the transfer list with no message:
its "the sender started another file" status was purged in the same call
that set it. A warning now names the file
(`a_file_offer_that_cannot_proceed_cleans_up_and_tells_the_sender`, seen
failing before the fix).

**Tests that could not fail, fixed.**
- `Settings` "rolls a rejected toggle back rather than leaving it flipped"
  checked only the error toast; it passed with the rollback deleted.
- `the_ui_signature_moves_for_badges_and_hosting` ended on
  `transfer_signature(m) == transfer_signature(m)`; replaced by
  `transfer_progress_and_transfer_state_move_different_signatures`.
- `renders_every_overlay_without_panic` passes for an overlay that draws
  nothing; `overlays_say_what_the_state_they_show_is` checks the words.

**Contracts now tested from both sides.**
- `desktop/src/lib/bridge.invoke.test.js` runs every real `bridge.js` wrapper
  against a recording `invoke`: each command must be registered in
  `invoke_handler()`, and its exact key set must be one a Rust IPC test sends.
  This is the arg-naming footgun behind "messages send but don't arrive".
- `list_transfers_reports_each_status_in_the_frontends_words` pins the status
  strings `Messages.jsx` branches on.

**Previously untested code now exercised**, each test seen failing under a
deliberate break of the code it covers:
- the desktop poll loop's body (`PollContext::tick`, now separate from the
  loop so the mock runtime can run it);
- `--retain-days` pruning at server start-up;
- start-up auto-reconnect and auto-host;
- the hosting guards (a second listener, a dead one, repeated bind failures);
- the TUI's community governance and messaging commands, and the transfers
  overlay;
- the protocol's redacting `Debug` (no message text or key bytes in logs)
  and the legacy `FILE_CHUNK` arm;
- Reconnect choosing the right saved contact;
- the listener card and every Settings switch's wiring.

**Mutation testing** (`cargo-mutants`, core crate tests only) on the five
security-critical files: `crypto.rs`, `session.rs`, `protocol.rs`,
`identity/mod.rs`, `framing.rs`. 475 mutants; 354 caught, 66 that do not
compile, 5 that hang a test (caught), 50 missed. Each miss was settled one
of three ways:
- **A real gap, closed with a test that fails under that mutant.** Examples:
  - the Ed25519 private-key export (pinned to RFC 8032);
  - each Argon2 cost cap on its own, so an edited `identity.json` cannot hang
    the unlock;
  - every wire-size cap at its exact edge (framing, legacy text, file size,
    number fields, scheme list);
  - the per-address connection limit actually being consulted;
  - the version on the wire and the v3 downgrade floor;
  - the weak-code warning reaching only uncommitted peers (CRYPTO-02);
  - when a rekey is due, and what counts toward it;
  - delivery-receipt sequence reporting;
  - bounded reading while a write blocks.

  Two existing tests were found unable to fail:
  - `test_framing_recv_reject_oversized_header` passed on end-of-stream;
  - `idle_session_survives_past_the_idle_timeout` never reached the short
    idle timer.
- **Equivalent**: no input can tell the mutant apart. Examples: `|` versus
  `^` on disjoint bits in `derive_sas` and the nonce role, and the read chunk
  size.
- **Not worth a test:** a constant whose test would only restate it (the
  30-minute verification window, the first keep-alive tick), an invite
  version field the parser never reads, and the memory and time Argon2 caps
  at exactly their limit (a 1 GiB / 16-pass derivation per run).

  Also untested: `migrate_legacy_plaintext` and the Ed25519 hex helpers, which
  have no caller (see below).

  **Re-run after the review fixes**, on the same five files: 477 mutants;
  392 caught, 68 that do not compile, 6 that hang a test (caught), **11
  missed**. All 11 are the ones classified above, and nothing else:
  - equivalent: `rsa_rng` → `Default`, `|` → `^` in `derive_sas` and
    `new_with_role`, and the read chunk size `*` → `+`;
  - not worth a test: the verification window constant, the first
    keep-alive tick, the initial idle deadline, the invite version field,
    and the memory and time Argon2 caps at exactly their limit;
  - no caller: `migrate_legacy_plaintext`.

**Left open on purpose.** QUAL-10 (the async `IncomingFile` receiver has no
caller in this workspace), `Identity::migrate_legacy_plaintext` and the
Ed25519 hex helpers are public API of `messenger-core`, and the agent CLI
lives in a sibling repository this pass could not inspect. Deleting them, and their
test, waits on checking that repository.

### Review of the 24 November PRs (#197–#213)

An independent review of the merged batch and of this branch found these,
each now fixed with a test seen failing without the fix:
- **NET-16 bypass (blocking):** an IPv4-mapped IPv6 candidate
  (`[::ffff:127.0.0.1]`) passed the punch filter and the dual-stack socket
  dialled the host's own loopback. Candidates are now judged and dialled as
  the IPv4 address they name.
- **CRYPTO-04 on the listener side:** a saved contact arriving with another
  key looked like a stranger. It now warns `ContactAddressMismatch` when the
  connection comes from that contact's address. The desktop dialog no longer
  reads a failed lookup as first use, and `get_conversation` resolves
  session ids.
- **CRYPTO-04 follow-ups:** a key change that also contradicts an invite
  says the contact will be repointed, and a Trusted contact stays Trusted.
- **Refused file messages** no longer leave a counted reference and bytes
  behind (QUAL-05 follow-up).
- **The production tunnel derivation** (`HKDF_INFO`, transcript order, AAD
  format) is pinned by a known answer computed independently.
- **Diagnostics** scrub IPv6 addresses and server host names.
- **The data-directory chmod** is a warning, not a refusal to start.
- **Test hooks** compile only under `cfg(test)` or a dev-only `test-support`
  feature.
- **Idle uploads** stop holding the spool budget after two minutes.
- **csv** asks before opening, and four more types count as able to run
  code.

Left as design decisions: whether accepting an `InviteMismatch` should rebind
the contact (question 1 of the review), and a fuller listener-side identity
model (question 2) beyond the address hint added here.

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
