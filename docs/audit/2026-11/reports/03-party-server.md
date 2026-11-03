# Party / Community server and its protocol

## Scope covered (list every file read)

Read in full:
- `server/Cargo.toml`
- `server/src/lib.rs`
- `server/src/main.rs`
- `server/src/identity.rs`
- `server/src/hub.rs`
- `server/src/dispatch.rs`
- `server/src/connection.rs`
- `server/src/state/mod.rs`
- `server/src/state/blobs.rs`
- `server/src/state/db.rs`
- `server/src/adversarial_tests.rs`
- `core/src/party/mod.rs` (wire types, `send_framed`/`recv_framed`, `PartyClient`, tests)
- `docs/server_tiers.md`

Skimmed for coverage: `server/src/state/tests.rs` (all test names, plus the paging, guest-write, upload-ownership and chunked-read tests).

Read to confirm specific claims: `core/src/core/framing.rs` (`send_packet`/`recv_packet` size checks), `core/src/network/session.rs` (`host_handshake`, handshake timeouts), `core/src/network/ratelimit.rs` (limiter semantics), `core/src/util.rs::sanitize_filename`, `desktop/src/components/Parties.jsx:84-110` (the server-verify card), `client/src/app/party_manager.rs:824-836` (which download request the client uses).

I did not build or run anything. "VERIFIED" means the failure follows directly from the cited code. Where I give a number (frame sizes, rates) I worked it out from the constants; I did not observe it.

## Summary

The authorization layer is careful and mostly right. Roles are ordered, channel kinds are enforced on read and post, private channels go only to their members, downloads are checked against `file_refs` before any byte is read, and upload ownership is checked. The three-phase upload (reserve, stage, commit) with re-checks holds up. I found no IDOR: upload ids are minted by the server, a DM thread is derived from the caller's own id, and a blob hash only reaches the disk after a `blobs` lookup. There is no SQL injection: every value is bound, and the only interpolated identifiers are a closed enum and module constants.

The serious problems are about **availability and lifecycle**, not access control:

1. **One member can make a community permanently unjoinable (Critical).** A history page is capped by *count* (200 envelopes), not by *bytes*. 200 envelopes of 64 KiB text come to about 12.8 MiB, which is over the 8 MiB frame limit. The `mime` field of a file is never validated, so one upload can make even a *live broadcast* frame too large. A frame too large to send ends the connection, and every client asks for that history again on reconnect. This is the bug CLAUDE.md says paging fixed; the paging test only uses tiny messages.
2. **When the hub evicts a slow connection, the socket stays open** but never receives another broadcast or DM. The client goes deaf and nothing tells it.
3. **No idle, read or write timeouts and no cap on concurrent connections.** A client that never joins holds a task and a file descriptor forever, so an unauthenticated attacker can exhaust file descriptors within the per-IP rate limit.
4. **Trust bootstrap is weak.** The first identity to connect becomes Owner, and the server is open by default with no password. The client's "verify this server" screen asks the user to compare a SAS that the operator has no way to display.
5. **No rate limits or retention on members' writes** (posts, DMs, shares, permission changes). All history stays in RAM forever, and any member can push admin actions out of the visible audit log.
6. **No moderation tools.** There is no kick, ban, member removal or message deletion, and a Guest can still send DMs. On an open server a demoted member just reconnects with a new key.
7. **Writes are not transactional.** SQLite runs each statement on its own, and refcounts are not reconciled on load. Each write is an fsync while the global state lock is held.
8. **Several per-request paths are O(n) or worse while holding the single global lock:** history scans, `ListFiles` (SHA-256 per member per file), directory broadcasts.

## Findings — ordered by severity

### SRV-01 — Frames built from history, broadcasts or listings can exceed `MAX_PACKET_SIZE`; one member can make the community permanently unjoinable
- Severity: Critical
- Category: security (DoS) / bug
- Status: VERIFIED (static; sizes worked out from the constants)
- Location: `server/src/state/mod.rs:1056-1070` (`history_since`), `server/src/state/mod.rs:1109-1120` (`dm_history`), `core/src/party/mod.rs:661` (`MAX_HISTORY_BATCH = 200`), `server/src/state/mod.rs:97` (`MAX_MESSAGE_TEXT_BYTES` = 64 KiB), `core/src/core/framing.rs:12-18` (`send_packet` refuses > 8 MiB), `server/src/connection.rs:181-185, 221-225` (a failed send ends the connection), `server/src/state/blobs.rs:704-766, 1029-1090` (`mime` is never validated), `server/src/state/blobs.rs:634-672` (`files_for`, not paged), `server/src/state/mod.rs:663-676` (`members()`, not paged)
- Problem: pages are capped at 200 *envelopes* but have no byte budget.
  - **Text history:** each text can be 64 KiB, so a page can hold about 12.8 MiB. Any frame over 8 MiB fails in `send_packet` → `served = Err` → `break` → disconnect. About 129 maximum-size posts in one channel is enough.
  - **File metadata:** `mime` (and the pre-sanitised `name`) are taken from the client with no length check. They are stored in `blobs`, `messages` and `FileMeta`, and copied into every `Message` broadcast, `History` page and `Files` listing. A `StartUpload` whose `mime` sits just under 8 MiB produces a `Message(Envelope)` that is about 108 bytes **larger** than the request that created it. Smaller cases are easier: eight inline `PostFile`s, each with a 1 MiB `mime`, put any page containing them over the limit.
  - **Other unpaged lists:** `Files`, `Members` and `Channels` (private-channel member lists) grow without bound and hit the same wall.
