# Client application core (ChatManager, persistence, PartyManager, receiver)

## Scope covered

Read in full:
- `client/src/app/mod.rs`, `client/src/app/chat_manager/{mod,connect,contacts,events,files,invites,text}.rs`
- `client/src/app/persistence.rs`, `client/src/app/party_manager.rs` (logic in full; test module by name and spot-read)
- `client/src/support.rs`, `client/src/logbuf.rs`, `client/src/lib.rs`, `client/src/main.rs`
- `core/src/types.rs`, `core/src/transfer/{mod,receiver}.rs`
- Tests, for coverage: `chat_manager/tests.rs` and `adversarial_tests.rs` (by test name), `client/tests/{adversarial_persistence,history_scaling_probe,reconnect_e2e,file_transfer_e2e,attack_trust_boundaries}.rs` (by name, with the relevant bodies read).

To confirm behaviour at the boundaries I also read these, outside my scope: `core/src/network/session.rs` (the `send`/`receive` loop, `connect_first_reachable`, `run_client_session_multi`), `core/src/core/protocol.rs` (the `Debug` impl and filename sanitising), `server/src/dispatch.rs` (upload replies), `desktop/src-tauri/src/lib.rs` (`persist_history`), `desktop/src-tauri/src/commands/{auth,chats,party}.rs` (lock usage and load-error handling), and `client/src/tui/app/mod.rs` (history loading).

Constraints: static analysis only. I did not run any build or test.

## Summary

The core is in good shape overall. It carries many careful fixes for earlier bugs and has almost no `unwrap` in production paths. The serious findings are in failure paths:

1. **A history file that fails to load is silently replaced by an empty one (APP-01, High).** The load error is only logged. The shells then keep running with an empty manager, and the next save (the poll loop, a command, or window close) overwrites the encrypted history. Triggers include a OneDrive placeholder that is not available offline, a downgrade that meets a newer `version` / `Theme` / `MessageContent` variant, or a damaged file. CLAUDE.md already enforces "never replace what merely failed to load" for `identity.json`. History, which holds every message, has no such rule.
2. **`send_file` holds the manager lock across a bounded-channel `send().await` (APP-02).** With a stalled peer, this freezes the whole UI.
3. **File-transfer failure paths leak or mislead (APP-03 to APP-07, APP-12):**
   - an orphaned `tmp_` spool when finalize fails;
   - no `FileCancel` after a local disk-write error;
   - a fully received offer is destroyed when the peer disconnects before the user accepts;
   - `FileCancel` cannot say which transfer it means, so simultaneous send+receive cancels the wrong one;
   - unaccepted, peer-controlled bytes sit in the user's Downloads folder, and spools are not cleaned up on exit or crash.
4. **Party client correctness (APP-08, APP-10, APP-11):**
   - history paging can skip a range of messages when a live broadcast lands before the first page;
   - unconfirmed sends stay on screen, and upload buffers stay in memory, after a disconnect;
   - `connect_and_join` has no TCP connect timeout while the desktop holds the `PartyManager` lock.
5. **Chunked-text reassembly is capped by message count, not bytes (APP-09).** That allows about 384 to 512 MiB of buffered text per conversation.
6. **Answers to the coordinator's questions:**
   - **APP-13 (confirmed):** `accept_incoming_connection` falls back to the client-offered chat id with no liveness or fingerprint check, so it can remap a live conversation's session.
   - **APP-14 (confirmed):** `connect_to_contact` never falls back to the relay after the direct dial fails at runtime. It uses the relay only when the contact has no parsable direct address.

## Findings (ordered by severity)

### APP-01: A failed history load is followed by a save that overwrites the history with empty state
- **Severity:** High
- **Category:** bug (data loss)
- **Status:** VERIFIED
- **Location:**
  - `client/src/app/persistence.rs:205-240` (`load_history_auto` returns `Err`; state is left untouched)
  - callers: `desktop/src-tauri/src/commands/auth.rs:81-83` and `:137`, `client/src/tui/app/mod.rs:153-155` (each only `tracing::warn!`)
  - writer: `desktop/src-tauri/src/lib.rs:128-145` (`persist_history`, documented as "An unlocked but empty history IS saved on purpose")
  - `client/src/tui/app/mod.rs:445-449` (`save_history_now`)
