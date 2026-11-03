# Performance and resource usage audit

Auditor 11 of 13. HEAD `8d626b8` (after PR #188: `server/src/state/{mod,blobs,db,tests}.rs`, `client/src/tui/app/`).
Static review only: no builds, no benchmarks. Cost figures are estimates from the code shape
(allocation counts, bytes touched, syscall/fsync counts) and typical hardware, and are
labelled as estimates. "VERIFIED" means the code path was read end to end; the magnitude
can still be an estimate.

Some items were already reported by other auditors. For those, this report cites the
earlier finding, adds what was missing (cost model, amplifiers, the exact fix), and does not
repeat the basic description.

## Scope covered

- **Desktop bridge**: `desktop/src-tauri/src/lib.rs` (poll loop, `state_signature`,
  `ui_signature`, `party_signature`, `persist_history`), `commands/chats.rs`,
  `commands/party.rs`, `commands/auth.rs`, `init_bridge` and `run()`.
- **Frontend**: `desktop/src/App.jsx` (refresh fan-out), `lib/bridge.js` (`chatToContact`,
  `fmtTime`, `fmtWhen`), `components/Messages.jsx` (windowing, memoisation, previews),
  `components/Parties.jsx`, `components/Creator.jsx` intervals, `dist/` bundle size.
- **App core**: `client/src/app/persistence.rs` (snapshot, save, load),
  `chat_manager/{events,files,text,mod}.rs` (event drain, chunk receive, transfer lifecycle,
  text reassembly), `client/src/app/party_manager.rs` (history merge, downloads, `poll_events`).
- **TUI**: `client/src/tui/mod.rs` (loop), `tui/app/mod.rs` (`tick`, autosave), `tui/ui.rs`
  (`render_messages`, `wrapped_rows`), logging setup in `client/src/main.rs`.
- **Core**: `core/src/core/framing.rs`, `core/src/core/protocol.rs` (encoder, `Debug`
  redaction), `core/src/core/crypto.rs` (`AesCipher`), `core/src/network/session.rs` (send
  and receive path, rekey cadence, timers), `transfer/receiver.rs`, `identity/mod.rs`
  (Argon2, RSA keygen), `network/ratelimit.rs`, `network/relay.rs`, `network/nat.rs`.
- **Server**: `server/src/connection.rs` (select loop, global lock), `hub.rs` (fan-out),
  `state/mod.rs` (in-memory model, `history_since`, `post_*`, `members`, `channels_for`),
  `state/db.rs` (SQLite schema, pragmas, statement use), `state/blobs.rs` (upload spooling,
  read plan, quota accounting).
- **Build**: workspace `Cargo.toml` profiles, `Cargo.lock` reverse dependencies (`image`),
  `desktop/package.json`, built asset sizes.

## Summary (top 10 hotspots ranked by expected real-world impact)

| # | ID | Hotspot | Why it ranks here |
|---|----|---------|-------------------|
| 1 | PERF-01 | Desktop refresh path: full conversation re-serialized, then remapped in JS with a per-message `toLocaleTimeString`, on every `state-updated` (4 Hz during any transfer) | Roughly 0.1–0.5 s of webview main-thread work per refresh on a 10k-message thread. The UI janks for the whole duration of every file transfer |
| 2 | PERF-02 | A single oversized peer message (up to about 24 MiB) is re-processed on every render or poll: TUI frame, `list_conversations` preview, `get_conversation`, every history save | A peer can trigger this. One message permanently degrades the receiver's UI (TUI near-frozen while the chat is selected; desktop moves about 48 MiB over IPC per refresh) |
| 3 | PERF-03 | Whole-history persistence: deep clone under the manager lock, then serialize, encrypt and fsync of everything, up to every 3 s (desktop) and unconditionally every 30 s (TUI) | O(total history) per save. At 100k messages, about 30 MB is rewritten per save, peak RAM is 4–5× the history size, and the lock is held for the clone |
| 4 | PERF-04 | Server: every SQLite write is its own autocommit transaction (`journal_mode=DELETE`, `synchronous=FULL`, no WAL, no cached statements), executed synchronously under the global `PartyState` lock on a tokio worker | Caps the whole community at roughly 50–150 writes/s, and stalls every member while each fsync runs |
| 5 | PERF-05 | Client community history merge is O(n²): `existing.iter().any(...)` per envelope plus a full sort per 200-envelope page | Joining a 100k-message channel costs about 2.5×10⁹ comparisons and 500 sorts, under the party lock the 4 Hz poll loop needs |
| 6 | PERF-06 | Server history paging scans from message 1, and the server keeps the entire history resident and re-parses all of it at startup | Every catch-up page is O(channel). RSS grows with total history, and a reconnect storm serializes on the global lock |
| 7 | PERF-07 | No receive-side backpressure for P2P file transfers: the session reader pushes every chunk into an unbounded channel, drained only every 250 ms (desktop) or 100 ms (TUI). Chunks are then written with blocking `std::fs` under the manager lock on a tokio worker | Fast network plus slow disk means unbounded RAM. On a 1 Gbps LAN about 30 MB is queued per tick, and disk stalls freeze the UI |
| 8 | PERF-08 | Server upload spools: up to 4 × 100 MiB held in RAM per member, with `Vec` doubling slack and no server-wide in-flight bound (reservations are taken only at `FinishUpload`) | About 0.5 GiB RSS per uploading member. 20 members can push the server past 10 GiB |
| 9 | PERF-09 | Desktop poll loop: 4 Hz unconditional wakeups. Each tick walks every message twice (`state_signature` called directly and again inside `ui_signature`) under the manager lock, plus `party_signature` with a `format!` per server | Idle CPU and battery cost that scales with history (estimated 1–3 ms per pass at 100k messages, so 2–6 ms per tick). It runs even while locked or minimized |
| 10 | PERF-10 | Nagle plus delayed-ACK: no `TCP_NODELAY` and a 4-byte header written separately (already reported). Deepened here with the latency model and the Party server path | Adds up to 40 ms (Linux) or 200 ms (Windows) per small frame on direct and Party connections. Every typed message and every request/response round trip pays it |

Other findings, Medium and below: binary size from `image` default features pulling `rav1e`
(PERF-11); Argon2, RSA and history load running synchronously on async workers, plus no
optimized dev profile for crypto (PERF-12); release builds logging every frame at debug level
(PERF-13); finished outgoing transfers never pruned (PERF-14); fan-out deep clones and the
O(N²) member-directory broadcast (PERF-15); base64 data-URL previews (PERF-16); `useMemo`
defeated by identity churn, plus per-keystroke App-wide recompute (PERF-17); community
downloads assembled whole in RAM and hashed under the party lock (PERF-18); the rate limiter
doing a full `retain` per accept (PERF-19); per-frame copy and realloc chain (PERF-20); text
reassembly bounded by count, not bytes (PERF-21, cited); `party_dm_history`/`dm_messages`
double clone and full-thread IPC (PERF-22); TUI redrawing at 10 fps when idle (PERF-23).

## Findings — ordered by impact

### PERF-01 — Desktop refresh: full conversation over IPC, then O(n) Intl formatting in JS, at 4 Hz during transfers
- Severity: High
- Category: cpu / latency / battery
- Status: VERIFIED (code path). Magnitude SUSPECTED (no profiling).
- Location: `desktop/src-tauri/src/lib.rs:460-487` (`ui_signature` hashes `t.received` for every transfer); `desktop/src-tauri/src/lib.rs:634-638` (emits `state-updated` whenever it changes); `desktop/src/App.jsx:185-218` (`refresh`: five IPCs including `getConversation`); `desktop/src-tauri/src/commands/chats.rs:188-200` (`get_conversation` → `serde_json::to_value(chat)`, the whole chat); `desktop/src/lib/bridge.js:546-571` (`chatToContact` maps every message); `desktop/src/lib/bridge.js:471-483` (`fmtTime` → `toLocaleTimeString([], {hour, minute})` per message).
- Problem: Another auditor reported that the whole open conversation is re-serialized on each `state-updated`. The cost model was missing:
  - **Why it fires at 4 Hz.** `ui_signature` includes `received` of every transfer, so any in-flight transfer, in any conversation, changes the signature on every 250 ms tick. Each tick then triggers a full `refresh()`.
  - **Rust side (per refresh).** `get_conversation` builds a `serde_json::Value` tree: one heap node per field, roughly 8 per message. Tauri then serializes that tree a second time to a string. This happens while holding the `ChatManager` tokio mutex, which the poll loop and every command also need.
  - **JS side (per refresh).** `JSON.parse`, then `chatToContact` maps every message (not just the 150-message render window). Each mapped message calls `fmtTime`. `toLocaleTimeString` with an options object constructs a fresh `Intl.DateTimeFormat` per call. That is a known slow path, especially in WebKitGTK and WKWebView, which are the Tauri webviews on Linux and macOS: typically 10–50 µs per call. File messages also call `human()` and `folderOf()`.
  - **Defeated memo.** `ChatPane`'s `visible` `useMemo` (`Messages.jsx:529-537`) is keyed on `messages`, whose identity changes on every refresh. The comment says it is "recomputed only when the thread … actually changes", but it re-parses 150 `Date`s per refresh.
- Cost estimate (10k-message thread):
  - Rust: about 80k `Value` allocations, about 2 MB of JSON, roughly 5–15 ms.
  - JS: about 5–10 ms of `JSON.parse`, plus 10k × 10–50 µs of `Intl`, which is **0.1–0.5 s of main-thread time per refresh**.
  - At 4 Hz this saturates the webview main thread for the whole transfer: scrolling, typing and the progress bar all stutter.
  - Total per refresh: 5 IPCs (`listConversations`, `listTransfers`, `pendingFingerprint`, `lockState`, `getConversation`).
- Scenario where it hurts: Sending or receiving any file while a long conversation is open. The cost does not depend on which conversation the transfer belongs to.
- Recommendation:
  1. Split the event in two. Emit `transfers-updated` (which triggers only `listTransfers`) when only transfer progress changed, and keep `state-updated` for message, title and trust changes.
  2. Make `get_conversation` incremental. Return the newest N messages (for example 300) plus `total`, or accept `since: message_count` and return only the delta, and append on the JS side.
  3. Serialize with a typed DTO (`#[derive(Serialize)]`) instead of `to_value` into `Value`.
  4. In `bridge.js`, create one module-level `Intl.DateTimeFormat(undefined, {hour:"2-digit", minute:"2-digit"})` and call `.format(d)`. Format lazily inside `MessageItem`, so only rendered rows pay for it.
  5. Key `visible` on `(contactId, messages.length, lastId)` rather than array identity.
- Effort: M

### PERF-02 — One maximum-size peer text (about 24 MiB) is re-processed on every frame, refresh and save
- Severity: High
- Category: cpu / memory / latency
- Status: VERIFIED (paths). Magnitude SUSPECTED.
- Location: `client/src/app/chat_manager/text.rs:151-161` (reassembles up to `MAX_TEXT_CHUNKS` × 48 KiB, about 24 MiB, into one `String` stored in history); `client/src/tui/ui.rs:205-232` (every frame: `text.clone()` and `format!` into `plain_texts`); `client/src/tui/ui.rs:268-272` and `380-410` (`wrapped_rows` walks every word of every message, every frame); `desktop/src-tauri/src/commands/chats.rs:55-58` (`list_conversations` clones the **full** last-message text as the preview); `chats.rs:197` (`get_conversation`); `desktop/src/components/Messages.jsx:16-17` (`LinkifiedText` regex `split` over the whole string on mount).
- Problem: Text is chunked and the documented ceiling is about 24 MiB per message. Nothing downstream treats a message that size differently from a 20-byte one:
  - **TUI.** Each 100 ms frame copies the message twice (about 48 MiB of memcpy), runs a Unicode-width word walk over 24 MiB, and hands ratatui a 24 MiB `Span` that `Paragraph` wraps again. That is easily 100 ms to over 1 s per frame. The TUI is effectively frozen while that chat is selected, and is the slowest pane while anything else is.
  - **Desktop.** If the message is the *last* one in a chat, `list_conversations` ships it as the sidebar preview on every refresh (see PERF-01 for frequency), even though the sidebar shows about 60 characters. `get_conversation` ships it again if that chat is open. That is about 48 MiB of JSON per refresh, `JSON.parse`d on the webview main thread.
  - **History.** Every save (PERF-03) clones, serializes, encrypts and fsyncs it forever.
- Cost estimate: Desktop: about 48 MiB IPC plus parse per refresh, so roughly 0.2–0.5 s each, 4×/s during transfers. TUI: 2 × 24 MiB allocations plus a full width scan per frame, so likely over 200 ms per frame, which drops the TUI below 5 fps.
- Scenario where it hurts: A peer pastes a log file. Or a hostile peer sends one 24 MiB message to make the victim's client sluggish permanently: deleting the conversation is the only remedy.
- Recommendation:
  1. Truncate the preview server-side in `list_conversations` (for example the first 200 chars, cut on a char boundary).
  2. In `get_conversation`, cap per-message text at, say, 64 KiB with `truncated: true` and a `get_message_text(id)` command for "show full message".
  3. In the TUI, render only messages intersecting the viewport (walk backwards from the bottom, summing `wrapped_rows`, until `inner_h + scroll` rows are covered). Cap rendered text per message at a few KiB with a "[… N KiB more]" marker.
  4. Consider storing very large texts as a file attachment rather than inline history text.
- Effort: M

### PERF-03 — Whole-history persistence: O(total history) clone, serialize, encrypt and fsync per save; TUI saves every 30 s unconditionally
- Severity: High
- Category: io / cpu / memory / battery
- Status: VERIFIED
- Location: `client/src/app/persistence.rs:315-330` (`history_snapshot`: `.cloned()` every `Chat`, so every `Message` and `String`, plus all contacts and config); `persistence.rs:165-189` (`save_encrypted`: `serde_json::to_string`, then one-shot AEAD into a new `Vec`, then `nonce.to_vec(); extend_from_slice(&ciphertext)`, which is a fourth full copy, then `write_file_atomic`, which does a temp write, `sync_all` and rename); `desktop/src-tauri/src/lib.rs:128-146` (snapshot taken **under** `manager.lock().await`; only the encrypt and write are moved to `spawn_blocking`); `lib.rs:113` (`HISTORY_SAVE_MIN_INTERVAL = 3s`); `client/src/tui/app/mod.rs:63`, `430-452` (TUI `autosave` every 30 s regardless of change, synchronous on the UI loop: clone, serialize, encrypt and fsync inline); `persistence.rs:134-160` (load: read, decrypt and parse the whole file).
- Problem: Already reported: full-history encrypt+fsync every 30 s in the TUI even when unchanged. The deepening:
  - **Desktop clones under the lock.** The desktop path also deep-clones the entire history while holding the `ChatManager` mutex.
  - **Peak memory.** Peak RAM per save is about 4–5× the history: clone, JSON, ciphertext, then the concatenated output.
  - **Write rate.** The desktop write frequency is bounded only by the 3 s floor.
  - **TUI freezes.** The TUI's save runs on the render loop, so it freezes input for the save's whole duration.
  - **Startup.** Unlock reads, decrypts and parses the whole file synchronously under the manager lock (`commands/auth.rs:80-83`, inside an async command), so startup time grows linearly with history.
- Cost estimate (100k messages, about 250–350 B of JSON per message, so about 30 MB):
  - Clone: about 100k–200k allocations, roughly 15–40 ms under the lock.
  - Serialize: about 60–120 ms. ChaCha20-Poly1305: about 30 ms. Extra copies: about 30 MB of memcpy.
  - fsync of 30 MB: 50 ms on NVMe, more than 500 ms on HDD or a network home directory.
  - Peak RSS: about +120–150 MB.
  - Desktop during an active conversation: one save per 3 s, so about **10 MB/s of sustained writes**, about 36 GB per hour of chatting.
  - TUI: about 30 MB per 30 s even when idle, about 86 GB per day left running.
  - SSD wear plus battery.
- Scenario where it hurts: Any long-lived install. It gets worse monotonically, because there is no pruning and no archive.
- Recommendation:
  1. **Immediate.** Make TUI autosave conditional on a change counter (reuse a `revision` counter, see PERF-09), and move it to `spawn_blocking` like desktop.
  2. **Immediate.** In `persist_history`, take the snapshot outside the lock: copy-on-write `Arc<Chat>`/`Arc<[Message]>`, or clone only chats whose revision changed since the last save.
  3. **Immediate.** Use `encrypt_in_place` on the JSON `Vec<u8>` and write the nonce and ciphertext with two `write_all` calls, to drop two full copies.
  4. **Structural.** Segment persistence per chat (one encrypted file per conversation, rewritten only when that chat changes), or an append-only encrypted log (per-record AEAD, compacted occasionally), or SQLite with per-row AEAD (see the server's pattern). Each of these turns an O(total) save into O(delta).
  5. Make unlock's `load_history_auto` run on `spawn_blocking`.
- Effort: S (items 1–3), L (item 4)

### PERF-04 — Server SQLite: autocommit + DELETE journal + `synchronous=FULL` + no statement cache, all under the global lock
- Severity: High
- Category: io / latency / scalability
- Status: VERIFIED
- Location: `server/src/state/mod.rs:382` (`Connection::open` with no pragmas: SQLite defaults are rollback journal `DELETE` and `synchronous=FULL`); `server/src/state/db.rs:36-152` (every `persist_*` is a separate `conn.execute`: no transaction, no `prepare_cached`); `db.rs:555-570` (`insert_dm_message_row` = **two** autocommit transactions per DM); `db.rs:91-102` (`delete_channel_rows` = three separate transactions, not atomic); `server/src/connection.rs:194-197` (`state.lock().await` then `handle_request(&mut st, …)` performs these writes synchronously on a tokio worker while holding the lock); `db.rs:392-404` (no index on `file_refs(location)` or `(uploader)`, so the channel-delete `DELETE … WHERE location=?` is a full scan).
- Problem: Already reported: every SQLite write is its own fsync under the global lock, with no WAL. The deepening:
  - **fsync count per write.** In DELETE-journal mode with `synchronous=FULL`, one autocommit INSERT costs about 3–4 fsyncs: journal, journal header, database, plus a directory sync on journal deletion on some filesystems.
  - **Lock and worker cost.** These run while the single `tokio::Mutex<PartyState>` is held and the worker thread is blocked, so all members' requests (history fetches, joins, posts) queue behind the disk.
  - **Statement compilation.** Each `conn.execute` re-prepares its statement (SQL parse plus plan, about 5–20 µs). Small next to fsync, but free to remove.
- Cost estimate: Cloud volume or SATA SSD fsync is about 1–5 ms, so about 5–15 ms per post, about 10–30 ms per DM, which is a server-wide ceiling of roughly **50–150 posts/s**. On HDD (about 10 ms per fsync), roughly 25 posts/s. During that time every other request waits on the lock.
- Scenario where it hurts: A busy community, a bot, or a reconnect storm that coincides with chatter. Also a single member spamming DMs throttles everyone.
- Recommendation:
  1. At open: `PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;`. WAL with NORMAL is durable against application crashes and loses at most the last commits on power loss. That is about one fsync per checkpoint instead of 3–4 per write.
  2. Use `conn.prepare_cached(...)` in every `persist_*`.
  3. Wrap multi-statement mutations (DM thread+message, channel delete, upload commit) in `conn.transaction()`.
  4. Move DB writes off the state lock: push `PersistOp`s into a channel to a dedicated writer thread that batches per transaction every few ms. The in-memory state remains authoritative.
  5. Add `CREATE INDEX IF NOT EXISTS file_refs_location ON file_refs(location)`.
- Effort: S (items 1–3, 5), M (item 4)

### PERF-05 — Client community history merge is quadratic
- Severity: High
- Category: cpu / latency / scalability
- Status: VERIFIED
- Location: `client/src/app/party_manager.rs:1026-1068` (`PartyResponse::History`: for each envelope in the page, `existing.iter().any(|e| e.seq == env.seq)`, then `existing.sort_by_key(..)` per page; the full-page path recomputes `dm_thread_id` (SHA-256) for every member to find the DM peer); `party_manager.rs:1019-1024` (live `Message`: O(n) `any` per arrival); `desktop/src-tauri/src/lib.rs:661-683` (`poll_parties` runs `apply` under `party.lock()` on the 4 Hz loop).
- Problem: History arrives in pages of `MAX_HISTORY_BATCH` (200). Merging page k costs 200 × (200·k) comparisons plus an O(n log n) sort. Over a channel of n messages that is about n²/2 comparisons and n/200 sorts. All of it runs inside `poll_events` under the `PartyManager` mutex, which every party command and the desktop poll loop also take.
- Cost estimate: n = 100k gives about 5×10⁹ `seq` comparisons (sequential, but still about 2–5 s of CPU) plus 500 sorts of up to 100k elements (about 0.5–1 s). On join, the desktop's party pane and the 4 Hz loop freeze for seconds. Full pages also trigger `members.len()` SHA-256s each, so 500 × 1000 members is about 0.3 s more.
- Scenario where it hurts: Joining or rejoining any community with a long-running channel. Every app start re-seeds history.
- Recommendation: Keep each thread as a `Vec` sorted by seq plus a `HashSet<u64>` (or `BTreeMap<u64, Envelope>`) of confirmed seqs. Since pages arrive in ascending order after `last_seq`, the common case is an append: check `page.first().seq > existing.last_confirmed_seq` and `extend`, falling back to a merge otherwise. Cache `thread_id → peer` in a `HashMap` when `Members` arrives instead of re-hashing per page.
- Effort: S

### PERF-06 — Server history is fully resident; paging scans from message 1; startup re-parses everything
- Severity: Medium
- Category: memory / cpu / startup / scalability
- Status: VERIFIED
- Location: `server/src/state/mod.rs:1056-1070` (`history_since`: `.iter().filter(|m| m.seq > since_seq).take(200)`); `mod.rs:1109-1120` (`dm_history`, same); `mod.rs:175-183`, `256-260` (`Channel.messages`, `DmThread.messages`: the whole history is kept in RAM); `server/src/state/db.rs:611-629` (`load_messages`: `SELECT envelope … ORDER BY seq`, then `serde_json::from_str` per row, at startup for every channel and thread).
- Problem: Already reported: history paging scans from message 1. The deepening:
  - **The fix is O(1).** `seq == index + 1` is an invariant (stated at `mod.rs:181` and relied on by `post_message`), so the scan can be replaced by `messages.get(since_seq as usize..).take(200)`.
  - **Memory.** Server RSS equals the total message history. Text up to 64 KiB per message, plus `Envelope` overhead, plus per-message `String`.
  - **Startup.** Startup time is O(total messages) of JSON parsing.
- Cost estimate: Catch-up through a 100k-message channel is 500 pages × about 50k average envelopes skipped, which is about 25M envelope touches (about 0.3–1 s) **under the global lock** per joining client. 50 clients reconnecting after a restart means tens of seconds of lock time. RSS: 1M messages × about 300–400 B is about 0.4 GB. Startup is about 2–5 s per million rows.
- Scenario where it hurts: Long-lived communities, and server restarts (every client reconnects and pages at once).
- Recommendation:
  1. Slice by index (O(1) seek).
  2. Longer term, keep only a recent tail (for example the last 1k per channel) in RAM and serve older pages from SQLite with `WHERE channel_id=? AND seq>? ORDER BY seq LIMIT 200` (the primary key already supports it), via `spawn_blocking` outside the lock.
  3. Store envelopes as bincode BLOBs rather than JSON TEXT, to halve parse time and size.
- Effort: S (item 1), M (items 2–3)

### PERF-07 — P2P receive path has no backpressure; chunk writes are blocking I/O under the manager lock
- Severity: Medium
- Category: memory / latency / io
- Status: VERIFIED (mechanism). Magnitude SUSPECTED.
- Location: `client/src/app/chat_manager/connect.rs:92` (`to_app_tx` = `unbounded_channel`); `core/src/network/session.rs` receive loop (forwards each decrypted `FileChunk` immediately); `client/src/app/chat_manager/events.rs:8-31` (`poll_session_events` drains **all** queued events into a `Vec` per tick); `events.rs:655-688` → `core/src/transfer/receiver.rs:301-315` (`IncomingFileSync::write_chunk` = synchronous `std::fs::File::write_all`, unbuffered); `desktop/src-tauri/src/lib.rs:598` (250 ms tick) and `lib.rs:613-614` (`m.poll_session_events()` inside `manager.lock().await`, on a tokio worker); `receiver.rs:323-326` (finalize `sync_all` of up to 10 GiB, under the same lock).
- Problem: The *send* side has a bounded `file_tx` lane (`FILE_LANE_CAPACITY = 8`), but the *receive* side does not:
  - **No TCP flow control.** The session task reads and decrypts as fast as TCP delivers and pushes into an unbounded channel. Because the reader never stops, the TCP receive window never closes and the sender is never slowed.
  - **Batched drain.** The app drains only once per tick, then writes all queued chunks synchronously while holding the `ChatManager` mutex on a runtime worker.
  - **Disk stalls freeze everything.** Any disk stall (USB stick, network share, antivirus scanning the spool, the final `sync_all` of a multi-GB file) blocks every command and the poll loop. On a disk slower than the link, the queue grows without bound.
- Cost estimate: On a 1 Gbps LAN about 30 MB is queued per 250 ms tick, plus a `Vec<SessionEvent>` holding the same chunks again. With a disk at half the link speed, about 60 MB/s of net growth: a 10 GiB transfer can buffer several GB. The final `sync_all` on a 10 GiB file can take seconds while every UI command blocks.
- Scenario where it hurts: Large transfers on fast LANs to slow or removable storage. Also any slow-disk machine.
- Recommendation:
  1. Hand file-data frames to a dedicated per-session writer task over a **bounded** channel (for example 16 chunks). The session reader then awaits `send()`, so TCP backpressure reaches the sender.
  2. That writer uses `tokio::fs` or `spawn_blocking` with a `BufWriter` and reports progress via an `AtomicU64` (mirroring the send side's `spawn_file_stream`).
  3. Run `finalize` (`sync_all` + rename) on `spawn_blocking`, outside the manager lock.
- Effort: M

### PERF-08 — Server upload spools held in RAM: about 0.5 GiB per member, not bounded server-wide
- Severity: Medium
- Category: memory / scalability
- Status: VERIFIED
- Location: `server/src/state/blobs.rs:704-766` (`start_upload`: checks `uploads_in_flight(uploader) < MAX_CONCURRENT_UPLOADS` (4) and the member's quota, which counts stored bytes plus *reservations*; `data: Vec::with_capacity(size.min(1 MiB))`); `blobs.rs:772-795` (`upload_chunk`: `extend_from_slice` into the in-memory `Vec`); `blobs.rs:829` (`begin_upload`, where the reservation is taken only at `FinishUpload`); `blobs.rs:291-306` (`member_blob_bytes` ignores `uploads` spools); `server/src/state/mod.rs:221-223` (the rationale for spooling in memory).
- Problem: Each upload buffers up to `MAX_PARTY_FILE_BYTES` (100 MiB) in RAM until finish.
  - **Per member.** The per-member cap is 4 concurrent uploads, which is 400 MiB of payload. With doubling growth from 1 MiB, a 100 MiB `Vec` ends at 128 MiB capacity, so roughly **512 MiB RSS per member**.
  - **Not covered by the ceilings.** Spools are not counted in `member_blob_bytes` or in the server-wide `MAX_TOTAL_BLOB_BYTES` until `begin_upload`, so neither the 128 MiB member ceiling nor the 1 GiB server ceiling bounds in-flight RAM.
  - **Admins are exempt** from the member limit entirely.
- Cost estimate: N concurrently uploading members × about 0.5 GiB. 20 members is about 10 GiB, which means OOM on a typical small VPS. Each 256 KiB chunk also does a memcpy plus SHA-256 under the global lock (about 0.2 ms; fine).
- Scenario where it hurts: A hostile member with multiple accounts on an open server, or simply a busy day of file sharing on a 1–2 GB VPS.
- Recommendation: Stream chunks to the per-upload staging file as they arrive. Writing happens on `spawn_blocking` after releasing the lock, which already exists as `write_staging`; append there instead of at finish. Then keep only the hasher and byte count in `PendingUpload`. Alternatively, count spool bytes in `member_blob_bytes` and in a global in-flight counter with its own ceiling (for example 512 MiB total), and refuse `start_upload` past it. At minimum, `reserve_exact(declared)` once the first chunk arrives, to remove the doubling slack.
- Effort: M

### PERF-09 — Desktop poll loop: unconditional 4 Hz wakeups, each walking every message twice under the lock
- Severity: Medium
- Category: cpu / battery / scalability
- Status: VERIFIED
- Location: `desktop/src-tauri/src/lib.rs:596-640` (`interval(250ms)`, never paused); `lib.rs:153-197` (`state_signature`: per chat `c.messages.iter().filter(|m| m.delivered).count()`, hashing every title and fingerprint, every contact with all its fields, and a sorted copy of `contact_to_chat`); `lib.rs:460-487` (`ui_signature` calls `state_signature` again, clones every `FileTransferState` (`active_transfers_snapshot`), and calls `chat_ids()`, which allocates a `Vec`); `lib.rs:489-541` (`party_signature`: `format!("{:?}", conn.status)` per server per tick, a sort of the thread list, and hashing of all members, channels and files); `lib.rs:134` (`persist_history` computes `state_signature` a third time).
- Problem: Already reported: `state_signature` walks all messages twice per tick. The deepening:
  - **What else each tick does.** Each tick also takes both mutexes, allocates (`chat_ids`, transfer clones, the `format!`, the `pairs` vector), and runs whether or not the window is visible or the identity is locked.
  - **Wakeup cost.** On laptops the 4 Hz wakeup itself prevents deep CPU idle states.
- Cost estimate: A `Message` is about 96 B, so 100k messages is about 9.6 MB walked per pass. That is about 1–3 ms per pass, about 2–6 ms per tick, about 8–24 ms/s, roughly 1–2 % of a core continuously, with the manager lock held throughout. Plus 4 Hz × (2 mutexes + 4 allocations + N `format!`) at idle.
- Scenario where it hurts: An idle app left open all day on a laptop, which is the normal mode for a messenger.
- Recommendation:
  1. Add `revision: u64` to `ChatManager`, bumped in every mutating method (or via a `touch()` helper), plus a `delivered_count` cached on `Chat` and updated where `delivered` flips. `state_signature` then becomes O(1).
  2. Replace the fixed tick with an event-driven wake. Each session already owns an `mpsc`, so a shared `tokio::sync::Notify` signalled by session tasks and `PartyManager` can wake the loop, with a slow fallback interval (for example 2 s, used for UPnP, rehost and toast expiry).
  3. Use `MissedTickBehavior::Delay`.
  4. Compute `ui_signature` from `state_signature`'s value instead of recomputing it.
  5. Hash the `PartyStatus` discriminant instead of `format!("{:?}")`.
- Effort: M

### PERF-10 — Nagle plus delayed ACK on direct-P2P and Party connections (latency model)
- Severity: Medium
- Category: latency
- Status: VERIFIED (code). Latency figure from TCP behaviour.
- Location: `core/src/core/framing.rs:20-26` (`write_all(&header)` then `write_all(payload)` then `flush`); `rg set_nodelay` finds only `core/src/network/punch.rs:357`. Direct host/connect sockets (`client/src/app/chat_manager/connect.rs`, `core/src/network/session.rs`), the relay-bridged path, and the Party server and client (`server/src/connection.rs`, `core/src/party/mod.rs` `send_framed`) never set it.
- Problem: Already reported (no `TCP_NODELAY`, two writes). The model:
  - Nagle sends the 4-byte header immediately and then holds the payload, because it is smaller than one MSS and there is unacknowledged data in flight.
  - The receiver's delayed-ACK timer (40 ms on Linux, up to 200 ms on Windows) is waiting for a response to piggyback on, and the protocol sends none.
  - Each small frame (text, typing indicator, Ack, Party request) therefore pays up to one delayed-ACK interval.
  - On the Party path every request/response round trip is affected, and join pipelines several requests in a row.
  - Hole-punched sessions are *not* affected (`punch.rs:357`). Direct-connect is the default LAN path and is affected.
- Cost estimate: Up to +40 ms (Linux) or +200 ms (Windows) per message or request whenever a previous segment is still unacknowledged. Most visible for chat bursts and typing indicators.
- Recommendation:
  1. Build one buffer `[len_be | payload]` and do a single `write_all` (one syscall, one segment).
  2. Call `set_nodelay(true)` on every accepted and connected `TcpStream` in `session.rs`, on the relay bridge sockets, and on the Party server's `accept` and client `connect`.
- Effort: S

### PERF-11 — `image` default features compile AV1, EXR, TIFF, WebP and rayon only to encode one PNG
- Severity: Medium
- Category: binary-size / startup / build time
- Status: VERIFIED
- Location: `Cargo.toml` (workspace) `image = "0.25.9"` with default features; `client/Cargo.toml:63`; sole use `client/src/app/chat_manager/invites.rs:137-144` (`ImageLuma8 → write_to(.., ImageFormat::Png)`); `Cargo.lock`: `image → ravif → rav1e`, plus `exr`, `tiff`, `image-webp`, `gif`, `qoi`.
- Problem: The QR invite PNG needs only the `png` codec. `rav1e` (the AV1 encoder) is among the largest Rust crates in compile time and code size. `rayon` adds a thread pool. All of it links into both the TUI and the shipped desktop app, which depends on `p2pem-classic`.
- Cost estimate: Several MB of release binary (rav1e alone is commonly 2–4 MB of `.text`), plus a large share of clean-build time (relevant to the build auditor). It also adds dependency attack surface.
- Recommendation: `image = { version = "0.25.9", default-features = false, features = ["png"] }`. Or drop `image` entirely and use `qrcode`'s SVG or a raw `png` crate encoder. Verify afterwards with `cargo tree -i rav1e` (it should be empty).
- Effort: S

### PERF-12 — Argon2, RSA and history load run synchronously on async workers and hold locks; no optimized dev profile for crypto
- Severity: Medium
- Category: startup / latency
- Status: VERIFIED (code). Timings SUSPECTED.
- Location: `desktop/src-tauri/src/commands/auth.rs:66-85` (`unlock`: `id.decrypt(&password)` (Argon2id, m=64 MiB, t=3, p=4) on the async worker while holding the identity `std::sync::Mutex`, then `load_history_auto` under the manager lock); `auth.rs:115-140` (`set_password`: Argon2 in `encrypt` **and again** in `decrypt`, back to back); `core/src/identity/mod.rs:280-290` (params); `desktop/src-tauri/src/lib.rs:349` → `init_bridge` → `Identity::get_or_create` (RSA-2048 keygen on the main thread before the window exists on first run; the error path at `lib.rs:415-418` also generates a throwaway RSA key just to show an error); `lib.rs:693-703` (`auto_rehost` calls `lock_identity` (std mutex) from the poll loop every 1.5 s, so it blocks a worker for the length of an Argon2 run); the workspace `Cargo.toml` has no `[profile.dev.package.*]`.
- Problem:
  - **Argon2 cost.** The `argon2` crate runs `p_cost=4` lanes sequentially unless built with its `parallel` feature (not enabled), so wall time is the full t × m cost.
  - **Unlock blocks other work.** Unlock blocks a runtime worker for the Argon2 run, and the poll loop's `auto_rehost` also blocks on the same std mutex.
  - **First-run window delay.** On first run, window creation waits for RSA keygen (variable: 50 ms to over 1 s in release).
  - **Debug builds.** Without `opt-level` for `rsa`, `num-bigint-dig`, `argon2`, `blake2`, `aes-gcm`, `sha2` and `chacha20poly1305` in dev builds, keygen and Argon2 take seconds to tens of seconds. That hurts `cargo run`, `tauri dev` and the test suite (many tests generate RSA keys).
- Cost estimate: Release: about 0.3–1 s per Argon2, so `set_password` takes about 2× that. Debug: Argon2 at 64 MiB × 3 passes is roughly 5–20 s, and RSA-2048 keygen roughly 5–60 s.
- Recommendation:
  1. Wrap `decrypt`/`encrypt` and `load_history_auto` in `tokio::task::spawn_blocking`, as `change_password` already does at `auth.rs:167`.
  2. In `set_password`, derive the key once and reuse it, rather than encrypting and then decrypting.
  3. Generate the first-run identity lazily (on the `set_password` screen, off-thread) or in `setup()` after the window shows.
  4. Add:
     ```toml
     [profile.dev.package."*"]
     opt-level = 2
     ```
     (or list just the crypto crates), which typically cuts debug-test crypto time by 10–50×.
  5. Optionally enable `argon2/parallel` (rayon is already in the tree via `image`, though PERF-11 removes it).
- Effort: S

### PERF-13 — Release builds log every frame at debug level
- Severity: Low
- Category: cpu / io
- Status: VERIFIED
- Location: `desktop/src-tauri/src/lib.rs:330-336` (default filter `info,p2pem_classic=debug,messenger_core=debug` when `RUST_LOG` is unset, so it applies in release); `core/src/network/session.rs:1124` (`debug!("Sending message: {:?}", msg)`) and `:1210` (`Received message`); `client/src/app/chat_manager/events.rs:672` (`debug!("Received file chunk …")`); `client/src/main.rs:77-81` (TUI: `p2pem_classic=debug` into the in-process `LogBuffer`, under a global std `Mutex` per event).
- Problem: Every frame on both lanes is formatted and written to stdout, which is line-buffered: one `write` syscall per line. The `Debug` impl correctly redacts text and chunk bytes (`protocol.rs:82-149`), so this is cost, not leakage.
- Cost estimate: A 10 GiB transfer is about 164k chunks per side, each producing 2–3 formatted lines and syscalls, so about 0.5M syscalls plus formatting: a few seconds of CPU per transfer. Rekey and ping messages add noise.
- Recommendation: Default to `info` in release (`cfg!(debug_assertions)` picks `debug`). Downgrade the per-frame lines to `trace!`.
- Effort: S

### PERF-14 — Finished outgoing transfers are never pruned; every tick clones all of them
- Severity: Low
- Category: memory / cpu
- Status: VERIFIED
- Location: `client/src/app/chat_manager/events.rs:268-273` (`FileSendComplete` sets `Completed` but leaves the entry in `active_transfers`); `client/src/app/chat_manager/files.rs:269`, `327` (Cancelled), `350`, `389` (Failed), all left in place; the only bulk cleanup is `files.rs:413-415`, which runs only when a **new incoming** file starts on that chat; `client/src/app/chat_manager/mod.rs:627-629` (`active_transfers_snapshot` clones all of them); consumers are `lib.rs:475` (`ui_signature`, 4 Hz), `commands/chats.rs:141-171` (`list_transfers`, per refresh) and `tui/ui.rs:245` (per frame).
- Problem: A user who sends many files accumulates `FileTransferState` entries (with `filename` `String`s) for the life of the process. All are cloned 4×/s and shipped to the UI on each refresh, and the UI keeps rendering "done" cards.
- Cost estimate: Small per entry (about 150 B), but O(files sent) per tick and per refresh, unbounded over a long session.
- Recommendation: Drop terminal-state transfers after a short grace period (for example 10 s after completion, keyed by a `finished_at: Instant`), or move them to a bounded "recent" ring (for example the last 20).
- Effort: S

### PERF-15 — Hub fan-out deep-clones and re-serializes per connection; member directory broadcast is O(N²) on join storms
- Severity: Low (Medium for large communities)
- Category: cpu / memory / scalability
- Status: VERIFIED
- Location: `server/src/hub.rs:61-70`, `85-95`, `101-107` (`resp.clone()` of the whole `PartyResponse` per connection, under a std `Mutex`); `server/src/connection.rs:182` (each connection independently `push.to_bytes()`, which is bincode, then encrypts); `connection.rs:268-280` (leave: `st.members()` (clone and sort of every member) then `broadcast_all(Members(directory))`; join does the same, as already reported); `server/src/state/mod.rs:663-677` (`members()` = O(M log M) per call).
- Problem: A 64 KiB post to N connections makes N deep copies plus N bincode encodings of the same value. The directory broadcast on each join or leave sends M entries to N connections. When a restarted server sees all M clients reconnect, that is about M² `MemberInfo` clones, serializations and AES-GCM seals, all funnelled through bounded 256-slot queues that can then overflow and disconnect clients (`BROADCAST_QUEUE_DEPTH`).
- Cost estimate: M = 1000 gives about 10⁶ `MemberInfo` encodings (about 50 B each, about 50 MB of serialization plus encryption) during a reconnect storm. A 64 KiB post × 500 connections is about 32 MB of transient clones.
- Recommendation: Make the hub carry `Arc<PartyResponse>`, or better, pre-encode once to `Arc<[u8]>` (plaintext bincode) and have each connection only seal it. Coalesce directory updates: mark "directory dirty" and broadcast at most every 500 ms, or send `MemberPresence { id, online }` deltas instead of the full list.
- Effort: M

### PERF-16 — Image previews as base64 `data:` URLs over JSON IPC
- Severity: Low
- Category: memory / cpu
- Status: VERIFIED
- Location: `desktop/src-tauri/src/commands/chats.rs:486` (`PREVIEW_MAX_BYTES = 4 MiB`), `492-516` (read the whole file, `STANDARD.encode`, `format!` into a `String`, then JSON-escape over IPC); `desktop/src/components/Messages.jsx:405-419`, `452-479` (up to `MAX_CACHED_PREVIEWS = 24` cached; fetched sequentially).
- Problem: Each preview costs the original bytes, plus about 1.33× as base64, plus a JSON string copy, plus a JS string (UTF-16, so 2× again), plus the decoded bitmap. The cap and the LRU-ish trim are good, but the worst case is still about 24 × 4 MiB × about 2.7 ≈ 260 MB of string memory in the webview before decoding, and every preview fully downscale-decodes a full-resolution photo.
- Cost estimate: For a typical 3–4 MB phone photo: about 10 MB of JS heap plus about 48 MB decoded (12 MP RGBA) per preview.
- Recommendation: Generate a thumbnail in Rust. `image` is already a dependency; add the `jpeg` and `png` decoders and `thumbnail(320, 320)` → small WebP or PNG of about 20–40 KB, cached on disk by message id. Alternatively serve via a custom `asset:`-style protocol handler (no base64 or JSON) if the CSP allows it.
- Effort: M

### PERF-17 — React memoisation defeated by identity churn; whole-App recompute on every keystroke
- Severity: Low
- Category: cpu / latency
- Status: VERIFIED
- Location: `desktop/src/App.jsx:83` (`draft` state lives in `App`, so each keystroke re-renders `App`); `App.jsx:265` (`const contacts = convs.map(summaryToConv)` every render: `fmtWhen` makes 2 `toDateString` calls plus `toLocale*` per conversation); `desktop/src/components/Messages.jsx:151-154` (`filtered` `useMemo` keyed on the new `contacts` array, so it recomputes every render); `Messages.jsx:448-451` (`previewKey` string rebuilt over 150 messages every render); `Messages.jsx:529-537` (`visible`, see PERF-01); `desktop/src/components/Parties.jsx:174`, `676-678` (`MessageRow` is not memoised and is keyed by **index** within a sliding `slice(-shown)`, so one new message shifts every key and all 150 rows re-render on each `party-updated`).
- Problem: `ConvRow` and `MessageItem` have value comparators (good), but the work feeding them is redone on each keystroke and each poll-driven refresh.
- Cost estimate: 200 conversations × about 3 Intl calls ≈ 600 × 10–30 µs ≈ 6–18 ms per keystroke in WebKit. That is visible input lag on low-end machines.
- Recommendation: Move `draft` into `ChatPane` (or a composer component) with a ref callback for send. Memoise `contacts` on `convs`. Hoist one shared `Intl.DateTimeFormat`. In `Parties.jsx`, key rows by `${m.seq}:${m.sender}` (or the envelope id) and wrap `MessageRow` in `memo`.
- Effort: S

### PERF-18 — Community downloads are assembled whole in RAM and hashed in one pass under the party lock
- Severity: Low
- Category: memory / latency
- Status: VERIFIED
- Location: `client/src/app/party_manager.rs:1134-1180` (`FileChunk`: `state.data.extend_from_slice(&data)`, then `blob_hash(&state.data)` over up to 100 MiB at the end); `party_manager.rs:1087-1100` (`FileData`, same); called from `poll_events` under the `PartyManager` mutex (desktop `lib.rs:662-663`, 4 Hz).
- Problem: A 100 MiB download is buffered fully (with doubling slack, about 128 MiB), then SHA-256'd in one go inside the poll loop's critical section.
- Cost estimate: SHA-256 at about 0.5–2 GB/s is about 50–200 ms holding the party lock, plus 100–128 MiB RSS per concurrent download.
- Recommendation: Keep an incremental `Sha256` in the download state (update per chunk), and stream chunks to a temp file in the target directory, renaming on hash match. This mirrors the server's `PendingUpload::hasher`.
- Effort: S

### PERF-19 — Rate limiter does a full `retain` over up to 4096 entries on every accept
- Severity: Low
- Category: cpu / scalability
- Status: VERIFIED
- Location: `core/src/network/ratelimit.rs:162-191` (`check_at`: `self.attempts.retain(..)` over every tracked address, each scanning up to `MAX_CONNECTIONS` timestamps, plus `evict_stalest` (O(n)) when full); callers: relay accept loop `core/src/network/relay.rs:178` (under a tokio `Mutex`), the Party server accept loop, and the host path in `session.rs:56`.
- Problem: The accept loop is single-threaded, so per-accept cost bounds the accept rate exactly when it matters (a flood).
- Cost estimate: 4096 × (up to 10 timestamps) is about 40k operations, about 20–60 µs per accept. A 10k connections/s flood costs about 0.2–0.6 s of CPU per second in the accept task, which then falls behind and lets the SYN backlog fill.
- Recommendation: Prune at most once per second (track `last_prune`), or amortise with a `VecDeque` ordered by `last_seen`. Make `evict_stalest` O(1) with that queue.
- Effort: S

### PERF-20 — Per-frame copy and realloc chain on the hot transfer path
- Severity: Low
- Category: cpu / memory
- Status: VERIFIED
- Location: `core/src/core/protocol.rs:361-365` (`to_plain_bytes` starts from `Vec::new()`, so a 64 KiB chunk frame grows through about 7 reallocations); `core/src/core/crypto.rs:617-666` (`encrypt` returns `ciphertext`, then copies it into a new `output` with the nonce prepended); `core/src/core/framing.rs:58-70` (`recv_packet` zero-fills via `resize` in 64 KiB steps); decode copies the chunk again into `FileChunk { chunk: Vec<u8> }`.
- Problem: Each 64 KiB chunk is copied about 4–5 times per side plus reallocations. This is not dominant against AES-GCM, but free to fix.
- Cost estimate: At 110 MB/s (gigabit), about 0.5 GB/s of extra memcpy and allocator traffic, roughly 5–10 % of one core.
- Recommendation:
  - Add `Vec::with_capacity(1 + 8 + 4 + payload_len)` in `to_plain_bytes`.
  - Have `AesCipher::encrypt` write the nonce into a buffer and then `encrypt_in_place_detached` / `encrypt_in_place` on the tail.
  - In `recv_packet`, allocate `vec![0; total_len]` once (the length is already validated against `MAX_PACKET_SIZE`), or keep the chunked growth but reuse a per-session buffer.
  - Bump `FILE_CHUNK_SIZE` to 256 KiB, which would quarter per-frame overhead, the number of rekeys (every 100 frames = 6.4 MiB today) and the debug logs. Wire impact: chunk size is sender-chosen, but receivers cap `FileChunk` frames, so check against `MAX_PACKET_SIZE` and the decoder caps before changing.
- Effort: S

### PERF-21 — Chunked-text reassembly bounded by count, not bytes (cited; cost added)
- Severity: Low (memory DoS)
- Category: memory
- Status: VERIFIED
- Location: `client/src/app/chat_manager/text.rs:120-161` (`MAX_CONCURRENT_PARTIAL_TEXT_PER_CHAT` (16) partial messages, each up to 512 × 48 KiB; `parts.iter().all(Option::is_some)` per chunk, then `join("")`).
- Problem: Already reported. Cost: 16 × 24 MiB = **384 MiB per chat** of held text for up to 120 s. Completion needs an extra 24 MiB for `join`, so peak is about 2× one message. The per-chunk `all()` is O(k²) per message (512² ≈ 262k checks, negligible). After completion, the message feeds PERF-02.
- Recommendation: Add a per-chat byte budget (for example 32 MiB across partials). Track `received_parts: u32` instead of re-scanning. Pre-size the output with `String::with_capacity(sum_len)`.
- Effort: S

### PERF-22 — `party_dm_history` / `party_history` ship whole threads on every `party-updated`, with a double clone for DMs
- Severity: Low
- Category: cpu / latency
- Status: VERIFIED
- Location: `desktop/src-tauri/src/commands/party.rs:415-433` (`party_history` maps the full channel to DTOs); `party.rs:493-511` (`party_dm_history`: `dm_messages()` clones the whole thread Vec at `client/src/app/party_manager.rs:550-561`, then maps it again); `desktop/src/components/Parties.jsx:271-274` (each `party-updated` calls `loadServers()` **and** `loadMsgs()`); `desktop/src/App.jsx:228-230` (`party-updated` also calls `partyList()` again for the rail badge, so `server_dto` runs twice per event; `server_dto` (`party.rs:175-191`) computes a SHA-256 `dm_thread_id` per member per call).
- Problem: `party_signature` includes presence, so any member going online or offline on any joined server triggers a full re-ship of the open thread plus two directory builds.
- Cost estimate: 10k-message channel: about 1–3 MB of JSON plus mapping per presence blip. 1000 members means 2000 SHA-256 per event (about 1 ms). Small individually, but frequent in big communities.
- Recommendation: Share one `partyList` result between App and `Parties` (lift the state, or emit the DTO with the event). Return only the tail or a delta for history (`since_seq`). Make `dm_messages` return a slice, not a clone. Cache `dm_thread_id` per member.
- Effort: S

### PERF-23 — TUI redraws at 10 fps when idle and does O(history) per frame
- Severity: Low
- Category: cpu / battery
- Status: VERIFIED
- Location: `client/src/tui/mod.rs:153-182` (`tick()` and `terminal.draw` on every loop iteration; `event::poll(100ms)`, a blocking call inside an async fn on the `#[tokio::main]` thread); `client/src/tui/app/mod.rs:366-395` (`tick` calls `poll_session_events`, `party_manager.poll_events`, `active_transfers_snapshot` (per frame in `ui.rs:245`), `mark_chat_read`, `report_presence`, `refresh_status_line` and `autosave` every iteration).
- Problem: Already reported: the TUI re-wraps the whole conversation every frame. The deepening:
  - Even with nothing happening, the TUI rebuilds `Line`s for **every** message of the selected chat (`ui.rs:205-232`: a `chrono` `format` plus two `String` allocations per message) 10×/s.
  - `wrapped_rows` folds into a `u16`, so beyond 65,535 rendered rows the total saturates and scrolling clamps wrongly. That is a functional limit as well as a cost.
- Cost estimate: 10k messages × (1 `chrono` format + 2 allocations + width scan) is about 3–10 ms per frame, so about 3–10 % of a core while idle.
- Recommendation: Redraw only on input, a session or party event, or a 1 s clock tick. A `tokio::select!` over `crossterm::event::EventStream` and a `Notify` replaces the blocking poll. Cache per-message rendered `Line`s and their row counts keyed by `(msg.id, width)`. Render only the viewport slice. Compute heights in `u32` or `usize`.
- Effort: M

## Things done well

- **Send-side flow control is right.** File data rides a bounded lane (`FILE_LANE_CAPACITY = 8`) with `send().await` backpressure. The stream runs in a spawned task off the manager lock, with atomic progress and cancel flags (`client/src/app/chat_manager/files.rs`, `mod.rs:53`).
- **Hot-path logging is redacted and cheap to format.** `ProtocolMessage`'s hand-written `Debug` prints lengths, never chunk bytes or message text (`core/src/core/protocol.rs:82-149`), so the debug-level logging in PERF-13 is a cost issue, not a 64 KiB `{:?}` dump.
- **History writes are already coalesced on desktop.** There is a shared `saved_sig` so a command's save and the next tick do not double-write, a 3 s floor, an off-thread encrypt and write, compact rather than pretty JSON, and an atomic fsynced write (`desktop/src-tauri/src/lib.rs:98-146`, `client/src/app/persistence.rs:165-189`).
- **UI events fire only on change.** `state-updated` and `party-updated` are gated on signatures, so an idle app does not make the webview refetch at 4 Hz (`lib.rs:634-645`). The waste is in computing the signatures (PERF-09), not in emitting.
- **Rendering is windowed and memoised by value in the main chat.** `MSG_WINDOW = 150`, `MessageItem` with a field-wise comparator, `ConvRow` with `convRowsEqual`, and a bounded preview cache (`MAX_CACHED_PREVIEWS = 24`) that keeps a `requested` set so evicted previews do not refetch-loop (`desktop/src/components/Messages.jsx`).
- **Server blob I/O is correctly off the lock.** `plan_blob_read`/`perform` and the three-phase staged upload run disk work on `spawn_blocking` after releasing the state lock. Disk-backed blobs keep `data: None`, so storage is not RSS (`server/src/state/blobs.rs`).
- **Server broadcast queues are bounded** (`BROADCAST_QUEUE_DEPTH = 256`, `try_send`, stuck clients dropped). `MAX_HISTORY_BATCH` keeps every reply inside one frame. `MAX_CHANNELS`, per-member and global blob ceilings, `MAX_PENDING_RENDEZVOUS` and `MAX_TRACKED_ADDRESSES` bound the obvious growth vectors.
- **`recv_packet` validates the length before allocating** and grows in 64 KiB steps, so a forged 8 MiB header cannot force an 8 MiB allocation up front (`core/src/core/framing.rs`).
- **Timers are sane.** The session keep-alive is 120 s with a 300 s idle timeout, rekeying is every 100 frames or 300 s (cheap HKDF), the NAT-PMP retry is bounded at 20 × 100 ms, and the `Creator.jsx` intervals (2–3 s) run only while their pane is open.
- **The frontend bundle is lean.** About 354 KB of JS and 72 KB of CSS in a single chunk (`desktop/dist/assets`). Only React, `@tauri-apps/api` and named `lucide-react` imports funnelled through one `Icon.jsx`, so tree-shaking works.
- **Release profile** uses `lto = true`, `codegen-units = 1`, `strip = "symbols"` and `panic = "abort"`, which is appropriate for binary size and speed.
- **No message content duplication for files.** `MessageContent::File` stores a path, never bytes, so file traffic does not bloat history (`core/src/types.rs:254-264`).