- Impact: one ordinary member can do this in seconds.
  - Every client that fetches `#general` history from before the bad messages is disconnected, and every client fetches it on join. With the `mime` variant, every *online* recipient is also disconnected the moment the broadcast is sent.
  - It persists across restarts (the history is durable) and affects the owner too, whose client also seeds history on join.
  - Recovery means editing SQLite by hand.
- Recommendation:
  - Budget pages by serialized bytes, e.g. stop adding envelopes at about 4 MiB, always return at least one, and make sure a single envelope can never exceed the budget.
  - Validate `mime` (≤ 255 bytes, `type/subtype` charset) and cap `name` length on arrival in `start_upload`/`begin_inline_upload`. Validate `SetChannelAccess.members` length too.
  - Page `ListFiles` and `ListMembers`.
  - Defence in depth: in `serve_connection`, if a *reply* is too big, answer with `Error` instead of tearing the connection down. If a *broadcast* is too big, drop it and log it.
  - Add tests with 200 × 64 KiB messages and with a 1 MiB `mime`.
- Effort: M

### SRV-02 — When the hub evicts a stuck connection, the socket stays open and the client goes deaf
- Severity: High
- Category: bug
- Status: VERIFIED
- Location: `server/src/hub.rs:61-72, 117-130`; `server/src/connection.rs:134, 136, 214-219`
- Problem: when `try_send` finds a full queue, `Hub::push` removes the connection from the hub map, but nothing signals `serve_connection`:
  - The task still owns the original `out_tx` (line 134), so `out_rx.recv()` never returns `None`.
  - `registered` stays `true`, so the connection is never registered again.
  - The client keeps sending requests and getting replies, but gets no more broadcasts, channel-directory nudges or DMs.
- How it happens: the queue fills whenever the task does not drain `out_rx` for 256 broadcasts. That includes while it waits on `state.lock()`, while it runs `perform_deferred` (a 100 MiB staging write and fsync), and while it is blocked writing to a client with a slow TCP window. In a busy community, or when one member floods posts (see SRV-06), this happens to ordinary slow clients.
- Impact: the user sees a live connection that silently stopped updating, which is the exact symptom the `read_frame` fix was meant to remove. Presence is also wrong:
  - `member_is_connected` returns false for an evicted device, so when the member's other device disconnects they are marked offline while still connected.
  - The hub doc comment says the client "catches up via FetchHistory on reconnect", but it never reconnects.
- Recommendation: on eviction, close the connection.
  - Option 1: keep a per-connection `CancellationToken` or `oneshot` in `Conn`, fire it in `push`, and `select!` on it in `serve_connection` so the loop breaks and the client reconnects.
  - Option 2: drop the task's own `out_tx` after registering, so eviction makes `recv()` return `None`, and break on that.
  - Add a test: fill the queue, then assert the server closes the connection.
- Effort: S

### SRV-03 — No idle, read or write timeouts and no concurrent-connection cap: pre-auth file-descriptor exhaustion, ghost presence, permanently blocked tasks
- Severity: High
- Category: security (DoS) / bug
- Status: VERIFIED
- Location: `server/src/lib.rs:53-77`; `server/src/connection.rs:163, 173-247` (no timeout around `pending`, the `send_framed` calls, or the pre-join phase); `core/src/network/ratelimit.rs:22-24` (10 connections per 30 s per IP is a *rate*, not a concurrency cap)
- Problem:
  - Once the handshake completes (each handshake read has a 15 s timeout), a connection may sit unjoined forever. Nothing limits how many connections exist at once, and no TCP keepalive is set.
  - Writes (`send_framed` to a client that stopped reading) have no timeout, so the task blocks indefinitely. While it is blocked, the broadcasts of that poster's own message are also held back (see SRV-18).