- **Problem:** The history key is set before the load (`set_history_key`), the error is swallowed, and the manager runs on with empty chats and contacts. Any later state change makes the desktop poll loop write a new `history.json.enc`. The window-close flush and the TUI's `save_history_now` do the same. `adversarial_persistence::damaged_history_fails_cleanly_and_changes_nothing` checks only that the *manager* is unchanged. Nothing checks that the *file* survives what the shell does next.
- **Impact:** Every conversation and contact is permanently replaced by an empty file. It looks like a fresh install, which is the same failure CLAUDE.md forbids for `identity.json`. Realistic triggers:
  - a `history.json.enc` in a OneDrive / iCloud folder that is not available offline (the read fails);
  - an EIO;
  - a downgrade after a newer build saved `version: "1.2"`;
  - a newer build that added a `Theme` or `MessageContent` variant. These are closed serde enums (`core/src/types.rs:252-264`, `432-439`) with no `#[serde(other)]`, so one unknown value fails the whole `HistoryFile` deserialize.
- **Recommendation:**
  - Make a failed load sticky. For example, add `ChatManager::history_load_failed: bool` (or a `HistoryState::Unreadable`), have `save_history` / `history_snapshot` refuse while it is set, and show the error to the user, as `auth_status.state == "error"` does for the identity.
  - Before any overwrite, move the unreadable file aside (`history.json.enc.corrupt-<ts>`).
  - Add `#[serde(other)]` fallback variants to `Theme`, or deserialize `config` leniently (it is `#[serde(default)]`, but an invalid `theme` value still fails).
  - Add an integration test that runs load-fail, then save, and asserts the original bytes are still on disk.
- **Effort:** M

### APP-02: `send_file` awaits the bounded file lane while the caller holds the `ChatManager` lock
- **Severity:** Medium
- **Category:** bug / performance
- **Status:** VERIFIED
- **Location:**
  - `client/src/app/chat_manager/files.rs:46-92` (`validate_outgoing_file(...).await`, then `file_tx.send(FileMeta).await`)
  - caller: `desktop/src-tauri/src/commands/chats.rs:239-245` (`state.manager.lock().await.send_file(...).await`); the TUI awaits it inline (`client/src/tui/app/cmd_misc.rs:14-18`)
- **Problem:** `file_tx` has capacity `FILE_LANE_CAPACITY` (8). A cancelled earlier send leaves chunks queued in the lane, and `cancel_transfer` makes the chat eligible for a new send immediately. If the session's writer is then blocked (a peer that stopped reading, or a stalled TCP path), `send(FileMeta).await` waits indefinitely while the manager lock is held. `validate_outgoing_file` also awaits `metadata` and `open` under the lock, which can take seconds on a cloud placeholder file.
- **Impact:** Every Tauri command and the desktop poll loop block on the manager mutex. Toasts, events and saves all stop: the app appears frozen until the peer drains the lane or the socket dies.
- **Recommendation:** Use `file_tx.try_send(FileMeta)` and return an "outbound queue busy, try again" error. Alternatively, move the `FileMeta` send into the spawned stream task, and register the transfer before releasing the lock. Validate the file before taking the lock; the bridge can call a free function.
- **Effort:** S

### APP-03: A received file whose finalize fails leaves its spool, and possibly an empty placeholder, in Downloads
- **Severity:** Medium
- **Category:** bug / security (disk litter)
- **Status:** VERIFIED
- **Location:**
  - `core/src/transfer/receiver.rs:323-355` (`IncomingFileSync::finalize` consumes `self`; on a size mismatch or rename error it returns `Err` without removing `tmp_path`, and a reserved `create_new` placeholder is left behind if the rename fails)
  - `client/src/app/chat_manager/files.rs:556-563` (the error branch only drops the transfer row)
- **Problem:** A peer that declares `size = N` and sends `FileEnd` after fewer bytes makes finalize fail with "Size mismatch". The `tmp_<uuid>_<name>` file with the partial data stays in the download directory. It is invisible to the app, and nothing else reaches it (see APP-07).
- **Impact:** With `auto_accept_files` on, or after the user accepts a held offer, a peer can repeatedly leave arbitrary partial content in `~/Downloads`. Those bytes then fall outside any cap, because the transfer row is gone. Ordinary failures (disk full during the rename) leak too.
- **Recommendation:** In `IncomingFileSync::finalize`, remove `tmp_path` on every error path, and the reserved placeholder if the rename fails. A drop guard is the robust way. Add a test: a short `FileEnd` must leave the download directory empty.
- **Effort:** S

