# Findings index — November 2026 audit

Every finding from the thirteen area reports, sorted by severity. "Status" is the auditor's
(VERIFIED = traced through the code; SUSPECTED = plausible, needs confirmation); `Fixed` marks
findings already resolved since the audit. Effort: S < 1 h, M ≈ half a day, L = days.
Duplicates across reports are kept here (each report stands alone); the deduplicated,
prioritised view is in [README.md](README.md).

| ID | Severity | Status | Effort | Report | Finding |
|---|---|---|---|---|---|
| CRYPTO-01 | Critical | Fixed | S | [01-crypto-protocol](reports/01-crypto-protocol.md) | The fingerprint and the verified key come from two different PEM parsers, so a preamble spoofs any identity |
| JOURNEY-01 | Critical | VERIFIED | M | [08-user-journey-footprint](reports/08-user-journey-footprint.md) | An unreadable history file is silently replaced with an empty one right after unlock |
| SRV-01 | Critical | VERIFIED | M | [03-party-server](reports/03-party-server.md) | Frames built from history, broadcasts or listings can exceed `MAX_PACKET_SIZE`; one member can make the community permanently unjoinable |
| APP-01 | High | VERIFIED | M | [04-client-core](reports/04-client-core.md) | A failed history load is followed by a save that overwrites the history with empty state |
| CRYPTO-02 | High | VERIFIED | M | [01-crypto-protocol](reports/01-crypto-protocol.md) | The SAS has no commitment, so a MITM can grind its last ephemeral key until both SAS values match |
| DOC-01 | High | VERIFIED | S | [09-documentation](reports/09-documentation.md) | Community verification asks the user to compare a code the operator cannot see, and step 2 is not bound to step 1 |
| DOC-02 | High | VERIFIED | S | [09-documentation](reports/09-documentation.md) | "Settings → Delete Everything" and a Windows uninstaller data wipe are documented but do not exist |
| DOC-03 | High | VERIFIED | S | [09-documentation](reports/09-documentation.md) | THREAT_MODEL relies on an "operator can read messages" badge that is not implemented |
| DOC-04 | High | VERIFIED | M | [09-documentation](reports/09-documentation.md) | `protocol.md` says Party chunked transfer is not implemented and omits 9 shipped message variants |
| DOC-05 | High | VERIFIED | S | [09-documentation](reports/09-documentation.md) | Wrong server binary path in the build instructions |
| DOC-06 | High | VERIFIED | S | [09-documentation](reports/09-documentation.md) | The SAS short code is missing from the user verification instructions |
| DOC-07 | High | VERIFIED | S | [09-documentation](reports/09-documentation.md) | THREAT_MODEL says invite expiry is not enforced |
| DX-01 | High | VERIFIED | S | [10-dx-ci-tests](reports/10-dx-ci-tests.md) | Crypto dependencies compile unoptimised in dev/test: 95% of test time is debug RSA and Argon2 |
| DX-02 | High | VERIFIED | S | [10-dx-ci-tests](reports/10-dx-ci-tests.md) | The change classifier skips the frontend contract tests when only `desktop/src-tauri/` changes |
| DX-03 | High | VERIFIED | S | [10-dx-ci-tests](reports/10-dx-ci-tests.md) | `desktop` builds a staticlib and a cdylib it never uses (1.8 GB per profile, about 3.6 GB per build plus test), and debuginfo is full everywhere |
| JOURNEY-02 | High | VERIFIED | S | [08-user-journey-footprint](reports/08-user-journey-footprint.md) | TUI first run: Esc on "set password" runs the whole session on an identity that is never saved |
| JOURNEY-03 | High | VERIFIED | M | [08-user-journey-footprint](reports/08-user-journey-footprint.md) | No way to delete local data; documented "Delete Everything" and uninstaller cleanup do not exist; uninstall leaves identity and history behind |
| JOURNEY-04 | High | VERIFIED | S | [08-user-journey-footprint](reports/08-user-journey-footprint.md) | Users are told everything is end-to-end encrypted with no server copy, but community operators read all community traffic in plaintext |
| JOURNEY-05 | High | SUSPECTED | S | [08-user-journey-footprint](reports/08-user-journey-footprint.md) | Release: `tauri-action` is given `tagName` with a read-only token (pipeline break or attestation bypass) |
| NET-01 | High | VERIFIED | S | [02-network](reports/02-network.md) | Transport frames can be reflected back to their sender and are accepted as the peer's |
| NET-02 | High | VERIFIED | M | [02-network](reports/02-network.md) | The one-shot host listener lets one TCP connection make the host unreachable, repeatedly |
| PERF-01 | High | VERIFIED | M | [11-performance](reports/11-performance.md) | Desktop refresh: full conversation over IPC, then O(n) Intl formatting in JS, at 4 Hz during transfers |
| PERF-02 | High | VERIFIED | M | [11-performance](reports/11-performance.md) | One maximum-size peer text (about 24 MiB) is re-processed on every frame, refresh and save |
| PERF-03 | High | VERIFIED | S | [11-performance](reports/11-performance.md) | Whole-history persistence: O(total history) clone, serialize, encrypt and fsync per save; TUI saves every 30 s unconditionally |
| PERF-04 | High | VERIFIED | S | [11-performance](reports/11-performance.md) | Server SQLite: autocommit + DELETE journal + `synchronous=FULL` + no statement cache, all under the global lock |
| PERF-05 | High | VERIFIED | S | [11-performance](reports/11-performance.md) | Client community history merge is quadratic |
| PRIV-01 | High | VERIFIED | S | [13-privacy-supply-chain](reports/13-privacy-supply-chain.md) | The desktop UI never tells users the community server reads their messages and DMs, and THREAT_MODEL.md claims a badge that does not exist |
| QUAL-01 | High | Fixed | S | [12-code-quality](reports/12-code-quality.md) | Pasting a crafted v1 invite link crashes the app: string slice on unvalidated PEM |
| QUAL-02 | High | VERIFIED | S/M | [12-code-quality](reports/12-code-quality.md) | A failed history load leads to the encrypted history being overwritten with an empty one |
| SRV-02 | High | VERIFIED | S | [03-party-server](reports/03-party-server.md) | When the hub evicts a stuck connection, the socket stays open and the client goes deaf |
| SRV-03 | High | VERIFIED | M | [03-party-server](reports/03-party-server.md) | No idle, read or write timeouts and no concurrent-connection cap: pre-auth file-descriptor exhaustion, ghost presence, permanently blocked tasks |
| SRV-04 | High | VERIFIED | M | [03-party-server](reports/03-party-server.md) | Ownership goes to whoever connects first, and servers are open by default |
| SRV-05 | High | VERIFIED | S | [03-party-server](reports/03-party-server.md) | Users are asked to verify the server with a SAS the operator cannot see and that does not belong to the connection being trusted |
| SRV-06 | High | VERIFIED | L | [03-party-server](reports/03-party-server.md) | No per-member rate limits on posts, DMs, shares or permission changes; all history kept in RAM forever; no retention |
| TAURI-01 | High | VERIFIED | S | [06-tauri-bridge](reports/06-tauri-bridge.md) | The verified community fingerprint is not bound to the credential-sending connection |
| TAURI-30 | High | VERIFIED | S | [06-tauri-bridge](reports/06-tauri-bridge.md) | A history file that fails to decrypt or parse is silently overwritten with empty state after unlock |
| TUI-01 | High | VERIFIED | S | [05-tui](reports/05-tui.md) | The fingerprint overlay pops up under the user's fingers; `y`/`Enter` accept, `n`/`Esc` reject |
| TUI-02 | High | VERIFIED | S | [05-tui](reports/05-tui.md) | `:party-trust` does not pin the fingerprint the user verified (TOCTOU on community join) |
| TUI-03 | High | VERIFIED | M | [05-tui](reports/05-tui.md) | `:set-password` changes the password without the current one (bypasses `change_password`); reports success on partial failure |
| TUI-04 | High | VERIFIED | S | [05-tui](reports/05-tui.md) | Dismissing the first-run password prompt means the identity is never saved, so the user gets a new identity every launch and earlier history is overwritten |
| TUI-05 | High | VERIFIED | S | [05-tui](reports/05-tui.md) | A multi-line paste is executed as keystrokes: each line is sent, and lines starting with `:` run as commands |
| TUI-06 | High | VERIFIED | S | [05-tui](reports/05-tui.md) | The terminal is left in raw mode on the alternate screen on startup errors and panics, and the error message is invisible |
| TUI-07 | High | VERIFIED | L | [05-tui](reports/05-tui.md) | Community (Party) messages, DMs, the audit log and notices are never displayed; the TUI can post but never read |
| TUI-08 | High | VERIFIED | M | [05-tui](reports/05-tui.md) | Community connect and trust block the whole UI on a TCP connect with no timeout |
| UI-01 | High | VERIFIED | S | [07-react-frontend](reports/07-react-frontend.md) | Community first-join SAS check does not cover the connection that receives the credentials |
| UI-02 | High | VERIFIED | S | [07-react-frontend](reports/07-react-frontend.md) | A late conversation fetch can show A while the composer sends to B |
| APP-02 | Medium | VERIFIED | S | [04-client-core](reports/04-client-core.md) | `send_file` awaits the bounded file lane while the caller holds the `ChatManager` lock |
| APP-03 | Medium | VERIFIED | S | [04-client-core](reports/04-client-core.md) | A received file whose finalize fails leaves its spool, and possibly an empty placeholder, in Downloads |
| APP-04 | Medium | VERIFIED | S | [04-client-core](reports/04-client-core.md) | A local disk-write error on receive does not tell the sender to stop |
| APP-05 | Medium | VERIFIED | S | [04-client-core](reports/04-client-core.md) | A fully received offer awaiting the user's decision is destroyed when the peer disconnects |
| APP-06 | Medium | VERIFIED | M | [04-client-core](reports/04-client-core.md) | `FileCancel` carries no transfer id, so with transfers both ways it cancels the wrong one |
| APP-07 | Medium | VERIFIED | M | [04-client-core](reports/04-client-core.md) | Unaccepted, peer-controlled spools live in the user's Downloads folder and are never swept |
| APP-08 | Medium | VERIFIED | S | [04-client-core](reports/04-client-core.md) | Party history paging skips a range when a live broadcast arrives before the first page |
| APP-09 | Medium | VERIFIED | S | [04-client-core](reports/04-client-core.md) | Chunked-text reassembly is bounded by message count, not bytes (hundreds of MiB per chat) |
| APP-10 | Medium | VERIFIED | S | [04-client-core](reports/04-client-core.md) | Party `connect_and_join` has no TCP connect timeout, and the desktop holds the `PartyManager` lock across it |
| CRYPTO-03 | Medium | VERIFIED | S | [01-crypto-protocol](reports/01-crypto-protocol.md) | Both directions share one key and one AAD, and decrypt never checks the role bit, so frames can be reflected |
| CRYPTO-04 | Medium | VERIFIED | S | [01-crypto-protocol](reports/01-crypto-protocol.md) | An invite's fingerprint is never compared with the fingerprint presented on first connection |
| CRYPTO-05 | Medium | VERIFIED | M | [01-crypto-protocol](reports/01-crypto-protocol.md) | The connection password is sent to an unverified host |
| CRYPTO-06 | Medium | SUSPECTED | S | [01-crypto-protocol](reports/01-crypto-protocol.md) | A second rekey can overwrite the retained old key while old-key frames are still in flight |
| DOC-08 | Medium | VERIFIED | S | [09-documentation](reports/09-documentation.md) | The TUI `:set` key list, `settings_keys()` and help text all omit `upnp`, which the guide uses |
| DOC-09 | Medium | VERIFIED | S | [09-documentation](reports/09-documentation.md) | The USER_GUIDE TUI command reference omits about 16 commands |
| DOC-10 | Medium | VERIFIED | S | [09-documentation](reports/09-documentation.md) | USER_GUIDE describes community pinning wrongly, and the TUI never pins |
| DOC-11 | Medium | VERIFIED | S | [09-documentation](reports/09-documentation.md) | `SECURITY.md`'s accepted-advisory list is stale |
| DOC-12 | Medium | VERIFIED | S | [09-documentation](reports/09-documentation.md) | SECURITY.md and architecture.md understate frontend testing and miscount bridge tests |
| DOC-13 | Medium | VERIFIED | S | [09-documentation](reports/09-documentation.md) | `architecture.md` code map is stale after PR #188 |
| DOC-14 | Medium | VERIFIED | S | [09-documentation](reports/09-documentation.md) | `architecture.md` says history is rewritten once per message |
| DOC-15 | Medium | VERIFIED | S | [09-documentation](reports/09-documentation.md) | `CLAUDE.md` claims `connect_to_contact` has a relay fallback |
| DOC-16 | Medium | VERIFIED | S | [09-documentation](reports/09-documentation.md) | `CLAUDE.md` says chunked upload ids are client-chosen |
| DOC-17 | Medium | VERIFIED | S | [09-documentation](reports/09-documentation.md) | `CLAUDE.md` says unaccepted files do not reach the download directory |
| DOC-18 | Medium | VERIFIED | S | [09-documentation](reports/09-documentation.md) | `server_tiers.md` lists moderation powers that do not exist |
| DOC-19 | Medium | VERIFIED | S | [09-documentation](reports/09-documentation.md) | No single local command set matches CI; CONTRIBUTING, DEVELOPER_GUIDE, the PR template and README disagree |
| DOC-20 | Medium | VERIFIED | M | [09-documentation](reports/09-documentation.md) | `platform_spec.md` "Current State" was verified at 1.15.0 and contradicts itself |
| DOC-21 | Medium | VERIFIED | M | [09-documentation](reports/09-documentation.md) | `CLAUDE.md` is 56 KB and breaks its own size rule |
| DOC-26 | Medium | VERIFIED | S | [09-documentation](reports/09-documentation.md) | Backup and restore is incomplete, and migrating from Classic is undocumented |
| DOC-27 | Medium | VERIFIED | M | [09-documentation](reports/09-documentation.md) | No operator guide for the community server or relay |
| DOC-28 | Medium | VERIFIED | M | [09-documentation](reports/09-documentation.md) | No privacy / metadata page |
| DOC-29 | Medium | VERIFIED | M | [09-documentation](reports/09-documentation.md) | THREAT_MODEL is missing major attack surfaces |
| DX-04 | Medium | VERIFIED | S | [10-dx-ci-tests](reports/10-dx-ci-tests.md) | `image` builds with all default codecs (AVIF/rav1e, EXR, TIFF…) to write one PNG; this re-creates an advisory acceptance and contradicts `deny.toml` |
| DX-05 | Medium | VERIFIED | S | [10-dx-ci-tests](reports/10-dx-ci-tests.md) | World-state checks gate every PR: advisories, yanked crates and floating-stable clippy |
| DX-06 | Medium | VERIFIED | S | [10-dx-ci-tests](reports/10-dx-ci-tests.md) | Redundant compilation: `build-verification` duplicates what the Linux test leg already compiles |
| DX-07 | Medium | VERIFIED | S | [10-dx-ci-tests](reports/10-dx-ci-tests.md) | The frontend job runs everything on three operating systems |
| DX-08 | Medium | SUSPECTED | S | [10-dx-ci-tests](reports/10-dx-ci-tests.md) | Rust cache pressure: about 12 separate rust-cache entries, saved from every PR branch |
| DX-09 | Medium | VERIFIED | S | [10-dx-ci-tests](reports/10-dx-ci-tests.md) | No single local command equivalent to CI; the four documented check lists disagree |
| DX-10 | Medium | VERIFIED | S | [10-dx-ci-tests](reports/10-dx-ci-tests.md) | Secret scan: the CI copy installs gitleaks unverified, and PRs rescan the whole history every time |
| DX-11 | Medium | SUSPECTED | S | [10-dx-ci-tests](reports/10-dx-ci-tests.md) | `release.yml`: tauri-action is given `tagName`, `releaseName` and a token under a read-only permission block |
| JOURNEY-06 | Medium | VERIFIED | M | [08-user-journey-footprint](reports/08-user-journey-footprint.md) | Backup covers the identity only; no restore/import; the "Backed up" badge is weak and goes stale after a password change |
| JOURNEY-07 | Medium | VERIFIED | S | [08-user-journey-footprint](reports/08-user-journey-footprint.md) | Desktop notifications put message plaintext into OS notification stores and on the lock screen |
| JOURNEY-08 | Medium | VERIFIED | S | [08-user-journey-footprint](reports/08-user-journey-footprint.md) | No single-instance guard: two desktop instances share one identity and race on the history file |
| JOURNEY-09 | Medium | SUSPECTED | S | [08-user-journey-footprint](reports/08-user-journey-footprint.md) | Release: both macOS legs may stage an identically named `P2PEM.app.tar.gz`, which the collision check rejects |
| JOURNEY-10 | Medium | VERIFIED | M | [08-user-journey-footprint](reports/08-user-journey-footprint.md) | The desktop release build has no logs or crash reports, and the diagnostics bundle is almost empty |
| JOURNEY-11 | Medium | VERIFIED | M | [08-user-journey-footprint](reports/08-user-journey-footprint.md) | `chat-p2p://` invite links are not registered with the OS, and the IPs they embed are not disclosed |
| JOURNEY-12 | Medium | VERIFIED | S | [08-user-journey-footprint](reports/08-user-journey-footprint.md) | Unaccepted file offers are spooled into the user's Downloads folder and orphaned after a crash |
| JOURNEY-14 | Medium | VERIFIED | S | [08-user-journey-footprint](reports/08-user-journey-footprint.md) | Unsigned installers ship with no install walkthrough |
| NET-03 | Medium | SUSPECTED | S | [02-network](reports/02-network.md) | Two host rekeys can happen before the client switches, after which old-key frames are undecryptable |
| NET-04 | Medium | VERIFIED | S | [02-network](reports/02-network.md) | No `TCP_NODELAY` and a two-write frame cause delayed-ACK stalls on every small message |
| NET-05 | Medium | VERIFIED | M | [02-network](reports/02-network.md) | Unauthenticated peers can create unlimited TOFU prompts, 30-minute sessions and junk conversations |
| NET-06 | Medium | VERIFIED | S | [02-network](reports/02-network.md) | A relay host that disconnects while waiting keeps its token slot for the full 5 minutes |
| NET-07 | Medium | VERIFIED | S | [02-network](reports/02-network.md) | No per-address cap on pending rendezvous slots; a few IPs can fill the global table |
| NET-08 | Medium | VERIFIED | S | [02-network](reports/02-network.md) | The punch hello tag is sent before the peer proves anything, so any stranger can reflect it and claim the punched socket |
| NET-09 | Medium | VERIFIED | M | [02-network](reports/02-network.md) | The relay control channel is cleartext: token, LAN IPs and pairing are visible and spoofable on path |
| NET-10 | Medium | VERIFIED | S/M | [02-network](reports/02-network.md) | Confirming the SAS more than 5 minutes apart kills the session |
| PERF-06 | Medium | VERIFIED | S | [11-performance](reports/11-performance.md) | Server history is fully resident; paging scans from message 1; startup re-parses everything |
| PERF-07 | Medium | VERIFIED | M | [11-performance](reports/11-performance.md) | P2P receive path has no backpressure; chunk writes are blocking I/O under the manager lock |
| PERF-08 | Medium | VERIFIED | M | [11-performance](reports/11-performance.md) | Server upload spools held in RAM: about 0.5 GiB per member, not bounded server-wide |
| PERF-09 | Medium | VERIFIED | M | [11-performance](reports/11-performance.md) | Desktop poll loop: unconditional 4 Hz wakeups, each walking every message twice under the lock |
| PERF-10 | Medium | VERIFIED | S | [11-performance](reports/11-performance.md) | Nagle plus delayed ACK on direct-P2P and Party connections (latency model) |
| PERF-11 | Medium | VERIFIED | S | [11-performance](reports/11-performance.md) | `image` default features compile AV1, EXR, TIFF, WebP and rayon only to encode one PNG |
| PERF-12 | Medium | VERIFIED | S | [11-performance](reports/11-performance.md) | Argon2, RSA and history load run synchronously on async workers and hold locks; no optimized dev profile for crypto |
| PRIV-02 | Medium | VERIFIED | M | [13-privacy-supply-chain](reports/13-privacy-supply-chain.md) | The same long-term identity key authenticates the user to every community server, so operators can link users across communities and to their P2P identity |
| PRIV-03 | Medium | VERIFIED | M | [13-privacy-supply-chain](reports/13-privacy-supply-chain.md) | Relay hole punching discloses each peer's public and LAN addresses to whoever holds the token, before verification, and the relay control channel is plaintext |
| PRIV-04 | Medium | VERIFIED | S | [13-privacy-supply-chain](reports/13-privacy-supply-chain.md) | The TUI log buffer and "Export diagnostics" bundle capture relay tokens, peer IPs, file names and paths, contact names and the external IP, under a README that implies the bundle is safe to share |
| PRIV-05 | Medium | SUSPECTED | S | [13-privacy-supply-chain](reports/13-privacy-supply-chain.md) | Desktop debug logging (filenames, peer addresses, frame metadata) goes to stdout, which on Linux plausibly persists in the systemd journal |
| PRIV-06 | Medium | VERIFIED | S | [13-privacy-supply-chain](reports/13-privacy-supply-chain.md) | Desktop notifications show message text by default with no "hide content" option |
| PRIV-09 | Medium | VERIFIED | S | [13-privacy-supply-chain](reports/13-privacy-supply-chain.md) | The community server writes plaintext `party.db` and `blobs/` under the default umask, and server logs record member IPs at info |
| QUAL-03 | Medium | VERIFIED | M | [12-code-quality](reports/12-code-quality.md) | Session runners take 8–9 positional arguments; `SessionChannels` exists but stays client-private |
| QUAL-04 | Medium | VERIFIED | L | [12-code-quality](reports/12-code-quality.md) | Errors are untyped across every module boundary |
| QUAL-05 | Medium | VERIFIED | M | [12-code-quality](reports/12-code-quality.md) | Server persistence logs and ignores every error and has no transactions or schema versioning |
| QUAL-06 | Medium | VERIFIED | S | [12-code-quality](reports/12-code-quality.md) | The TUI log buffer drops every structured field, so the overlay and diagnostics lose the actual error |
| QUAL-07 | Medium | VERIFIED | S/M | [12-code-quality](reports/12-code-quality.md) | The shipped desktop app has no persistent log, no panic hook, and unused logging dependencies |
| QUAL-08 | Medium | VERIFIED | M | [12-code-quality](reports/12-code-quality.md) | The desktop app compiles the terminal UI; `ChatManager` is not in a UI-agnostic crate |
| QUAL-09 | Medium | VERIFIED | S/M | [12-code-quality](reports/12-code-quality.md) | 36 public functions have no production caller; test helpers are public API |
| QUAL-10 | Medium | VERIFIED | S | [12-code-quality](reports/12-code-quality.md) | A complete second file receiver (async `IncomingFile`) is dead and has already drifted |
| QUAL-11 | Medium | VERIFIED | S/M | [12-code-quality](reports/12-code-quality.md) | Stringly-typed seams: transport carried as a `"p2p:"`/`"relay:"` prefix; roles and kinds mapped twice |
| QUAL-12 | Medium | VERIFIED | S | [12-code-quality](reports/12-code-quality.md) | Swallowed results that hide real failures (judged list) |
| QUAL-13 | Medium | VERIFIED | M | [12-code-quality](reports/12-code-quality.md) | `ChatManager` is a god object with 11 public mutable fields; validation happens only at load time |
| SRV-07 | Medium | VERIFIED | M | [03-party-server](reports/03-party-server.md) | Chunked-upload spools live in RAM, bounded only per member (4 × 100 MiB) and outside every quota |
| SRV-08 | Medium | VERIFIED | S | [03-party-server](reports/03-party-server.md) | Moderation is largely missing, and a Guest can still send DMs |
| SRV-09 | Medium | VERIFIED | S-M | [03-party-server](reports/03-party-server.md) | Any member can push admin actions out of the visible audit log; the audit table grows without bound |
| SRV-10 | Medium | VERIFIED | M | [03-party-server](reports/03-party-server.md) | Multi-row operations are not transactional and refcounts are not reconciled on load: crashes leak blobs or delete files still in use |
| SRV-11 | Medium | VERIFIED | S | [03-party-server](reports/03-party-server.md) | `party.db` and blobs (plaintext channels and DMs) are created with default permissions |
| SRV-12 | Medium | VERIFIED | S | [03-party-server](reports/03-party-server.md) | Every mutation is an autocommit fsync made while the global state lock is held; no WAL |
| SRV-13 | Medium | VERIFIED | M | [03-party-server](reports/03-party-server.md) | O(n) and O(n·m) work per request while holding the global lock |
| SRV-14 | Medium | VERIFIED | S | [03-party-server](reports/03-party-server.md) | `DownloadFile` reads a whole blob (up to 100 MiB) into memory and then fails to send it |
| SRV-15 | Medium | VERIFIED | M | [03-party-server](reports/03-party-server.md) | Username uniqueness is ASCII case-only; homoglyphs and non-ASCII case variants allow impersonation |
| SRV-16 | Medium | VERIFIED | M | [03-party-server](reports/03-party-server.md) | Per-IP connection limit silently refuses groups behind one NAT; password guessing scales with source addresses |
| SUPPLY-01 | Medium | VERIFIED | S | [13-privacy-supply-chain](reports/13-privacy-supply-chain.md) | The process gap: new RustSec advisories break unrelated Rust PRs, frontend PRs merge green past them, and the daily audit only files an issue |
| SUPPLY-02 | Medium | SUSPECTED | S | [13-privacy-supply-chain](reports/13-privacy-supply-chain.md) | tauri-action is given `tagName`/`releaseName`, which makes it try to create or upload to the GitHub release, contradicting the attest-then-publish design |
| TAURI-02 | Medium | VERIFIED | S | [06-tauri-bridge](reports/06-tauri-bridge.md) | `set_password` re-keys an unlocked, already-protected identity without the current password; `unlock` is re-entrant |
| TAURI-03 | Medium | VERIFIED | M | [06-tauri-bridge](reports/06-tauri-bridge.md) | One `connection_password` serves both hosting and dialling: the host password leaks to dialled peers and is lost on auto-rehost |
| TAURI-04 | Medium | VERIFIED | M | [06-tauri-bridge](reports/06-tauri-bridge.md) | `party_join` holds the party mutex across an untimed TCP connect and handshake, freezing the whole poll loop |
| TAURI-05 | Medium | VERIFIED | S | [06-tauri-bridge](reports/06-tauri-bridge.md) | No single-instance guard: two app instances race on one identity, history file and port |
| TAURI-06 | Medium | VERIFIED | M | [06-tauri-bridge](reports/06-tauri-bridge.md) | Received files carry no Mark-of-the-Web or quarantine attribute, and the "executes on open" list has gaps |
| TAURI-07 | Medium | VERIFIED | S | [06-tauri-bridge](reports/06-tauri-bridge.md) | Argon2 runs on async runtime workers under a std mutex; sync commands block the main thread on it |
| TAURI-08 | Medium | VERIFIED | S | [06-tauri-bridge](reports/06-tauri-bridge.md) | mDNS keeps advertising and browsing after the user turns it off or hosting stops |
| TAURI-09 | Medium | VERIFIED | M | [06-tauri-bridge](reports/06-tauri-bridge.md) | Every `state-updated` re-serializes the whole open conversation, up to 4×/s during transfers |
| TAURI-31 | Medium | VERIFIED | S | [06-tauri-bridge](reports/06-tauri-bridge.md) | `export_identity` can destroy the identity it is backing up, and records "backed up" for any destination |
| TUI-09 | Medium | VERIFIED | S | [05-tui](reports/05-tui.md) | Multi-line messages have their newlines removed when rendered, and scroll height is miscomputed |
| TUI-10 | Medium | VERIFIED | S | [05-tui](reports/05-tui.md) | CLI launch flags are dropped for a password-protected identity; `--port` is ignored by `--connect`; no flag validation |
| TUI-11 | Medium | VERIFIED | S | [05-tui](reports/05-tui.md) | Passwords are typed in clear on the command line, kept in command history, and truncated at the first space |
| TUI-12 | Medium | VERIFIED | S | [05-tui](reports/05-tui.md) | Setting a password has no confirmation field and no length hint |
| TUI-13 | Medium | VERIFIED | M | [05-tui](reports/05-tui.md) | Blocking work runs on the UI loop: Argon2, full-history save every 30 s, whole-file read for community upload; failures are silent |
| TUI-14 | Medium | VERIFIED | M | [05-tui](reports/05-tui.md) | Each frame does O(all messages) work, and the loop handles one input event per frame |
| TUI-15 | Medium | VERIFIED | S | [05-tui](reports/05-tui.md) | `:disconnect` deletes the conversation and its history; deletions have no confirmation |
| TUI-16 | Medium | VERIFIED | M | [05-tui](reports/05-tui.md) | The Party pane's community selection does not change which community commands act on; posts always go to the first channel |
| TUI-17 | Medium | VERIFIED | M | [05-tui](reports/05-tui.md) | The TUI never pins community identities, so every join is first contact; docs say the app pins |
| TUI-18 | Medium | VERIFIED | S | [05-tui](reports/05-tui.md) | The selected chat is marked read every tick, even while the user is away |
| TUI-19 | Medium | VERIFIED | M | [05-tui](reports/05-tui.md) | Settings that do nothing in the TUI, and features missing compared with the desktop app |
| TUI-20 | Medium | VERIFIED | M | [05-tui](reports/05-tui.md) | The composer and command box do not scroll, and cursor placement ignores soft wrap |
| TUI-21 | Medium | VERIFIED | M | [05-tui](reports/05-tui.md) | Colours are hard-coded: security text is white (invisible on light themes), no `NO_COLOR`, RGB safety grid |
| TUI-22 | Medium | VERIFIED | S | [05-tui](reports/05-tui.md) | Unit tests touch the developer's real data directory; the TUI has no data-dir override |
| TUI-23 | Medium | VERIFIED | S | [05-tui](reports/05-tui.md) | `:invite` advertises the configured listen port, not the port actually being hosted |
| TUI-24 | Medium | VERIFIED | M | [05-tui](reports/05-tui.md) | No log output anywhere (relay server is silent; `RUST_LOG` has no visible effect); docs reference a log view that does not exist |
| UI-03 | Medium | VERIFIED | S | [07-react-frontend](reports/07-react-frontend.md) | Communities composer draft is shared across channels, DMs and servers |
| UI-04 | Medium | VERIFIED | S | [07-react-frontend](reports/07-react-frontend.md) | Stale Communities history responses can render under the wrong channel header |
| UI-05 | Medium | VERIFIED | S/M | [07-react-frontend](reports/07-react-frontend.md) | Community read marks never survive a restart or rejoin |
| UI-06 | Medium | VERIFIED | M | [07-react-frontend](reports/07-react-frontend.md) | Whole-conversation refetch on every UI tick, up to 4 Hz during transfers |
| UI-07 | Medium | VERIFIED | S | [07-react-frontend](reports/07-react-frontend.md) | The invite link is fetched once and cached for the whole session |
| UI-08 | Medium | VERIFIED | S | [07-react-frontend](reports/07-react-frontend.md) | Modal focus management overrides every `autoFocus` inside a dialog |
| UI-09 | Medium | SUSPECTED | S | [07-react-frontend](reports/07-react-frontend.md) | Toast live regions are `display:none` when empty, so the first announcement may be lost |
| UI-10 | Medium | VERIFIED | S | [07-react-frontend](reports/07-react-frontend.md) | `--faint` text fails WCAG AA contrast in every theme |
| UI-11 | Medium | VERIFIED | S | [07-react-frontend](reports/07-react-frontend.md) | Peer-supplied text is not bidi-isolated; linked URLs can display spoofed |
| UI-12 | Medium | SUSPECTED | M | [07-react-frontend](reports/07-react-frontend.md) | The file-permission dialog does not reflect the current grant, so Save can revoke rights |
| QUAL-14 | Low/Medium | VERIFIED | M | [12-code-quality](reports/12-code-quality.md) | Logic duplicated between the TUI and the desktop shell, already drifting |
| QUAL-19 | Low/Medium | VERIFIED | M | [12-code-quality](reports/12-code-quality.md) | Test fixtures copy-pasted across test binaries; port allocation is racy |
| APP-11 | Low | VERIFIED | S | [04-client-core](reports/04-client-core.md) | After a Party disconnect, unconfirmed sends stay on screen and upload offers stay in memory |
| APP-12 | Low | VERIFIED | M | [04-client-core](reports/04-client-core.md) | Messages typed during the handshake return `Ok` and can vanish without an error |
| APP-15 | Low | VERIFIED | S | [04-client-core](reports/04-client-core.md) | An unsigned (v1) invite can silently overwrite an existing, even Verified, contact's address and name |
| APP-16 | Low | VERIFIED | S | [04-client-core](reports/04-client-core.md) | `clear_history` / `delete_all_data` leave `is_hosting`, `hosting_port` and UPnP state set, so auto-rehost restarts the listener |
| APP-17 | Low | VERIFIED | S | [04-client-core](reports/04-client-core.md) | `delete_chat` leaves `contact_to_chat` entries pointing at the deleted chat |
| APP-18 | Low | VERIFIED | S | [04-client-core](reports/04-client-core.md) | Blocking a contact does not cover a fingerprint-less contact or a pending TOFU prompt |
| APP-19 | Low | VERIFIED | S | [04-client-core](reports/04-client-core.md) | The `awaiting_ack` cap clears every conversation's pending receipts at once |
| APP-20 | Low | VERIFIED | S | [04-client-core](reports/04-client-core.md) | Received timestamps are inconsistent and peer-controlled for large messages |
| APP-21 | Low | VERIFIED | M | [04-client-core](reports/04-client-core.md) | Blocking file I/O on the manager lock and runtime worker during receive and finalize |
| APP-22 | Low | VERIFIED | M | [04-client-core](reports/04-client-core.md) | O(n) scans per event and whole-history clones per save |
| APP-23 | Low | VERIFIED | S | [04-client-core](reports/04-client-core.md) | Concurrent downloads of the same Party file collide |
| CRYPTO-07 | Low | VERIFIED | S | [01-crypto-protocol](reports/01-crypto-protocol.md) | Signature-scheme negotiation is not covered by the transcript or signatures |
| CRYPTO-08 | Low | VERIFIED | S | [01-crypto-protocol](reports/01-crypto-protocol.md) | A malformed `nonce` in `identity.json` panics during unlock |
| CRYPTO-09 | Low | VERIFIED | M | [01-crypto-protocol](reports/01-crypto-protocol.md) | The history file has no AAD, no anti-rollback, and random nonces under one fixed key |
| CRYPTO-10 | Low | VERIFIED | S | [01-crypto-protocol](reports/01-crypto-protocol.md) | Gaps in key-material hygiene |
| CRYPTO-11 | Low | VERIFIED | S | [01-crypto-protocol](reports/01-crypto-protocol.md) | Packets before authentication may be up to 8 MiB |
| CRYPTO-12 | Low | VERIFIED | S | [01-crypto-protocol](reports/01-crypto-protocol.md) | `has_hidden_characters` and `is_forbidden_char` miss several invisible and direction-changing characters |
| CRYPTO-13 | Low | SUSPECTED | S | [01-crypto-protocol](reports/01-crypto-protocol.md) | The Windows reserved-name list is incomplete |
| CRYPTO-14 | Low | VERIFIED | S | [01-crypto-protocol](reports/01-crypto-protocol.md) | Peer identity keys have no minimum strength |
| CRYPTO-16 | Low | VERIFIED | S | [01-crypto-protocol](reports/01-crypto-protocol.md) | Security docs disagree with the code |
| DOC-22 | Low | VERIFIED | S | [09-documentation](reports/09-documentation.md) | `CLAUDE.md` tool rules give commands that do not work, and its layout note is stale |
| DOC-23 | Low | VERIFIED | S | [09-documentation](reports/09-documentation.md) | `CLAUDE.md` Tauri bridge section names the wrong file and lists about half the commands |
| DOC-24 | Low | VERIFIED | S | [09-documentation](reports/09-documentation.md) | `design/tokens.json` points to the deleted egui module |
| DOC-25 | Low | VERIFIED | S | [09-documentation](reports/09-documentation.md) | `desktop/src-tauri/Cargo.toml` tech-debt note is stale |
| DOC-30 | Low | VERIFIED | S | [09-documentation](reports/09-documentation.md) | `SECURITY.md` contradicts itself on Ed25519 |
| DOC-31 | Low | VERIFIED | M | [09-documentation](reports/09-documentation.md) | USER_GUIDE leaves out user-visible desktop features and safety behaviour |
| DOC-32 | Low | VERIFIED | S | [09-documentation](reports/09-documentation.md) | Smaller USER_GUIDE inaccuracies |
| DOC-33 | Low | VERIFIED | S | [09-documentation](reports/09-documentation.md) | DESIGN_NOTES "Current Gaps" is stale |
| DOC-34 | Low | VERIFIED | S | [09-documentation](reports/09-documentation.md) | Superseded entries in AUDITS.md are not marked |
| DOC-35 | Low | VERIFIED | S | [09-documentation](reports/09-documentation.md) | Bug-report template is out of date |
| DOC-36 | Low | VERIFIED | S | [09-documentation](reports/09-documentation.md) | CHANGELOG: security fixes have sat unreleased for 2.5 months |
| DOC-37 | Low | VERIFIED | S | [09-documentation](reports/09-documentation.md) | Contributor docs miss per-OS setup, fuzzing and CodeQL |
| DX-12 | Low | VERIFIED | M | [10-dx-ci-tests](reports/10-dx-ci-tests.md) | Eighteen client integration-test crates: about 4.1 GB and 17 extra links per test build |
| DX-13 | Low | VERIFIED | S | [10-dx-ci-tests](reports/10-dx-ci-tests.md) | Duplicated 20-line apt block ×5 in `ci.yml` and ×1 in `release.yml`, already drifting |
| DX-14 | Low | VERIFIED | S | [10-dx-ci-tests](reports/10-dx-ci-tests.md) | Test hooks leak process-global environment variables and are compiled into release builds; the documented `cargo test` fallback relies on luck |
| DX-15 | Low | VERIFIED | S | [10-dx-ci-tests](reports/10-dx-ci-tests.md) | `run-codeql-local.ps1` does not reproduce CI's CodeQL |
| DX-16 | Low | VERIFIED | S | [10-dx-ci-tests](reports/10-dx-ci-tests.md) | Stale or misleading comments that steer build decisions |
| JOURNEY-13 | Low | VERIFIED | S | [08-user-journey-footprint](reports/08-user-journey-footprint.md) | The UPnP / NAT-PMP mapping is not removed when the app exits |
| JOURNEY-15 | Low | VERIFIED | M | [08-user-journey-footprint](reports/08-user-journey-footprint.md) | No update mechanism or update notification |
| JOURNEY-16 | Low | SUSPECTED | S | [08-user-journey-footprint](reports/08-user-journey-footprint.md) | The history flush on close may be skipped on macOS Quit and on session logout |
| JOURNEY-17 | Low | VERIFIED | S | [08-user-journey-footprint](reports/08-user-journey-footprint.md) | Deleting a conversation does not mention received files, which stay on disk |
| JOURNEY-18 | Low | VERIFIED | S/M | [08-user-journey-footprint](reports/08-user-journey-footprint.md) | Metadata stored in plaintext outside the encrypted history is not documented |
| JOURNEY-19 | Low | VERIFIED | S | [08-user-journey-footprint](reports/08-user-journey-footprint.md) | The first host triggers an OS firewall prompt the app never explains; the listener is IPv4-only on every interface |
| JOURNEY-20 | Low | VERIFIED | S | [08-user-journey-footprint](reports/08-user-journey-footprint.md) | Server and tools defaults: relative data dir, password visible in process list, no Intel macOS tools |
| JOURNEY-21 | Low | VERIFIED | S | [08-user-journey-footprint](reports/08-user-journey-footprint.md) | Relative-path fallback when no data directory resolves |
| NET-11 | Low | VERIFIED | S | [02-network](reports/02-network.md) | The connection password is silently ignored for relay-hosted sessions |
| NET-12 | Low | VERIFIED | S | [02-network](reports/02-network.md) | `connect_to_contact` never falls back to the relay when direct addresses fail |
| NET-13 | Low | VERIFIED | S | [02-network](reports/02-network.md) | A UPnP / NAT-PMP mapping that fails the routable check is created and then leaked |
| NET-14 | Low | VERIFIED | S | [02-network](reports/02-network.md) | Client-side relay I/O has no timeouts |
| NET-15 | Low | SUSPECTED | M | [02-network](reports/02-network.md) | Punch outcome is decided per client, not by the server, so split verdicts break both peers |
| NET-16 | Low | VERIFIED | S | [02-network](reports/02-network.md) | Peer-supplied punch candidates include loopback and link-local; the host dials them |
| NET-17 | Low | VERIFIED | S | [02-network](reports/02-network.md) | Punch listener spawns detached, unbounded validation tasks |
| NET-18 | Low | VERIFIED | S | [02-network](reports/02-network.md) | mDNS advertises the user's display name and machine hostname; peer names are unsanitised |
| NET-19 | Low | VERIFIED | S | [02-network](reports/02-network.md) | Desktop defaults to `messenger_core=debug`, logging every frame |
| PERF-13 | Low | VERIFIED | S | [11-performance](reports/11-performance.md) | Release builds log every frame at debug level |
| PERF-14 | Low | VERIFIED | S | [11-performance](reports/11-performance.md) | Finished outgoing transfers are never pruned; every tick clones all of them |
| PERF-15 | Low | VERIFIED | M | [11-performance](reports/11-performance.md) | Hub fan-out deep-clones and re-serializes per connection; member directory broadcast is O(N²) on join storms |
| PERF-16 | Low | VERIFIED | M | [11-performance](reports/11-performance.md) | Image previews as base64 `data:` URLs over JSON IPC |
| PERF-17 | Low | VERIFIED | S | [11-performance](reports/11-performance.md) | React memoisation defeated by identity churn; whole-App recompute on every keystroke |
| PERF-18 | Low | VERIFIED | S | [11-performance](reports/11-performance.md) | Community downloads are assembled whole in RAM and hashed in one pass under the party lock |
| PERF-19 | Low | VERIFIED | S | [11-performance](reports/11-performance.md) | Rate limiter does a full `retain` over up to 4096 entries on every accept |
| PERF-20 | Low | VERIFIED | S | [11-performance](reports/11-performance.md) | Per-frame copy and realloc chain on the hot transfer path |
| PERF-21 | Low | VERIFIED | S | [11-performance](reports/11-performance.md) | Chunked-text reassembly bounded by count, not bytes (cited; cost added) |
| PERF-22 | Low | VERIFIED | S | [11-performance](reports/11-performance.md) | `party_dm_history` / `party_history` ship whole threads on every `party-updated`, with a double clone for DMs |
| PERF-23 | Low | VERIFIED | M | [11-performance](reports/11-performance.md) | TUI redraws at 10 fps when idle and does O(history) per frame |
| PRIV-07 | Low | VERIFIED | S | [13-privacy-supply-chain](reports/13-privacy-supply-chain.md) | The invite link silently embeds the LAN IP, the external IP and a creation timestamp, and is pushed through the clipboard, which is never cleared |
| PRIV-08 | Low | VERIFIED | M | [13-privacy-supply-chain](reports/13-privacy-supply-chain.md) | Zeroize coverage stops at the PEM and the AEAD keys: the history key, passwords and decrypted history are never wiped, and there is no swap protection |
| PRIV-10 | Low | VERIFIED | S | [13-privacy-supply-chain](reports/13-privacy-supply-chain.md) | `parties.json` stores the user's community list in plaintext, outside the encrypted history |
| PRIV-11 | Low | VERIFIED | S | [13-privacy-supply-chain](reports/13-privacy-supply-chain.md) | Delivery receipts cannot be disabled, and typing indicators default to on |
| QUAL-15 | Low | VERIFIED | S | [12-code-quality](reports/12-code-quality.md) | Inconsistent mutex-poisoning policy: the server hub panics, the desktop recovers |
| QUAL-16 | Low | SUSPECTED | S | [12-code-quality](reports/12-code-quality.md) | `#![allow(dead_code)]` on the whole server state module is stale |
| QUAL-17 | Low | VERIFIED | M | [12-code-quality](reports/12-code-quality.md) | Typestate expressed as `Option` plus `expect` |
| QUAL-18 | Low | VERIFIED | S | [12-code-quality](reports/12-code-quality.md) | Logging of identifying metadata at info and debug, with debug on by default |
| QUAL-20 | Low | VERIFIED | S/M | [12-code-quality](reports/12-code-quality.md) | Synchronization by sleeping, absence checks bounded by time, and an empty test |
| QUAL-21 | Low | VERIFIED | S | [12-code-quality](reports/12-code-quality.md) | Persisted config fields with no consumer; a security-relevant knob nobody can set |
| QUAL-22 | Low | VERIFIED | S | [12-code-quality](reports/12-code-quality.md) | Diagnostics and crash dir hard-coded to the TUI's ProjectDirs |
| QUAL-24 | Low | VERIFIED | S | [12-code-quality](reports/12-code-quality.md) | Cargo manifests: unused dependencies, broad features, stale metadata |
| QUAL-25 | Low | VERIFIED | M | [12-code-quality](reports/12-code-quality.md) | Long functions concentrated in protocol state machines |
| SRV-17 | Low | VERIFIED | S | [03-party-server](reports/03-party-server.md) | Presence race between a member's leaving and joining connections |
| SRV-18 | Low | VERIFIED | S | [03-party-server](reports/03-party-server.md) | Live delivery order does not follow seq order, and fan-out waits on the poster's own socket |
| SRV-19 | Low | VERIFIED | M | [03-party-server](reports/03-party-server.md) | Operator experience gaps |
| SRV-20 | Low | VERIFIED | S | [03-party-server](reports/03-party-server.md) | Upgrade bootstrap hands Owner to the member with the lowest UUID |
| SRV-21 | Low | VERIFIED | S | [03-party-server](reports/03-party-server.md) | Quota and ceiling edge cases |
| SRV-22 | Low | VERIFIED | S | [03-party-server](reports/03-party-server.md) | Stale or incorrect comments and docs |
| SRV-23 | Low | VERIFIED | S | [03-party-server](reports/03-party-server.md) | Every broadcast is cloned and serialized once per connection, inside the hub's std `Mutex` |
| SUPPLY-03 | Low | VERIFIED | S | [13-privacy-supply-chain](reports/13-privacy-supply-chain.md) | Dependabot does not cover GitHub Actions or the fuzz workspace, and the fuzz lockfile is git-ignored |
| SUPPLY-04 | Low | VERIFIED | S | [13-privacy-supply-chain](reports/13-privacy-supply-chain.md) | CI's secret-scan job installs gitleaks with no checksum (`curl \| sudo tar`) |
| SUPPLY-05 | Low | VERIFIED | S | [13-privacy-supply-chain](reports/13-privacy-supply-chain.md) | The committed `desktop/dist` is effectively unreviewable in PRs, and the staleness check ignores whitespace |
| SUPPLY-06 | Low | VERIFIED | M | [13-privacy-supply-chain](reports/13-privacy-supply-chain.md) | The accepted-advisory list has no structured reasons or expiry; RUSTSEC-2023-0071's rationale is narrower than the advisory; 55 crates are duplicated, including two RustCrypto generations |
| SUPPLY-07 | Low | VERIFIED | S | [13-privacy-supply-chain](reports/13-privacy-supply-chain.md) | Unused `tauri-plugin-log` and `log` dependencies add build-script and transitive surface |
| SUPPLY-08 | Low | VERIFIED | S | [13-privacy-supply-chain](reports/13-privacy-supply-chain.md) | Dev-only npm advisories (2 high, 8 moderate), and release builds run npm lifecycle scripts |
| TAURI-10 | Low | SUSPECTED | S | [06-tauri-bridge](reports/06-tauri-bridge.md) | Shutdown flush only covers `CloseRequested`; concurrent saves can land an older snapshot last |
| TAURI-11 | Low | SUSPECTED | S | [06-tauri-bridge](reports/06-tauri-bridge.md) | `open_url` validates by string prefix and launches through `rundll32 url.dll,FileProtocolHandler` on Windows |
| TAURI-12 | Low | VERIFIED | S | [06-tauri-bridge](reports/06-tauri-bridge.md) | `core:default` grants the webview more core IPC than it uses |
| TAURI-13 | Low | SUSPECTED | S | [06-tauri-bridge](reports/06-tauri-bridge.md) | No navigation or new-window guard on the main webview |
| TAURI-14 | Low | VERIFIED | M | [06-tauri-bridge](reports/06-tauri-bridge.md) | Poll-loop cost: two full O(total messages) passes, four times a second, forever |
| TAURI-15 | Low | VERIFIED | S | [06-tauri-bridge](reports/06-tauri-bridge.md) | Desktop notifications block under the manager lock and expose message previews |
| TAURI-16 | Low | VERIFIED | S | [06-tauri-bridge](reports/06-tauri-bridge.md) | Release builds log at debug level to stdout only; the log plugin is a dead dependency; diagnostics have no logs |
| TAURI-17 | Low | VERIFIED | S | [06-tauri-bridge](reports/06-tauri-bridge.md) | `change_password` can revert a concurrent display-name change |
| TAURI-18 | Low | VERIFIED | S | [06-tauri-bridge](reports/06-tauri-bridge.md) | The invite link advertises the settings port, not the live listener port |
| TAURI-19 | Low | VERIFIED | S | [06-tauri-bridge](reports/06-tauri-bridge.md) | `open_file` / `open_url` leave zombie processes on Unix and duplicate the `open` crate |
| TAURI-20 | Low | VERIFIED | S | [06-tauri-bridge](reports/06-tauri-bridge.md) | `unlock` reports every failure as "Wrong password" |
| TAURI-21 | Low | VERIFIED | S | [06-tauri-bridge](reports/06-tauri-bridge.md) | `panic = "abort"`: any command panic is a whole-app crash that skips the history flush |
| TAURI-22 | Low | SUSPECTED | S | [06-tauri-bridge](reports/06-tauri-bridge.md) | `send_file` awaits the bounded file lane while holding the manager mutex |
| TAURI-23 | Low | VERIFIED | S | [06-tauri-bridge](reports/06-tauri-bridge.md) | Data-directory fallbacks and permissions |
| TAURI-32 | Low | VERIFIED | M | [06-tauri-bridge](reports/06-tauri-bridge.md) | No restore or import path, although the UI tells users to "restore it over the file" |
| TAURI-33 | Low | VERIFIED | S | [06-tauri-bridge](reports/06-tauri-bridge.md) | The backup covers only the identity; conversations, contacts and trust states have no backup path, and the UI does not say so |
| TAURI-34 | Low | VERIFIED | M | [06-tauri-bridge](reports/06-tauri-bridge.md) | No in-app way to delete the identity or all local data |
| TUI-25 | Low | VERIFIED | S | [05-tui](reports/05-tui.md) | Ctrl+C does nothing, and Esc has a destructive meaning in one overlay |
| TUI-26 | Low | VERIFIED | S | [05-tui](reports/05-tui.md) | Message timestamps show UTC hours with no date; all incoming messages are labelled "Peer" |
| TUI-27 | Low | VERIFIED | S | [05-tui](reports/05-tui.md) | Auto-rehost can overwrite a command the user just submitted |
| TUI-28 | Low | VERIFIED | S | [05-tui](reports/05-tui.md) | In-app help and documentation are inaccurate or incomplete |
| TUI-29 | Low | SUSPECTED | S | [05-tui](reports/05-tui.md) | Clipboard: contents may vanish on Linux, the Identity overlay overwrites the clipboard without asking, and failures are hidden |
| TUI-30 | Low | SUSPECTED | S | [05-tui](reports/05-tui.md) | Windows: AltGr characters are probably dropped from all text fields |
| TUI-31 | Low | VERIFIED | S | [05-tui](reports/05-tui.md) | Duplicate toasts on connect |
| TUI-32 | Low | VERIFIED | M | [05-tui](reports/05-tui.md) | Leftover patterns from the `cmd_*.rs` split worth factoring |
| TUI-33 | Low | VERIFIED | S | [05-tui](reports/05-tui.md) | File-path arguments: no `~` expansion, relative to CWD, backup overwrites silently |
| TUI-34 | Low | VERIFIED | S | [05-tui](reports/05-tui.md) | Ambiguous name matching picks an arbitrary item; `#index` refers to an order that is never shown |
| TUI-35 | Low | VERIFIED | S | [05-tui](reports/05-tui.md) | Transfers overlay cannot accept or decline a specific offer; `:accept` picks one heuristically |
| TUI-36 | Low | SUSPECTED | S | [05-tui](reports/05-tui.md) | Bidi overrides in peer-supplied text are not neutralised in the TUI |
| UI-13 | Low | VERIFIED | S | [07-react-frontend](reports/07-react-frontend.md) | A failed send restores the draft into whichever conversation is now open |
| UI-14 | Low | VERIFIED | M | [07-react-frontend](reports/07-react-frontend.md) | Relay tokens and host addresses are lost when the dialog closes or the pane changes |
| UI-15 | Low | VERIFIED | S | [07-react-frontend](reports/07-react-frontend.md) | IPv6 addresses are mis-parsed in Connect |
| UI-16 | Low | VERIFIED | S | [07-react-frontend](reports/07-react-frontend.md) | Community admin dialogs close and discard input even when the action fails |
| UI-17 | Low | VERIFIED | S | [07-react-frontend](reports/07-react-frontend.md) | "Remove" and "Leave" share one two-click confirmation state |
| UI-18 | Low | VERIFIED | S | [07-react-frontend](reports/07-react-frontend.md) | Change password does not mention that existing backups keep the old password |
| UI-19 | Low | VERIFIED | S | [07-react-frontend](reports/07-react-frontend.md) | Contacts pane gaps (double connect, stale list, inverted state dot) |
| UI-20 | Low | VERIFIED | S | [07-react-frontend](reports/07-react-frontend.md) | The thread always jumps to the bottom on any new message |
| UI-21 | Low | VERIFIED | S | [07-react-frontend](reports/07-react-frontend.md) | The composer textarea never grows |
| UI-22 | Low | VERIFIED | S | [07-react-frontend](reports/07-react-frontend.md) | Some error paths bypass `friendlyError` |
| UI-23 | Low | VERIFIED | S | [07-react-frontend](reports/07-react-frontend.md) | The chat header labels a fingerprint prefix as an address; Info overstates encryption |
| UI-24 | Low | VERIFIED | M | [07-react-frontend](reports/07-react-frontend.md) | Menus and composite widgets lack keyboard semantics |
| UI-25 | Low | VERIFIED | S | [07-react-frontend](reports/07-react-frontend.md) | Icon-only trust badges have no accessible name |
| UI-26 | Low | VERIFIED | S | [07-react-frontend](reports/07-react-frontend.md) | The Verify dialog can switch peers under the user without notice |
| UI-27 | Low | SUSPECTED | S | [07-react-frontend](reports/07-react-frontend.md) | Link handling: auxiliary clicks, silent failures, no confirmation |
| UI-28 | Low | VERIFIED | S | [07-react-frontend](reports/07-react-frontend.md) | Relays pane shows developer-only instructions and a placeholder section |
| UI-29 | Low | VERIFIED | S | [07-react-frontend](reports/07-react-frontend.md) | Jargon exposed to non-technical users |
| UI-30 | Low | VERIFIED | S | [07-react-frontend](reports/07-react-frontend.md) | Incoming file offers show no size; Parties marks threads read while the window is hidden |
| UI-31 | Low | VERIFIED | S | [07-react-frontend](reports/07-react-frontend.md) | The global right-click block removes copy from message bubbles |
| UI-32 | Low | VERIFIED | M | [07-react-frontend](reports/07-react-frontend.md) | Maintainability: dead CSS, duplicated helpers, large components |
| UI-33 | Low | SUSPECTED | S | [07-react-frontend](reports/07-react-frontend.md) | The safety grid is case-sensitive over the hex string |
| APP-24 | Info | VERIFIED | S | [04-client-core](reports/04-client-core.md) | Dead code and dead fields |
| CRYPTO-15 | Info | VERIFIED | S | [01-crypto-protocol](reports/01-crypto-protocol.md) | The Ed25519 proof path adds work and no security while the identity is still RSA |
| CRYPTO-17 | Info | VERIFIED | M | [01-crypto-protocol](reports/01-crypto-protocol.md) | Security-critical paths with no tests |
| CRYPTO-18 | Info | VERIFIED | S | [01-crypto-protocol](reports/01-crypto-protocol.md) | Minor protocol and encoding nits |
| DOC-38 | Info | VERIFIED | S | [09-documentation](reports/09-documentation.md) | The project is called five different things |
| DOC-39 | Info | VERIFIED | S | [09-documentation](reports/09-documentation.md) | Public constants in `core` have no rustdoc |
| DOC-40 | Info | VERIFIED | S | [09-documentation](reports/09-documentation.md) | DEVELOPER_GUIDE uses `cargo tauri dev` where every other doc uses `npx tauri dev` |
| DX-17 | Info | VERIFIED | M | [10-dx-ci-tests](reports/10-dx-ci-tests.md) | Dependency duplication: two RustCrypto generations, three `syn`s, and `tokio` "full" |
| JOURNEY-22 | Info | VERIFIED | S | [08-user-journey-footprint](reports/08-user-journey-footprint.md) | Delivery receipts cannot be turned off; typing indicators are on by default |
| JOURNEY-23 | Info | VERIFIED | S | [08-user-journey-footprint](reports/08-user-journey-footprint.md) | Stale or unused packaging bits |
| NET-20 | Info | VERIFIED | S | [02-network](reports/02-network.md) | Relay control frames accept trailing bytes, unlike every other bincode frame |
| NET-21 | Info | VERIFIED | S/M | [02-network](reports/02-network.md) | Per-frame copies and the bridge's small buffers |
| NET-22 | Info | VERIFIED | ? | [02-network](reports/02-network.md) | Test gaps for these paths |
| PRIV-12 | Info | VERIFIED | S | [13-privacy-supply-chain](reports/13-privacy-supply-chain.md) | Stale privacy comment in `Config` about mDNS |
| QUAL-23 | Info | VERIFIED | S | [12-code-quality](reports/12-code-quality.md) | `expect` on the AEAD nonce counter |
| QUAL-26 | Info | VERIFIED | S | [12-code-quality](reports/12-code-quality.md) | No doctests while CI runs `cargo test --doc` |
| QUAL-27 | Info | VERIFIED | S | [12-code-quality](reports/12-code-quality.md) | No workspace lint policy; selected clippy lints worth adopting |
| QUAL-28 | Info | VERIFIED | ? | [12-code-quality](reports/12-code-quality.md) | Non-atomic channel removal in `delete_channel` *(cross-reference)* |
| QUAL-29 | Info | VERIFIED | ? | [12-code-quality](reports/12-code-quality.md) | Documentation drift in CLAUDE.md affecting maintainers |
| SECRETS-01 | Info | VERIFIED | n/a | [13-privacy-supply-chain](reports/13-privacy-supply-chain.md) | No committed secrets found; the one historical fixture is properly allowlisted |
| SRV-24 | Info | VERIFIED | S | [03-party-server](reports/03-party-server.md) | Maintainability nits |
| SUPPLY-09 | Info | VERIFIED | S | [13-privacy-supply-chain](reports/13-privacy-supply-chain.md) | Releases carry provenance but no SBOM, and installers are unsigned |
| TAURI-24 | Info | VERIFIED | S | [06-tauri-bridge](reports/06-tauri-bridge.md) | Stale comments and an unused command |
| TAURI-25 | Info | VERIFIED | S | [06-tauri-bridge](reports/06-tauri-bridge.md) | The invoke-contract test cannot see Tauri's camelCase mapping or nested struct fields |
| TAURI-26 | Info | VERIFIED | M | [06-tauri-bridge](reports/06-tauri-bridge.md) | No updater and no OS code signing |
| TAURI-27 | Info | VERIFIED | M | [06-tauri-bridge](reports/06-tauri-bridge.md) | Command test coverage gaps |
| TAURI-28 | Info | VERIFIED | S | [06-tauri-bridge](reports/06-tauri-bridge.md) | The community pin is keyed by the raw address string |
| TAURI-29 | Info | VERIFIED | S | [06-tauri-bridge](reports/06-tauri-bridge.md) | Optional hardening not enabled |
| TUI-37 | Info | VERIFIED | S | [05-tui](reports/05-tui.md) | Diagnostics bundle details |
| UI-34 | Info | VERIFIED | S | [07-react-frontend](reports/07-react-frontend.md) | Set-password explanation path is unreachable |
| UI-35 | Info | VERIFIED | L | [07-react-frontend](reports/07-react-frontend.md) | i18n readiness |
| UI-36 | Info | VERIFIED | S | [07-react-frontend](reports/07-react-frontend.md) | No system-theme default |
| UI-37 | Info | VERIFIED | M | [07-react-frontend](reports/07-react-frontend.md) | Test coverage gaps for the bugs above |