- Impact:
  - **Unauthenticated, from one IPv4 address:** 10 connections per 30 s is 1,200 idle sockets an hour. A default 1024 soft `RLIMIT_NOFILE` runs out in under an hour. After that `accept()` fails with EMFILE every 50 ms, logging each time, and nobody can join. The limit is higher than 4096 tracked addresses, and IPv6 /64 rotation is faster still.
  - **Half-open connections** (a laptop sleeping, NAT timeouts) stay registered and show as **online forever** in a quiet community.
  - **Upload spools** held by such a connection (SRV-07) are never freed.
- Recommendation:
  - Add a pre-join deadline, e.g. drop connections that have not joined within 30 s.
  - Add an idle read timeout plus an application-level ping/keepalive (or `SO_KEEPALIVE` via `socket2`).
  - Put a timeout on every `send_framed`.
  - Add a global `Semaphore` cap on concurrent connections (and optionally a per-IP concurrency cap), refusing early.
  - Raise the soft NOFILE limit at startup, or document it.
- Effort: M

### SRV-04 — Ownership goes to whoever connects first, and servers are open by default
- Severity: High
- Category: security
- Status: VERIFIED
- Location: `server/src/state/mod.rs:494-503` (`role = Owner` when `members.is_empty()`), `server/src/main.rs:40-42` (password optional; omitted means an open server), `server/src/main.rs:77` (binds `0.0.0.0`)
- Problem: the first identity to complete `Join` becomes the permanent, undemotable Owner (`set_role` refuses to change an Owner, lines 574-579). There is no bootstrap secret, and the default configuration has no password and listens on every interface.
- Impact:
  - An internet scanner, or anyone who learns the address before the operator joins, takes the community and can then appoint admins, delete channels, read every private channel and the audit log.
  - The operator can only recover by editing `party.db` by hand. No ownership transfer exists, and `ensure_owner` only runs when *no* admin exists.
  - Username squatting is in the same class: on an open server someone can register "admin" or the operator's name first.
- Recommendation:
  - At first start, print a one-time **owner claim token** (or accept `--owner-fingerprint <fp>`), and grant Owner only to a `Join` that presents it.
  - Consider defaulting to a required password, or binding to localhost unless `--public` is given.
  - Add an operator-only CLI path (`server admin set-owner <username>`) for recovery.
- Effort: M

### SRV-05 — Users are asked to verify the server with a SAS the operator cannot see and that does not belong to the connection being trusted
- Severity: High
- Category: security / ux
- Status: VERIFIED
- Location:
  - The server only logs the static fingerprint: `server/src/main.rs:72-87`.
  - `serve_connection` never computes or logs a SAS: `server/src/connection.rs:128-131`.
  - The SAS is computed client-side per connection: `core/src/party/mod.rs:857`.
  - The UI: `desktop/src/components/Parties.jsx:90-98` ("Ask the operator to read out their code", with the fingerprint under "Advanced").
- Problem: the SAS is derived from *that connection's* handshake transcript.
  - The operator has no tool to show it. Nothing in the server logs or prints a per-connection SAS, and a SAS is only meaningful when both ends display the same transcript.
  - The two-step flow hangs up after the first handshake and reconnects with `trust_new_identity = true`. The connection being trusted therefore has a different transcript, and so a different SAS, from the one shown.
  - The only comparable value is the fingerprint the server logs, and the UI hides it under "Advanced".
- Impact: in practice every first join is blind TOFU. Users press "The code matches" because no code to match exists, and an active MITM on first join goes undetected.
- Recommendation: for community servers, make the **fingerprint** (or a short, stable code derived from it, e.g. its first 8 groups or emoji derived from the fingerprint) the primary verification value. Print the same short code at server startup. Stop presenting the per-connection SAS as something the operator can read out.
- Effort: S

### SRV-06 — No per-member rate limits on posts, DMs, shares or permission changes; all history kept in RAM forever; no retention
- Severity: High (open servers) / Medium (password-protected)
- Category: security (DoS) / performance
- Status: VERIFIED
- Location: `server/src/state/mod.rs:1014-1041` (`post_message`), `server/src/state/mod.rs:1076-1105` (`post_dm`), `server/src/state/blobs.rs:404-501` (`share_file`), `server/src/state/db.rs:211, 230` (all messages loaded at startup), `server/src/state/mod.rs:183, 259` (history is `Vec<Envelope>` in memory)
- Problem:
  - Only channel creation is rate-limited. Any member can post 64 KiB messages and DMs as fast as the link allows.
  - Each message is kept in memory forever, written to SQLite with an fsync while the global lock is held (SRV-12), and cloned to every connection (SRV-23).
  - At startup every message in every channel and DM thread is deserialized into RAM.
  - DM threads can be created for every pair of members.