### APP-04: A local disk-write error on receive does not tell the sender to stop
- **Severity:** Medium
- **Category:** bug
- **Status:** VERIFIED
- **Location:** `client/src/app/chat_manager/events.rs:676-686` (`receive_file_chunk`, the `Err(e)` branch)
- **Problem:** On a `write_chunk` error (disk full, permission, or a peer overflowing its declared size), the transfer is marked `Failed` and the spool is discarded. No `FileCancel` is sent. Compare `refuse_file_offer` (`events.rs:645-652`) and `reject_incoming_file` (`files.rs:506`), which both send it.
- **Impact:** The sender keeps streaming the rest of a file of up to 10 GiB over the wire, and the receiver drops every byte. The sender then gets `FileSendComplete` and shows "File sent: …", with a delivery receipt that never arrives. A full disk is precisely when the remaining bytes are useless.
- **Recommendation:** Call `self.send_control(chat_id, ProtocolMessage::FileCancel { seq: 0 })` and `self.incoming_streams.remove(&chat_id)` in that branch.
- **Effort:** S

### APP-05: A fully received offer awaiting the user's decision is destroyed when the peer disconnects
- **Severity:** Medium
- **Category:** ux / bug
- **Status:** VERIFIED
- **Location:**
  - `client/src/app/chat_manager/files.rs:371-396` (`fail_incoming_transfers` matches every `status.is_active()` transfer, which includes `AwaitingAcceptance` with `pending_file_end` set, then calls `discard_spool`)
  - called from `events.rs:305` on `Disconnected`, and from `contacts.rs:220` and `mod.rs:726`
- **Problem:** CLAUDE.md justifies failing transfers on disconnect as removing "a stuck progress row". But a held offer whose `FileEnd` already arrived is not stuck: the file is complete on disk and is only waiting for a click.
- **Impact:** A peer sends a file and quits, or the network blips. The recipient comes back to "File transfer interrupted (connection lost)" and the complete file is gone.
- **Recommendation:** In `fail_incoming_transfers`, skip transfers that have an entry in `pending_file_end`. Keep them acceptable; finalize works offline, and the delivery receipt can simply be skipped when there is no session. Only fail transfers that are still streaming. Add a test.
- **Effort:** S

### APP-06: `FileCancel` carries no transfer id, so with transfers both ways it cancels the wrong one
- **Severity:** Medium
- **Category:** bug
- **Status:** VERIFIED (logic). The cancel-after-new-`FileMeta` ordering race is SUSPECTED.
- **Location:**
  - `client/src/app/chat_manager/files.rs:312-332` (`handle_peer_file_cancel` prefers an active incoming stream, otherwise the outgoing send)
  - `files.rs:209-212` (the streaming task emits its own `FileCancel` on the control lane)
- **Problem and impact:**
  - **Wrong transfer cancelled:** Alice sends file A to Bob while Bob streams file B to Alice. Bob declines A, which sends `FileCancel`. Alice resolves the cancel to her *incoming* B, deletes B's spool, and keeps streaming the declined A in full. Bob's sender of B keeps streaming into nothing, then shows "File sent".
  - **Cancel lands on the next file (SUSPECTED):** after `cancel_transfer` on an outgoing send, the old task's `FileCancel` rides the unbounded control lane. Meanwhile a new `send_file`'s `FileMeta` may already be on the file lane. If the session loop writes the new `FileMeta` before the old `FileCancel`, the receiver cancels the new transfer.
- **Recommendation:** Add a direction flag or transfer id to `FileCancel`, as an append-only protocol change (encode and decode changed together). Until then:
  - when both directions are active on a chat, disambiguate conservatively, for example by ignoring a `FileCancel` that arrives while an incoming stream is active and we have not finished sending;
  - make `send_file` refuse while a previous cancelled stream's task is still alive (track a `JoinHandle` and check `is_finished`).
- **Effort:** M

### APP-07: Unaccepted, peer-controlled spools live in the user's Downloads folder and are never swept
- **Severity:** Medium
- **Category:** security / docs
- **Status:** VERIFIED
- **Location:**
  - `core/src/transfer/receiver.rs:279-287` (spool is created in `dest_dir` as `tmp_<uuid>_<sanitised peer name>`)
  - `client/src/app/chat_manager/mod.rs:843-853` (`Drop` neither deletes `incoming_files` spools nor removes `pending_file_end` entries)
  - `Config::temp_dir` (`core/src/types.rs:395`) is never used for spooling: it appears only in `persistence.rs:90` and `support.rs:32`
- **Problem:**
  - Up to 64 MiB per conversation of content the user has *not* accepted is written into `~/Downloads`, under a peer-chosen extension such as `.pdf`, `.jpg` or `.svg`. There, OS indexers, thumbnailers and AV parse it, and the user can open it, before the user has agreed to anything. CLAUDE.md's "nothing reaches the download dir … until `accept_incoming_file()`" contradicts the code, and its later note that "the spool lives *in* the download dir".
  - On app exit or crash mid-receive, or with a held offer pending, the `tmp_*` files stay forever. There is no startup sweep (`rg tmp_` finds none).
- **Impact:** The acceptance gate's privacy and attack-surface promise is weaker than documented, and Downloads accumulates leftovers.
- **Recommendation:**
  - Spool unaccepted offers into `config.temp_dir`, or a hidden `.p2pem-partial/` subdirectory of the download dir (same filesystem, so the rename stays atomic), with a non-executable, extension-less name such as `<uuid>.part`. Rename into Downloads only on accept.
  - Sweep that directory at startup.
  - In `Drop` for `ChatManager`, call `abort_cleanup` on any remaining `incoming_files`.
  - Fix the CLAUDE.md sentence.
- **Effort:** M

### APP-08: Party history paging skips a range when a live broadcast arrives before the first page
- **Severity:** Medium
- **Category:** bug
- **Status:** VERIFIED
- **Location:** `client/src/app/party_manager.rs:1027-1069` (`since_seq = existing.iter().rfind(|e| e.seq > 0)`)
- **Problem:** The next page is requested from the highest seq *in the thread*, not from the end of the page just received. The flow: `Join` → `ListChannels` → `FetchHistory(0)`. A message broadcast in the meantime (seq 501) is pushed by `PartyResponse::Message` before the first `History` page (seq 1 to 200) is applied. After the merge and sort, the last confirmed seq is 501, so the client asks `since_seq = 501`.
- **Impact:** Messages 201 to 500 of that channel or DM are never fetched, silently. This is likely on any busy community.
- **Recommendation:** Compute `since_seq` as the maximum `seq` of the page items for that thread (`page.iter().map(|e| e.seq).max()`). Add a test that interleaves a `Message` broadcast with a full first page.
- **Effort:** S

### APP-09: Chunked-text reassembly is bounded by message count, not bytes (hundreds of MiB per chat)
- **Severity:** Medium
- **Category:** security (DoS) / performance
- **Status:** VERIFIED
- **Location:** `client/src/app/chat_manager/text.rs:101-169`; `MAX_CONCURRENT_PARTIAL_TEXT_PER_CHAT` (16) × `MAX_TEXT_CHUNKS` (512) × chunk size
- **Problem:** An accepted peer can open 16 partial messages per conversation and fill 511 of 512 parts each. That is 16 × 511 × 48 KiB, about 384 MiB, or more if a chunk frame may carry up to 64 KiB. `updated_at` refreshes on every chunk, so the 120 s timeout never fires for a peer that trickles one chunk per message every couple of minutes. A host accepts many peers, each with its own conversation, so the total is unbounded.
- **Impact:** Memory exhaustion of the desktop app from any verified contact, or from any peer when `auto_trust_on_first_use` is on.
- **Recommendation:**
  - Track buffered bytes per chat and globally, for example 32 MiB per chat and 128 MiB total, and drop the oldest partial message when the cap is exceeded.
  - Make the timeout absolute from the *first* chunk (keep `created_at`), not sliding.
- **Effort:** S

### APP-10: Party `connect_and_join` has no TCP connect timeout, and the desktop holds the `PartyManager` lock across it
- **Severity:** Medium
- **Category:** bug / ux
- **Status:** VERIFIED
- **Location:**
  - `client/src/app/party_manager.rs:403` (`TcpStream::connect` with no timeout; the P2P path uses `CONNECT_ATTEMPT_TIMEOUT_SECS` in `session.rs:169`)
  - `desktop/src-tauri/src/commands/party.rs:337-348` (`state.party.lock().await` held through `.connect_and_join(...).await`)