- Impact:
  - A single member can push the server's RSS and disk toward OOM: 1 GiB is about 16k max-size posts.
  - The global lock is held continuously, which slows every other member.
  - Each restart takes longer.
  - Members are free to create on an open server (a new RSA key per connection), so no ban helps (SRV-08).
- Recommendation:
  - Add a per-member token-bucket rate limit for every write request, with a byte budget as well as a count.
  - Load history lazily: keep only `len()`/last-seq in memory and serve pages from SQLite with `WHERE seq > ? ORDER BY seq LIMIT ?`.
  - Add a retention option (max age / max messages per channel).
- Effort: L

### SRV-07 — Chunked-upload spools live in RAM, bounded only per member (4 × 100 MiB) and outside every quota
- Severity: Medium
- Category: security (DoS) / performance
- Status: VERIFIED
- Location: `server/src/state/mod.rs:217-245` (`PendingUpload.data: Vec<u8>`, `MAX_CONCURRENT_UPLOADS = 4`), `server/src/state/blobs.rs:704-766` (`start_upload`), `server/src/state/blobs.rs:291-308` (`member_blob_bytes` ignores `self.uploads`)
- Problem:
  - `start_upload` checks each upload *individually* against the member quota. Spooled bytes are not counted, so four 100 MiB uploads each pass against a 128 MiB allowance.
  - Nothing checks spool bytes server-wide, and `check_storage_room` only runs at finish.
  - Spools have no expiry while the connection lives, and connections never time out (SRV-03).
- Impact: each member can pin 400 MiB of server RAM. On an open server, N members created for the purpose pin N × 400 MiB, and a typical VPS is OOM-killed after a few GiB of upload. A Guest cannot do this (`start_upload` refuses read-only roles), but a fresh member can.
- Recommendation:
  - Spool to staging files on disk instead of RAM. `write_staging` already exists, and the hash can still be accumulated.
  - Count spooled bytes in `member_blob_bytes` and in a global in-flight budget.
  - Expire spools that have been idle for N minutes.
  - Check the global ceiling in `start_upload`.
- Effort: M

### SRV-08 — Moderation is largely missing, and a Guest can still send DMs
- Severity: Medium (the DM bypass is a VERIFIED bug; missing features are unfinished work)
- Category: bug / unfinished
- Status: VERIFIED
- Location:
  - `server/src/state/mod.rs:1076-1105`: `post_dm` has no `can_write` check, while `check_may_post_file` (`blobs.rs:1109-1112`) and `share_file` (`blobs.rs:431-433`) both have one.
  - `server/src/state/tests.rs:1008-1027`: `a_guest_cannot_write_anywhere` never tests `post_dm`.
  - `core/src/party/mod.rs:317-429`: the request set.
- Problem:
  - `Role::Guest` is documented as "read-only everywhere" (`core/src/party/mod.rs:170, 192-196`), but text DMs from a Guest are accepted, so the only moderation tool, demotion, does not stop DM spam or harassment.
  - The protocol has no `RemoveMember`/kick, ban (by fingerprint), message delete or edit, member rename, ownership transfer, invites/invite codes, retention, or way for a member to leave.
  - `docs/server_tiers.md:159-166` says operators can "remove a member" and "delete a message", and neither exists.
  - Members are never removed, so usernames stay taken forever, and a member who loses their identity key cannot get their name back.
- Impact: on an open server, a demoted abuser keeps DMing everyone, or reconnects with a new key as a full Member. Admins cannot remove abusive content short of deleting the whole channel.
- Recommendation:
  - Add `if !self.role_of(from).can_write() { return Err(...) }` to `post_dm`, with a test.
  - Then add, in this order: a fingerprint ban (refused at `join`), member removal, admin message deletion (tombstone the envelope at the same seq so numbering is preserved), invite codes, and per-channel retention.
  - Fix the doc.
- Effort: S (bug) / L (features)