- **Impact:** A typo'd or firewalled community address can block for the OS connect timeout, about 2 minutes on Linux. During that time every Party command and the poll loop's party drain are blocked: the Communities pane freezes. The handshake reads are bounded at 15 s, but the connect is not.
- **Recommendation:** Wrap the connect in `tokio::time::timeout(CONNECT_ATTEMPT_TIMEOUT_SECS)`. Better, restructure so the connect and handshake run without `&mut self` (a free async function returning the client), and the lock is taken only to insert the connection.
- **Effort:** S

### APP-11: After a Party disconnect, unconfirmed sends stay on screen and upload offers stay in memory
- **Severity:** Low (on screen) / Medium (memory)
- **Category:** bug / ux
- **Status:** VERIFIED (client). The UI cannot tell them apart, because the party DTOs do not expose `seq` (`rg seq desktop/src-tauri/src/commands/party.rs` finds nothing).
- **Location:** `client/src/app/party_manager.rs:854-872` (`poll_events`: on `Disconnected` it only sets the status and fails downloads; `awaiting` is not drained)
- **Problem:** Optimistic `seq == 0` envelopes whose reply will never come remain in `messages` and look delivered. `AwaitingReply::UploadOffer { data }` keeps up to `MAX_PARTY_FILE_BYTES` (100 MiB) per offer alive until `remove_server`. `stream_accepted_upload` also clones every chunk (`part.to_vec()`) into the unbounded channel while `data` is still held, which doubles peak memory.
- **Recommendation:**
  - On disconnect, drain `awaiting`: `take_back` each shown message, or mark it "not sent", and drop the upload data.
  - Expose a `pending` flag in the DTO.
  - Stream the upload chunks from a task that consumes `data`, rather than queueing all of them at once.
- **Effort:** S

### APP-12: Messages typed during the handshake return `Ok` and can vanish without an error
- **Severity:** Low
- **Category:** bug (the "Ok + toast" anti-pattern)
- **Status:** VERIFIED
- **Location:**
  - `client/src/app/chat_manager/text.rs:196-262` (`has_session` is true as soon as `spawn_session` registers the handle; `connect.rs:114-124` registers before the dial)
  - `mod.rs:667-676` (`teardown_session` drops `pending_text_sends`)
- **Problem:** While a session is still dialing or waiting on TOFU, `send_message` queues the frames and pushes a "sent" message into history. If the dial fails, or the user or peer rejects the fingerprint, the queued frames die with the task. The message stays as a sent bubble that was never delivered, with no failed state. The same applies to any message whose frame had not reached the wire when the session died.
- **Recommendation:** Track session readiness (set on `SessionEvent::Ready`), and have `send_message` return an error until the session is ready. When `pending_text_sends` entries are dropped at teardown, mark those messages as failed (a new `Message.failed` field with `#[serde(default)]`) so the UI can offer a resend.
- **Effort:** M

### APP-13 (coordinator question): `accept_incoming_connection` can remap a live conversation's session to a new connection
- **Severity:** Low (hardening; needs knowledge of the target chat id)
- **Category:** security
- **Status:** VERIFIED (mechanics)
- **Location:** `client/src/app/chat_manager/events.rs:359-410`
- **Problem:** The fingerprint lookup excludes live chats (`!self.is_connected(id)`). But the fallback `offered_chat_id` is taken with **no** liveness, ownership or fingerprint check:
  - `chat_id_mapping.insert(chat_id, session_id)` (line 373) unconditionally overwrites an existing mapping;
  - `entry(chat_id).and_modify(|c| { c.transport = …; c.reset_sequences(); })` (lines 404-409) mutates whatever chat already has that id.
- **Consequences when the offered id names an existing conversation:**
  - **Remap.** `session_for_chat(victim)` now resolves to the new session, so the user's sends, files, receipts and typing go to the newcomer's session. On the host side, the previous session's events can no longer be resolved to the chat, so its frames are dropped with "Chat not found".
  - **TOFU against the victim's fingerprint.** TOFU then runs against the victim chat's stored fingerprint, giving a "fingerprint changed" prompt. If that chat has no stored fingerprint (still mid-handshake, or cleared by `remove_contact`), it is a first-use prompt, and accepting writes the newcomer's fingerprint onto the existing conversation (`connect.rs:36-40`).
  - **DoS even on reject.** Even if the user rejects, `Disconnected` for the newcomer runs `fail_incoming_transfers(newcomer)`, which resolves to the victim chat and fails its in-flight receives. It also removes the mapping, so a still-live host-side session is left unmapped: `is_connected` reads false and sends fail.
- **Exploitability:** Chat ids are random v4 UUIDs. A host-side chat id is chosen by that conversation's own client, so normally only the legitimate counterpart (or anyone it leaked to) knows it. The benign case, the same peer reconnecting before the old session is noticed dead, behaves acceptably: the same fingerprint is auto-accepted and the old session is orphaned until its idle timeout.
- **Recommendation:** Use `offered_chat_id` only if no chat with that id exists, or if that chat is idle *and* its `peer_fingerprint` is `None` or equal to the presented fingerprint. Otherwise mint `Uuid::new_v4()`. Add a test that offers a live chat's id with a different fingerprint.
- **Effort:** S

### APP-14 (coordinator question): `connect_to_contact` never falls back to the relay after a direct dial fails (confirmed)
- **Severity:** Low
- **Category:** bug / docs
- **Status:** VERIFIED
- **Location:**
  - `client/src/app/chat_manager/connect.rs:425-430` (doc: "otherwise dial — direct addresses first, then the relay")
  - `connect.rs:497-513` (the relay is used only in the `else if` taken when `candidates.is_empty()`)
  - `core/src/network/session.rs:227-237` (`run_client_session_multi` tries only the direct candidates)
- **Problem:** `connect_to_host_candidates` spawns the dial and returns `Ok` immediately. A runtime failure arrives later as `Error` + `Disconnected`, and nothing in `events.rs` retries over the relay. A contact whose invite carries both a stale direct address and a relay token is therefore never reached through the relay. CLAUDE.md's "the relay fallback" claim about `connect_contact` is also inaccurate.
- **Recommendation:** Pass the relay details into the session task, so it falls back after `connect_first_reachable` fails before emitting `Disconnected`. Alternatively, remember a "fallback relay" per session in the manager and redial it on a connect-phase `Error`. Then fix the doc comment.
- **Effort:** M

### APP-15: An unsigned (v1) invite can silently overwrite an existing, even Verified, contact's address and name
- **Severity:** Low
- **Category:** security
- **Status:** VERIFIED
- **Location:** `client/src/app/chat_manager/contacts.rs:77-103` (refresh path)
- **Problem:** Anyone holding Bob's *public* key, which is in every invite he shares, can mint a v1 link carrying Bob's fingerprint, an attacker address and relay token, and any name. `parse_invite_link` accepts it, because v1 only checks fingerprint == hash(public key). `import_contact` then updates `name`, `address`, `addresses`, `relay_server` and `relay_token` on the existing contact without regard to its trust state or the link's signedness. The next `connect_to_contact` dials the attacker. The TOFU mismatch prompt prevents a silent MITM, but the contact card is renamed and the real route is lost.
- **Recommendation:** Only let *signed* invites refresh an existing contact's reachability, or at least refuse when it is `Verified` or `Trusted`. Do not overwrite a user-chosen name on refresh.
- **Effort:** S

### APP-16: `clear_history` / `delete_all_data` leave `is_hosting`, `hosting_port` and UPnP state set, so auto-rehost restarts the listener
- **Severity:** Low
- **Category:** bug
- **Status:** VERIFIED
- **Location:** `client/src/app/chat_manager/mod.rs:748-796` (placeholder chats and tasks are cleared, but `is_hosting`, `hosting_port`, `upnp_cancel` and `external_address` are untouched); `check_rehost_needed` (`mod.rs:461-475`) then returns true.
- **Impact:** After "delete all data", the shell's rehost tick binds the port again and keeps the router mapping alive.
- **Recommendation:** Call `self.stop_hosting()` at the start of `clear_history`.
- **Effort:** S

### APP-17: `delete_chat` leaves `contact_to_chat` entries pointing at the deleted chat
- **Severity:** Low
- **Category:** bug
- **Status:** VERIFIED
- **Location:** `client/src/app/chat_manager/mod.rs:711-745` (does not touch `contact_to_chat`); consumed at `contacts.rs:332-363` and `connect.rs:449-472`
- **Impact:** The dangling id is persisted (`contact_chat_map`). The next `auto_reconnect_contacts` or `connect_to_contact` recreates a chat under the deleted id (`prepare_outgoing_chat`). It is empty, which is harmless, but the deletion is not clean.
- **Recommendation:** In `delete_chat`, `retain` `contact_to_chat` entries whose value is not one of the removed chat ids.
- **Effort:** S