### SRV-09 — Any member can push admin actions out of the visible audit log; the audit table grows without bound
- Severity: Medium
- Category: security
- Status: VERIFIED
- Location: `server/src/state/mod.rs:617-630` (in memory capped at 1000, oldest dropped), `server/src/state/db.rs:105-113` (insert only, never pruned), `server/src/state/db.rs:320-343` (reload reads the last 1000), `server/src/state/blobs.rs:498-499, 558, 626` (`file.share`, `file.permissions`, `file.delete` are recorded for ordinary members' own files, with no rate limit)
- Problem: `set_file_permissions` on one's own file can be called in a loop. Each call writes an audit row with an fsync. `FetchAuditLog` only ever shows the newest 1000, and the log has no paging past that.
- Impact: a malicious admin (or any member covering for one) can push a `role.set` or `channel.delete` out of view in about 1000 cheap requests. The `audit` table, and the disk, also grow without limit.
- Recommendation:
  - Record only governance actions, or rate-limit self-service ones.
  - Do not record no-op permission changes.
  - Serve the audit log from SQLite with paging (`before_id`) instead of a 1000-entry in-memory window.
  - Add an operator retention setting.
- Effort: S-M

### SRV-10 — Multi-row operations are not transactional and refcounts are not reconciled on load: crashes leak blobs or delete files still in use
- Severity: Medium
- Category: bug (consistency)
- Status: VERIFIED (by statement ordering)
- Location: `server/src/state/mod.rs:944-958` (`release_blob` persists refcounts / deletes blob files *before* `delete_channel_rows`), `server/src/state/db.rs:91-103` (three separate DELETEs), `server/src/state/blobs.rs:1037-1080` then `1296/1337/1310` (refcount or blob row, then file_ref, then message; each autocommit), `server/src/state/db.rs:251-268` (refcount loaded as stored, never recomputed from `file_refs`)
- Problem: each `persist_*` call runs as its own autocommit statement, and failures are only logged.
  - **Crash midway through `delete_channel`:** refcounts have already been decremented, but the channel, messages and `file_refs` rows survive. After restart the channel is back and its refs outnumber the stored refcount. Deleting one of them later frees a blob still referenced elsewhere, which then reads as "unknown file".
  - **Crash midway through a commit:** a blob file is renamed into place with no row (an orphan on disk, never swept and not counted against any quota), or a refcount is one higher than the real refs (the blob can never be reclaimed).
  - **One corrupt envelope or kind row** makes `reload_from_db` fail with `?`, and the server will not start at all.
- Recommendation:
  - Wrap each logical mutation in a single `Transaction` (post, commit-upload, share, delete-file, delete-channel).
  - On load, recompute `refcount = count(file_refs WHERE hash = ?)`.
  - Sweep blob files that have no row.
  - Treat an undecodable message row as a logged skip rather than a fatal error.
- Effort: M

### SRV-11 — `party.db` and blobs (plaintext channels and DMs) are created with default permissions
- Severity: Medium
- Category: security
- Status: VERIFIED
- Location: `server/src/state/mod.rs:379-382` (`create_dir_all`, `Connection::open`), `server/src/state/blobs.rs:164` (`File::create`). No chmod or umask anywhere in `server/` (only `identity.rs` uses `write_file_atomic` with 0600).
- Problem: under a 022 umask, the data directory, `party.db` (every message and DM, in plaintext per `docs/server_tiers.md:16-20`) and every blob are world-readable. The server key is 0600, which makes the gap obvious.
- Impact: any local user on a shared host reads the whole community.
- Recommendation: create the data dir 0700 (`DirBuilder::mode`), set the umask to 077 at startup or chmod `party.db*` and the blobs after creation, and create staging files with `OpenOptions::mode(0o600)`.
- Effort: S

### SRV-12 — Every mutation is an autocommit fsync made while the global state lock is held; no WAL
- Severity: Medium
- Category: performance
- Status: VERIFIED
- Location: `server/src/state/db.rs:357-446` (no `PRAGMA journal_mode`/`synchronous`/`busy_timeout`), `server/src/connection.rs:198-199` (all requests serialise on one `tokio::Mutex<PartyState>`)
- Problem: `post_message` and the like run `INSERT` in rollback-journal, `synchronous=FULL` mode, which costs a couple of fsyncs per statement, all while holding the only lock. `record_audit` adds more.
- Impact: community-wide message throughput is bounded by disk fsync latency (tens per second on SD cards or network disks), and every member's request queues behind it.
- Recommendation: `PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=...`. Batch the statements of one mutation into one transaction (SRV-10). In the longer term, move DB writes to a dedicated writer task.
- Effort: S

### SRV-13 — O(n) and O(n·m) work per request while holding the global lock
- Severity: Medium
- Category: performance
- Status: VERIFIED
- Location:
  - `server/src/state/mod.rs:1060-1067`: `history_since` iterates from message 1 even though `seq == index + 1`, making each page O(since_seq) and catching up on a full channel O(n²/200).
  - `server/src/state/blobs.rs:634-672`: `files_for` calls `effective_perms` 3× per ref, and for DM refs `can_reach_location` (`blobs.rs:386-390`) runs SHA-256 `dm_thread_id` against **every member**. `dm_location_name` (`675-682`) does it again.
  - `server/src/state/blobs.rs:1389-1397`: a scan of all `file_refs` on **every 256 KiB `DownloadChunk`**.
  - `server/src/state/mod.rs:663-676`, `dispatch.rs:393`, `connection.rs:272-279`: the full sorted member list is broadcast to every connection on every join and every last-device disconnect, O(N²) bytes per event.
  - `server/src/state/blobs.rs:291-308, 1144-1148`: quota sums over all refs and blobs.
- Impact: with 10k file refs and 1k members, one `ListFiles` is roughly 10–30 million SHA-256 operations under the lock, which takes seconds for every member, and any member can repeat it. Reconnect churn amplifies directory broadcasts.
- Recommendation:
  - Slice history: `messages[since_seq as usize..].iter().take(..)`.
  - Store DM participants (`a`, `b`) on DM `FileRef`s and check membership directly.
  - Index `file_refs` by hash (`HashMap<String, Vec<idx>>`).
  - Cache per-member usage.
  - Broadcast presence deltas (`MemberJoined`/`PresenceChanged`) instead of the full directory.
- Effort: M

### SRV-14 — `DownloadFile` reads a whole blob (up to 100 MiB) into memory and then fails to send it
- Severity: Medium
- Category: security (DoS) / bug
- Status: VERIFIED
- Location: `server/src/dispatch.rs:464`, `server/src/state/blobs.rs:1465-1466` (`len = total` when `offset` is `None`), `server/src/connection.rs:63-69, 221-225`
- Problem: the honest client only uses `DownloadFile` for files of 4 MiB or less (`client/src/app/party_manager.rs:824-836`), but the server accepts it for any size.
- Impact: a member with access to a 100 MiB blob can make the server allocate about 100 MiB (plus about 100 MiB more for bincode and the ciphertext) per request. The reply then breaks the 8 MiB limit, the connection drops, and the member reconnects and repeats. Spread across a few connections, that is a reliable memory spike.
- Recommendation: refuse `DownloadFile` when `total > MAX_INLINE_FILE_BYTES` (reply `Error`, telling the client to use chunks), or cap `len` at `MAX_INLINE_FILE_BYTES`.
- Effort: S

### SRV-15 — Username uniqueness is ASCII case-only; homoglyphs and non-ASCII case variants allow impersonation
- Severity: Medium
- Category: security
- Status: VERIFIED
- Location: `server/src/state/mod.rs:486-492` (`eq_ignore_ascii_case`), `server/src/state/mod.rs:463` (only hidden and bidi characters are rejected); the same applies to channel names at `mod.rs:746-751`
- Problem: "аdmin" (Cyrillic а), "ÀDMIN" vs "àdmin", and full-width "ａｄｍｉｎ" are all accepted as distinct from "admin". `MemberInfo` has no fingerprint for clients to tell the accounts apart.
- Impact: one member can impersonate the owner or admins in channels and DMs.
- Recommendation: compare a skeleton (NFKC + Unicode case fold + a confusables mapping such as `unicode-security`'s skeleton) for uniqueness. Separately, expose a short per-member identity code in `MemberInfo` (also needed for `docs/server_tiers.md` T1.1).
- Effort: M

### SRV-16 — Per-IP connection limit silently refuses groups behind one NAT; password guessing scales with source addresses
- Severity: Medium
- Category: ux / security
- Status: VERIFIED
- Location: `server/src/lib.rs:62-66`, `core/src/network/ratelimit.rs:22-24`; the password is one shared secret (`server/src/state/mod.rs:466-468`, `MAX_JOIN_ATTEMPTS = 3` per connection at `dispatch.rs:161`)
- Problem:
  - Ten connections per 30 s per IPv4 address, refused with no message, means the 11th friend at a LAN party or behind carrier-grade NAT, or one client's automatic reconnects after a Wi-Fi blip, gets a bare closed socket.
  - Guessing speed is about 1 password per second per source address with no global ceiling, so a botnet or IPv6 /48 has no meaningful limit.
  - A shared password cannot be revoked per member.
- Recommendation:
  - Send a short plaintext refusal before closing, or complete the handshake and answer `JoinRejected { reason: "rate limited" }`.
  - Make the limits configurable.
  - Add a global failed-join backoff.
  - In the longer term, use per-member invite codes instead of one shared password.
- Effort: M

### SRV-17 — Presence race between a member's leaving and joining connections
- Severity: Low
- Category: bug
- Status: VERIFIED (by ordering)
- Location: `server/src/connection.rs:213-219` (hub registration happens after `handle_request` released the lock), `server/src/connection.rs:271-280` (`member_is_connected`, then `set_online(false)`)
- Problem: device B's `join` sets `online = true` under the lock but registers with the hub only after the lock is released. If device A's teardown runs `member_is_connected` in that window, it sees no connection and marks the member offline. The member then shows as offline until the next directory event.
- Recommendation: register in the hub inside the same critical section as `join` (pass the hub into dispatch, or register before releasing the lock), or count connections per member in `PartyState`.
- Effort: S

### SRV-18 — Live delivery order does not follow seq order, and fan-out waits on the poster's own socket
- Severity: Low
- Category: bug / performance
- Status: VERIFIED
- Location: `server/src/connection.rs:198-235`
- Problem: the seq is assigned under the lock, but fan-out happens later, after the reply has been written to the poster with `send_framed(...).await`. So:
  - Two posters' broadcasts can arrive at others in the opposite order to their seqs.
  - A poster whose TCP window is full delays (with no timeout, see SRV-03, indefinitely) live delivery of a message that is already stored.
- Recommendation: push broadcasts to the hub before writing the reply (`try_send` never blocks), or enqueue the reply through `out_tx` as well. Clients should keep merging by seq.
- Effort: S

### SRV-19 — Operator experience gaps
- Severity: Low
- Category: ux / dx
- Status: VERIFIED
- Location: `server/src/main.rs:29-48, 77`; `server/src/state/mod.rs:357-366` (quota setters not exposed)
- Problems:
  - `--password` on the command line is visible in `ps` and shell history. Provide `--password-file`.
  - `--data-dir` defaults to the relative `party-data`, so a service started from a different working directory silently creates a *new* community with a *new* identity, and every client sees a key change.
  - The listener is hard-coded to IPv4 `0.0.0.0`: no `--bind` option, no IPv6, no way to keep it on localhost or a VPN interface.
  - The default port is the same as the P2P client's `PORT_DEFAULT`, so it conflicts on a machine that also hosts P2P.
  - Storage limits (`MAX_TOTAL_BLOB_BYTES`, `MAX_MEMBER_BLOB_BYTES`, `MAX_CHANNELS`) cannot be configured.
  - No backup or export command (copying a rollback-journal DB while it is live can be inconsistent; offer `VACUUM INTO`).
  - No schema version (`PRAGMA user_version`); migrations are matched on the string `"duplicate column name"` (`db.rs:416-436`).
  - No graceful SIGTERM handling.
  - No admin CLI for member/owner recovery.
  - Joins are logged by IP only, with no username or member id.
- Recommendation: add these flags and subcommands, and a `docs/` operator guide.
- Effort: M

### SRV-20 — Upgrade bootstrap hands Owner to the member with the lowest UUID
- Severity: Low
- Category: security
- Status: VERIFIED
- Location: `server/src/state/mod.rs:529-551`
- Problem: on a community that predates roles, a random member (not necessarily the operator) becomes the permanent Owner. It is audited but cannot be undone.
- Recommendation: require `--owner <username|fingerprint>` when no admin exists. If none is given, start with no owner and log loudly.
- Effort: S

### SRV-21 — Quota and ceiling edge cases
- Severity: Low
- Category: bug
- Status: VERIFIED
- Location:
  - `server/src/state/blobs.rs:448-463`: `share_file` adds a reference counted against the sharer's quota without calling `check_storage_room`, so a member can exceed their own allowance by sharing.
  - `server/src/state/blobs.rs:704-766`: `start_upload` does not check the server-wide ceiling, so a member can stream 100 MiB only to be refused at finish.
  - `server/src/state/blobs.rs:315`: `quota_for.server_used` excludes reservations, while the ceiling check includes them.
  - `server/src/state/blobs.rs:1296-1299`, `1337`: `append_file_*` records the file_ref *before* checking that the channel exists. This is safe today only because `commit_upload` re-checks under the same lock; it is fragile.
- Recommendation: check the quota in `share_file` and the ceiling in `start_upload`; report reservations in `server_used`; record the file_ref after the target lookup.
- Effort: S

### SRV-22 — Stale or incorrect comments and docs
- Severity: Low
- Category: docs
- Status: VERIFIED
- Location and problem:
  - `server/src/state/blobs.rs:6-37`: `blocking_io`'s doc still describes the request path as reading under the lock via `blob_chunk_for`/`blob_bytes`; the request path now uses `plan_blob_read`.
  - `server/src/state/mod.rs:303-308`: says reservations are pessimistic because "the hash is not known"; it is known on every path now.
  - `server/src/state/mod.rs:64-66, 753-756`: says "there is no admin role".
  - `server/src/state/mod.rs:976-987`: `member_can_read_channel` says there is "no per-channel membership list", but there is one.
  - `server/src/state/blobs.rs:96-97`: refers to `cancel_uploads_for` where `cancel_connection_uploads` is meant.
  - `server/src/state/blobs.rs:798-800`: duplicated doc header.
  - CLAUDE.md says chunked uploads are keyed by "the client's own upload id"; the id is server-minted (`blobs.rs:753`). It also says chunks are 64 KiB; `PARTY_CHUNK_BYTES` is 256 KiB.
  - `docs/server_tiers.md:159-166` claims message deletion and member removal exist (see SRV-08).
  - CLAUDE.md's "Party history is paged" entry says paging fixed the oversized-frame failure; it did not (SRV-01).
- Recommendation: correct each.
- Effort: S

### SRV-23 — Every broadcast is cloned and serialized once per connection, inside the hub's std `Mutex`
- Severity: Low
- Category: performance
- Status: VERIFIED
- Location: `server/src/hub.rs:61-72, 85-96`; `server/src/connection.rs:182`
- Problem: a 64 KiB post to 1000 connections means 64 MiB of `PartyResponse` clones while the global hub mutex is held, then 1000 separate bincode encodings.
- Recommendation: fan out `Arc<PartyResponse>`, or pre-serialized `Arc<[u8]>`, and encrypt per connection.
- Effort: S

### SRV-24 — Maintainability nits
- Severity: Info
- Category: maintainability
- Status: VERIFIED
- Location and problem:
  - `server/src/state/mod.rs:23`: a module-wide `#![allow(dead_code)]` hides dead code across the whole state module.
  - `server/src/state/mod.rs:733`: `pub fn create_channel` bypasses the role and per-member rate checks. Only `create_channel_of_kind` should be public.
  - `server/src/state/db.rs:184` and `db.rs:205`: inconsistent failure policy. A bad role falls back to Guest, while a bad channel kind aborts startup.
  - An unknown request variant (from a newer client) gets the generic `Error("malformed request")` (`connection.rs:201`). The client's FIFO `awaiting` correlation can attribute that error to the wrong pending send.
- Recommendation: narrow the `allow`; make `create_channel` `pub(crate)` or test-only; pick one failure policy; add capability negotiation in `Join` (also listed as T4.1 in `docs/server_tiers.md`).
- Effort: S

## Things done well
- **Authorization is centralised and fails closed.**
  - `role_of` returns `Guest` for unknown ids.
  - `member_can_read_channel` returns false for a missing channel.
  - `effective_perms` checks reachability *before* uploader rights, so excluding someone from a channel also excludes them from files they shared there.
  - `check_may_post_file` is one function used for both the check and the re-check.
- **Private channels are fanned out by member, never broadcast** (`dispatch.rs:255-276`), the channel list is per member with a `DirectoryChanged` nudge, and both are tested end to end over the tunnel.
- **Role escalation rules are correct:** a role can only be granted strictly below your own, nobody can change their own role, the owner cannot be demoted, and a peer admin cannot be demoted.
- **Downloads never confirm that a file exists** (a single `UNKNOWN_FILE_ERROR` for unknown and denied), and access is decided over `file_refs`.
- **The upload pipeline is well designed:**
  - reservations bound peak disk use, not just stored bytes;
  - staging files are per upload id, never the content hash;
  - the hash is accumulated during upload so dedup decisions are exact;
  - there are three reclaim paths (`Drop`, the write-error path, the startup sweep);
  - teardown is per connection;
  - the concurrency count comes from state rather than the connection's own list.
- **Blob I/O is moved off the state lock** (`Deferred` + `spawn_blocking`).
- **The connection loop holds one read future across `select!` iterations**, fixing the cancel-safety bug, with a deterministic regression test. Teardown runs even after a write failure.
- **Party frames carry an in-AEAD sequence number** (replay protection), and `decode_exact` rejects trailing bytes.
- **Joining is careful:** the password comparison is constant-time, `MAX_JOIN_ATTEMPTS` is counted per connection, there is a per-IP accept limiter with a bounded table, and usernames and channel names are checked for hidden or bidirectional characters.
- **The server identity is written atomically with 0600 permissions**, and a damaged key is never silently replaced.
- **SQL is fully parameterised**, and the one interpolated table name is a closed enum (`CountedTable`).
- **The test suite is extensive** and the comments are exceptionally good at explaining *why*.

## Open questions
1. Is the server meant to be run open to the internet with no password? The defaults and the Owner bootstrap (SRV-04) suggest LAN or invite-only use, but nothing enforces or documents that.
2. Should `Guest` be allowed to DM (SRV-08)? The role's docs say no; the code allows text DMs but not file DMs.
3. What retention and size budget is intended for a long-lived community? Everything is in RAM today (SRV-06).
4. Is `DownloadFile` (whole blob) meant to remain in the protocol, or is it legacy now that chunked download exists (SRV-14)?
5. Does the desktop or TUI client handle a reconnect loop caused by a failing history page (SRV-01)? I did not audit the client side; it may back off, or it may hammer the per-IP limiter and lock the user out further (SRV-16).