### APP-18: Blocking a contact does not cover a fingerprint-less contact or a pending TOFU prompt
- **Severity:** Low
- **Category:** security / ux
- **Status:** VERIFIED
- **Location:** `client/src/app/chat_manager/contacts.rs:190-233`; `connect.rs:19-47` (`confirm_fingerprint` never consults `is_fingerprint_blocked`)
- **Problem:**
  - A contact with `fingerprint: None` can be "Blocked" (a toast says so), but `is_fingerprint_blocked` can never match it.
  - A peer whose prompt is already queued when the user blocks them can still be accepted.
- **Recommendation:**
  - Refuse to block, or warn, when there is no fingerprint.
  - In `block_contact`, drop queued prompts for that fingerprint and answer `false`.
  - Re-check `is_fingerprint_blocked` in `confirm_fingerprint`.
- **Effort:** S

### APP-19: The `awaiting_ack` cap clears every conversation's pending receipts at once
- **Severity:** Low
- **Category:** bug
- **Status:** VERIFIED
- **Location:** `client/src/app/chat_manager/events.rs:39-53`
- **Impact:** One long-lived session with a peer that predates `Ack` fills the global 2048-entry map, then `clear()` wipes the receipts of all other sessions. Their messages never get ✓.
- **Recommendation:** Cap per session, for example by evicting the oldest entries for `session_id` only, or disable receipt tracking for a session after N unacked sends.
- **Effort:** S

### APP-20: Received timestamps are inconsistent and peer-controlled for large messages
- **Severity:** Low
- **Category:** bug / security (UI spoofing)
- **Status:** VERIFIED
- **Location:** `client/src/app/chat_manager/events.rs:502-503` (single `Text`: the peer's `timestamp` is ignored and `Utc::now()` used); `text.rs:162-165` (chunked: the peer's `timestamp_millis` is trusted)
- **Impact:** A peer can back-date or future-date any message over 64 KiB (the UI sorts or labels by it), and small and large messages behave differently.
- **Recommendation:** Use local receipt time for both, optionally keeping the peer's time separately as "sent at".
- **Effort:** S

### APP-21: Blocking file I/O on the manager lock and runtime worker during receive and finalize
- **Severity:** Low
- **Category:** performance
- **Status:** VERIFIED
- **Location:** `core/src/transfer/receiver.rs:301-355` (`std::fs` writes, `sync_all`, `rename`), called from `poll_session_events` under the tokio `Mutex` (`events.rs:676`, `files.rs:520-525`)
- **Impact:** Each 64 KiB chunk is a synchronous write, and finalizing a multi-GB file runs `fsync` inline. That stalls the poll loop, every command, and a tokio worker.
- **Recommendation:** Move spool writes to a per-transfer writer task (an mpsc of chunks) and run finalize on `spawn_blocking`, reporting the result back via an event.
- **Effort:** M

### APP-22: O(n) scans per event and whole-history clones per save
- **Severity:** Low
- **Category:** performance
- **Status:** VERIFIED
- **Location:**
  - `events.rs:728-731` (`mark_delivered` does a forward linear search of all messages per Ack; searching from the end with `.rev()` finds recent ones in O(1))
  - `party_manager.rs:1023` and `1044` (dedup by a linear `any` per message or page item, which is quadratic over a channel's life)
  - `persistence.rs:315-330` (`history_snapshot` clones every message under the lock)
  - `desktop/src-tauri/src/lib.rs:153-175` (`state_signature` counts `delivered` over all messages every poll tick, under the lock)
- **Problem:** There is no retention or pruning of P2P history or of Party in-memory `messages` anywhere (`rg prune|retention` finds nothing).
- **Recommendation:**
  - Iterate in reverse in `mark_delivered`.
  - Keep a `HashSet<u64>` of seqs per party thread.
  - Maintain a dirty counter instead of rehashing every tick.
  - Consider bounding Party in-memory history and paging from the server.
- **Effort:** M

### APP-23: Concurrent downloads of the same Party file collide
- **Severity:** Low
- **Category:** bug
- **Status:** VERIFIED
- **Location:** `client/src/app/party_manager.rs:824-837` (`insert` replaces the earlier waiter; for chunked downloads both offset-0 replies arrive and the second fails "out of order")
- **Impact:** Double-clicking a large file fails both downloads; the first caller's receiver is dropped.
- **Recommendation:** If a download for that hash is in flight, attach the new waiter to it (a `Vec` of senders) instead of re-requesting.
- **Effort:** S

### APP-24: Dead code and dead fields
- **Severity:** Info
- **Category:** maintainability / unfinished
- **Status:** VERIFIED
- **Location and items:**
  - `core/src/transfer/receiver.rs:105-242`: async `IncomingFile` and `reserve_unique_path` are used only by tests. The doc comment of `reserve_unique_path_sync` (`:11-16`) and of `reserve_unique_path` (`:60`) are spliced together mid-sentence.
  - `core/src/types.rs`:
    - `SessionRole` and `SessionStatus` are unused;
    - `Config::temp_dir` is unused for any I/O;
    - `Contact::{notes, tags, last_seen}` are never written;
    - `FileTransferState::seq` is incremented (`events.rs:670`) and never read.
  - `Chat::send_seq` is still incremented (`files.rs:83`, `text.rs:287`) and stamped into frames, although the session overwrites every seq.
  - `support.rs:113-118` and the panic hook write crash and diagnostics files under the *terminal* client's `ProjectDirs("…","EncryptedMessenger")`, so the desktop app (which links this crate) would write there too if it ever installs the hook.
- **Recommendation:** Delete or wire up these items, and fix the spliced doc comments.
- **Effort:** S

## Things done well

- **Session teardown is centralised.** `teardown_session` is the single place a session ends, and `Drop` aborts tasks, so no forgotten-piece bugs.
- **TOFU prompts are a queue keyed by session.** They are peeked, never consumed, and dropped when their session dies. `confirm_fingerprint` matches by session id.
- **Trust is enforced in the right layer.** Invites are clamped to `Unverified` in `import_contact` (and `Blocked` is preserved). `fingerprint_known_trusted` requires a `Verified` or `Trusted` contact. Fingerprints are compared case-insensitively on the block paths.
- **The receiver is hardened.**
  - `IncomingFileSync::new` sanitises the peer name itself, with the directory and the name kept separate.
  - The declared-size check runs before each write.
  - `reserve_unique_path_sync` uses `create_new` (O_EXCL), so existing files in Downloads are never overwritten.
- **Outgoing sends stream off the lock** with a bounded lane and atomic cancel, progress and failed flags. The rule "one outgoing transfer per conversation", which keeps the wire format safe, is enforced and documented.
- **Persistence writes are safe.** `write_file_atomic` (fsync + rename, 0600); compact JSON; `apply_history` replaces rather than merges; plaintext migration escalates delete → truncate → error toast; config paths are sanitised by path component.
- **Logs do not leak message text.** The `ProtocolMessage` `Debug` impl redacts text, so the default debug-level logging and the diagnostics bundle contain no message bodies. `LogBuffer` is bounded and survives a poisoned lock.
- **PartyManager is robust against its server.**
  - Every send and offer goes through one FIFO (`awaiting`), and pending rows are found by value rather than by a stored index.
  - Envelopes with `seq == 0` from the server are rejected.
  - Download hashes are verified, and chunk ordering and size are bounded.
  - Separate read and write pumps avoid the cancel-safety bug.
- **The suites cover many past regressions:** adversarial unit tests, e2e transfer and reconnect tests, and the persistence damage tests.

## Open questions

1. Is the spool-in-Downloads design (APP-07) a deliberate trade-off that the CLAUDE.md "nothing reaches the download dir" sentence predates, or should unaccepted data go to a private directory?
2. What is the maximum `FileChunk` / `TextChunk` payload the decoder admits (`TEXT_CHUNK_BYTES` vs `MAX_TEXT_MESSAGE_BYTES`)? It sets the exact bound in APP-09 and the overshoot past the 64 MiB unaccepted-spool cap.
3. Does the TUI remove a stale disconnected Party connection on rejoin, as the desktop does (`party.rs:367-379`)? If not, APP-11's memory retention persists for the life of the TUI process. This is not verified (TUI code is out of scope).
4. Can the desktop or TUI call `connect_to_host` with `existing_chat_id` of a chat whose session is still live? If so, `spawn_session` replaces the handle under the same key while the old session's `pending_text_sends` remain, which would misattribute delivery receipts (SUSPECTED). The current desktop call sites pass `None` (`commands/connect.rs:64` and `:202`).
